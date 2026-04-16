//! Leaderboard API (simple benchmark results — language/runtime comparisons).
//!
//! v0.28.0: The old `db::benchmarks` module was removed as part of the
//! TestConfig unification. The leaderboard table still exists in the DB but
//! the CRUD helpers were dropped. These handlers return empty stubs until the
//! leaderboard is rebuilt on top of `test_run` + `benchmark_artifact`.

use axum::{http::StatusCode, routing::get, Json, Router};
use std::sync::Arc;

use crate::AppState;

/// GET /api/leaderboard — stub
async fn leaderboard() -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    Ok(Json(vec![]))
}

/// GET /api/leaderboard/runs — stub
async fn list_runs() -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    Ok(Json(vec![]))
}

/// GET /api/leaderboard/grouped — stub
async fn grouped_leaderboard() -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(serde_json::json!({ "groups": [] })))
}

pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/leaderboard", get(leaderboard))
        .route("/leaderboard/grouped", get(grouped_leaderboard))
        .route("/leaderboard/runs", get(list_runs))
        .with_state(state)
}

pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new().with_state(state)
}
