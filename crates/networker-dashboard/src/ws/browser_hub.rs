use std::sync::Arc;
use axum::{
    extract::{ws, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};

use crate::AppState;
use networker_common::messages::DashboardEvent;
use networker_common::protocol;

async fn browser_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_browser_socket(socket, state))
}

async fn handle_browser_socket(socket: ws::WebSocket, state: Arc<AppState>) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let mut rx = state.events_tx.subscribe();

    // Forward all dashboard events to the browser
    let forward_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if let Ok(text) = protocol::encode(&event) {
                if ws_sink.send(ws::Message::Text(text.into())).await.is_err() {
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
