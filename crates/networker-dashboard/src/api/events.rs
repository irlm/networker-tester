use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::get,
    Router,
};
use futures::stream::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;

/// GET /events/approval — SSE stream for approval notifications.
/// Requires valid JWT (protected route).
pub async fn approval_events(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    // Verify auth user exists in extensions (injected by require_auth middleware)
    let _auth = req.extensions().get::<crate::auth::AuthUser>().cloned();

    let rx = state.approval_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(data) => Some(Ok::<_, Infallible>(
                Event::default().event("approval").data(data),
            )),
            Err(_) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/events/approval", get(approval_events))
        .with_state(state)
}
