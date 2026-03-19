mod agents;
mod auth;
mod dashboard;
mod jobs;
mod runs;
mod url_tests;

use axum::{middleware, Router};
use std::sync::Arc;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public = Router::new().merge(auth::router(state.clone()));

    // Protected routes (require valid JWT)
    let protected = Router::new()
        .merge(agents::router(state.clone()))
        .merge(jobs::router(state.clone()))
        .merge(runs::router(state.clone()))
        .merge(url_tests::router(state.clone()))
        .merge(dashboard::router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    public.merge(protected)
}
