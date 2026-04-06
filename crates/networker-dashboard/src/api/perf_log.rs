use axum::{
    extract::State,
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

// Field length limits matching the schema VARCHAR constraints
const MAX_SESSION_ID: usize = 64;
const MAX_METHOD: usize = 10;
const MAX_PATH: usize = 500;
const MAX_SOURCE: usize = 20;
const MAX_COMPONENT: usize = 100;
const MAX_TRIGGER: usize = 100;
const MAX_KIND: usize = 10;

/// POST /api/perf-log — ingest a batch of perf log entries (admin only)
async fn ingest(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_platform_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    let body = axum::body::to_bytes(req.into_body(), 1024 * 256)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: IngestRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if payload.entries.len() > MAX_BATCH {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate field lengths before touching the database
    if let Some(ref sid) = payload.session_id {
        if sid.len() > MAX_SESSION_ID {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    for entry in &payload.entries {
        if entry.kind.len() > MAX_KIND {
            return Err(StatusCode::BAD_REQUEST);
        }
        if entry.method.as_ref().is_some_and(|v| v.len() > MAX_METHOD) {
            return Err(StatusCode::BAD_REQUEST);
        }
        if entry.path.as_ref().is_some_and(|v| v.len() > MAX_PATH) {
            return Err(StatusCode::BAD_REQUEST);
        }
        if entry.source.as_ref().is_some_and(|v| v.len() > MAX_SOURCE) {
            return Err(StatusCode::BAD_REQUEST);
        }
        if entry
            .component
            .as_ref()
            .is_some_and(|v| v.len() > MAX_COMPONENT)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        if entry
            .trigger
            .as_ref()
            .is_some_and(|v| v.len() > MAX_TRIGGER)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let mut client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in perf_log ingest");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let inserted = crate::db::perf_log::insert_batch(
        &mut client,
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

/// GET /api/perf-log — list perf log entries with filters (admin only)
async fn list_logs(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::perf_log::PerfLogRow>>, StatusCode> {
    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_platform_admin {
        return Err(StatusCode::FORBIDDEN);
    }
    let q: ListQuery = axum::extract::Query::try_from_uri(req.uri())
        .map(|q| q.0)
        .unwrap_or(ListQuery {
            kind: None,
            path: None,
            user_id: None,
            limit: None,
            offset: None,
        });
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

/// GET /api/perf-log/stats — aggregate stats (admin only)
async fn get_stats(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_platform_admin {
        return Err(StatusCode::FORBIDDEN);
    }
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
