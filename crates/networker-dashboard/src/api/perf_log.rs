use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct IngestRequest {
    pub session_id: Option<String>,
    pub entries: Vec<crate::db::perf_log::PerfLogInput>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub kind: Option<String>,
    pub path: Option<String>,
    pub user_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;
const MAX_BATCH: usize = 200;

/// POST /api/perf-log — ingest a batch of perf log entries
async fn ingest(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = req.extensions().get::<AuthUser>().unwrap().clone();

    let body = axum::body::to_bytes(req.into_body(), 1024 * 256)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: IngestRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if payload.entries.len() > MAX_BATCH {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in perf_log ingest");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let inserted = crate::db::perf_log::insert_batch(
        &client,
        &user.user_id,
        payload.session_id.as_deref(),
        &payload.entries,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to insert perf logs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({ "inserted": inserted })))
}

/// GET /api/perf-log — list perf log entries with filters
async fn list_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<crate::db::perf_log::PerfLogRow>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in perf_log list");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let logs = crate::db::perf_log::list(
        &client,
        q.kind.as_deref(),
        q.path.as_deref(),
        q.user_id.as_ref(),
        q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        q.offset.unwrap_or(0).max(0),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list perf logs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(logs))
}

/// GET /api/perf-log/stats — aggregate stats
async fn get_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in perf_log stats");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stats = crate::db::perf_log::stats(&client).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to compute perf log stats");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(stats))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/perf-log", post(ingest).get(list_logs))
        .route("/perf-log/stats", get(get_stats))
        .with_state(state)
}
