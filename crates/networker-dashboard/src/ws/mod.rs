pub mod agent_hub;
pub mod browser_hub;

use std::sync::Arc;
use axum::Router;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(agent_hub::router(state.clone()))
        .merge(browser_hub::router(state.clone()))
}
