mod auth;
mod agents;
mod jobs;
mod runs;
mod dashboard;

use std::sync::Arc;
use axum::Router;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(auth::router(state.clone()))
        .merge(agents::router(state.clone()))
        .merge(jobs::router(state.clone()))
        .merge(runs::router(state.clone()))
        .merge(dashboard::router(state.clone()))
}
