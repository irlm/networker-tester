mod admin;
mod agents;
mod auth;
mod bench_tokens;
mod benchmark_callbacks;
mod benchmark_catalog;
mod benchmark_configs;
mod benchmarks;
mod cloud;
mod cloud_accounts;
mod cloud_connections;
mod command_approvals;
mod dashboard;
mod deployments;
mod events;
mod inventory;
mod invites;
mod jobs;
mod leaderboard;
mod logs;
mod modes;
mod perf_log;
mod project_members;
mod projects;
mod runs;
mod schedules;
mod share_links;
mod sso_admin;
// Task 14: tester REST handlers land here. Wiring into `project_scoped`
// is deferred to Task 18; for now we declare the module so it compiles
// and its unit tests run.
mod system_health;
mod testers;
mod tls_profiles;
mod update;
mod url_tests;
mod users;
mod version;
mod visibility;
mod zones;

use axum::{middleware, Router};
use std::sync::Arc;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public = Router::new()
        .merge(auth::router(state.clone()))
        .merge(share_links::public_router(state.clone()))
        .merge(invites::public_router(state.clone()))
        .merge(leaderboard::public_router(state.clone()))
        .merge(benchmark_callbacks::public_router(state.clone()))
        .merge(system_health::public_router(state.clone()));

    // Protected flat routes (require valid JWT, global/platform resources only)
    let protected_flat = Router::new()
        .merge(auth::protected_router(state.clone()))
        .merge(modes::router(state.clone()))
        .merge(version::router(state.clone()))
        .merge(update::router(state.clone()))
        .merge(users::router(state.clone()))
        .merge(projects::router(state.clone()))
        .merge(events::router(state.clone()))
        .merge(admin::router(state.clone()))
        .merge(sso_admin::router(state.clone()))
        .merge(system_health::router(state.clone()))
        .merge(bench_tokens::router(state.clone()))
        .merge(perf_log::router(state.clone()))
        .merge(logs::router(state.clone()))
        .merge(leaderboard::protected_router(state.clone()))
        .merge(zones::router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    // Project-scoped routes (require auth + project membership)
    let project_scoped = Router::new()
        .merge(agents::project_router(state.clone()))
        .merge(benchmarks::project_router(state.clone()))
        .merge(jobs::project_router(state.clone()))
        .merge(runs::project_router(state.clone()))
        .merge(schedules::project_router(state.clone()))
        .merge(dashboard::project_router(state.clone()))
        .merge(deployments::project_router(state.clone()))
        .merge(cloud::project_router(state.clone()))
        .merge(cloud_accounts::project_router(state.clone()))
        .merge(cloud_connections::project_router(state.clone()))
        .merge(inventory::project_router(state.clone()))
        .merge(url_tests::project_router(state.clone()))
        .merge(tls_profiles::project_router(state.clone()))
        .merge(project_members::router(state.clone()))
        .merge(share_links::project_router(state.clone()))
        .merge(command_approvals::project_router(state.clone()))
        .merge(visibility::project_router(state.clone()))
        .merge(invites::project_router(state.clone()))
        .merge(benchmark_configs::project_router(state.clone()))
        .merge(benchmark_catalog::project_router(state.clone()))
        .merge(testers::project_router(state.clone()))
        .merge(projects::detail_router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_project,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    let project_nested = Router::new().nest("/projects/{project_id}", project_scoped);

    public.merge(protected_flat).merge(project_nested)
}
