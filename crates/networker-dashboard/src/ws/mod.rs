pub mod agent_hub;
pub mod browser_hub;
pub mod tester_hub;

use axum::Router;
use std::sync::Arc;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(agent_hub::router(state.clone()))
        .merge(browser_hub::router(state.clone()))
        .merge(tester_hub::router(state.clone()))
}
