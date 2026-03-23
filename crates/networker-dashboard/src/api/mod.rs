mod agents;
mod auth;
mod cloud;
mod cloud_connections;
mod dashboard;
mod deployments;
mod inventory;
mod jobs;
mod modes;
mod project_members;
mod projects;
mod runs;
mod schedules;
mod update;
mod url_tests;
mod users;
mod version;

use axum::{middleware, Router};
use std::sync::Arc;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public = Router::new().merge(auth::router(state.clone()));

    // Protected routes (require valid JWT)
    let protected = Router::new()
        .merge(auth::protected_router(state.clone()))
        .merge(agents::router(state.clone()))
        .merge(jobs::router(state.clone()))
        .merge(runs::router(state.clone()))
        .merge(url_tests::router(state.clone()))
        .merge(dashboard::router(state.clone()))
        .merge(deployments::router(state.clone()))
        .merge(cloud::router(state.clone()))
        .merge(cloud_connections::router(state.clone()))
        .merge(modes::router(state.clone()))
        .merge(version::router(state.clone()))
        .merge(update::router(state.clone()))
        .merge(inventory::router(state.clone()))
        .merge(schedules::router(state.clone()))
        .merge(users::router(state.clone()))
        .merge(projects::router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    // Project-scoped routes (require auth + project membership)
    let project_scoped = Router::new()
        .merge(project_members::router(state.clone()))
        .merge(projects::detail_router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_project,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    let project_nested = Router::new().nest("/api/projects/:project_id", project_scoped);

    public.merge(protected).merge(project_nested)
}
