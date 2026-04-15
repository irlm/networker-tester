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

#[derive(Deserialize)]
struct BrowserWsQuery {
    token: Option<String>,
    /// Last event sequence number the client saw. On reconnect, the hub
    /// replays every buffered event with `seq > since` before tailing the
    /// live channel, so a transient network drop doesn't create holes in the
    /// UI's live feed. Omitted or 0 on first connect (no replay).
    since: Option<u64>,
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
            let since = q.since.unwrap_or(0);
            tracing::info!(
                email = %claims.email,
                role = %claims.role,
                since,
                "Browser WebSocket authenticated"
            );
            Ok(ws.on_upgrade(move |socket| handle_browser_socket(socket, state, since)))
        }
        Err(_) => {
            tracing::warn!("Browser WebSocket rejected: invalid token");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

async fn handle_browser_socket(socket: ws::WebSocket, state: Arc<AppState>, since: u64) {
    let (mut ws_sink, mut ws_stream) = socket.split();

    // Subscribe *before* snapshotting the replay log so any event published
    // during the small window between the two is seen on both — the dedup
    // below (`seq > max_replayed`) removes the duplicate from the live path.
    let mut rx = state.events_tx.subscribe();
    let replay = if since > 0 {
        state.events_tx.replay_since(since)
    } else {
        Vec::new()
    };
    let max_replayed = replay.iter().map(|e| e.seq).max().unwrap_or(since);

    let forward_task = tokio::spawn(async move {
        // 1. Flush any missed events to the client before tailing live.
        for e in replay {
            match serde_json::to_string(&e) {
                Ok(text) => {
                    if ws_sink.send(ws::Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(err) => tracing::warn!(error = %err, "failed to encode replay SeqEvent"),
            }
        }

        // 2. Tail the live channel, skipping anything already covered by the
        //    replay snapshot. `Lagged` is recoverable: client reconnects with
        //    its last-seen seq to trigger a new replay.
        loop {
            match rx.recv().await {
                Ok(seqe) => {
                    if seqe.seq <= max_replayed {
                        continue;
                    }
                    match serde_json::to_string(&seqe) {
                        Ok(text) => {
                            if ws_sink.send(ws::Message::Text(text.into())).await.is_err() {
                                return;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "failed to encode live SeqEvent");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        skipped = n,
                        "browser WS lagged the broadcast — client should reconnect with ?since="
                    );
                    // Keep the socket open; the client's next reconnect cycle
                    // will resync via the replay log.
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
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
