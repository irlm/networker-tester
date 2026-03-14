use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Deserialize)]
pub struct ListRunsQuery {
    pub target_host: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListRunsQuery>,
) -> Result<Json<Vec<crate::db::runs::RunSummary>>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let runs = crate::db::runs::list(
        &client,
        q.target_host.as_deref(),
        q.limit.unwrap_or(50),
        q.offset.unwrap_or(0),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(runs))
}

async fn get_run_attempts(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let attempts = crate::db::runs::get_attempts(&client, &run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(attempts))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/runs", get(list_runs))
        .route("/runs/:run_id/attempts", get(get_run_attempts))
        .with_state(state)
}
