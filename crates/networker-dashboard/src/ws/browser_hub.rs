use axum::{
    extract::{ws, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use networker_common::protocol;

#[derive(Deserialize)]
struct BrowserWsQuery {
    token: Option<String>,
}

async fn browser_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(q): Query<BrowserWsQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    // Validate JWT token before upgrading the WebSocket connection
    let token = q.token.as_deref().unwrap_or("");
    if token.is_empty() {
        tracing::warn!("Browser WebSocket rejected: no token provided");
        return Err(StatusCode::UNAUTHORIZED);
    }

    match crate::auth::validate_token(token, &state.jwt_secret) {
        Ok(claims) => {
            tracing::info!(
                username = %claims.username,
                role = %claims.role,
                "Browser WebSocket authenticated"
            );
            Ok(ws.on_upgrade(move |socket| handle_browser_socket(socket, state)))
        }
        Err(_) => {
            tracing::warn!("Browser WebSocket rejected: invalid token");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

async fn handle_browser_socket(socket: ws::WebSocket, state: Arc<AppState>) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let mut rx = state.events_tx.subscribe();

    // Forward all dashboard events to the browser
    let forward_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if let Ok(text) = protocol::encode(&event) {
                if ws_sink.send(ws::Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Keep connection alive by reading (handle browser commands in the future)
    while let Some(Ok(msg)) = ws_stream.next().await {
        match msg {
            ws::Message::Close(_) => break,
            ws::Message::Ping(data) => {
                // Pong is handled automatically by axum
                let _ = data;
            }
            _ => {}
        }
    }

    forward_task.abort();
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws/dashboard", get(browser_ws_handler))
        .with_state(state)
}
