use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext};
use crate::AppState;

const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 1000;

// ── Query params ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LogsQueryParams {
    pub service: Option<String>,
    pub level: Option<String>,
    pub config_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub search: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct StatsQueryParams {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

// ── Pipeline status response ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PipelineStatusResponse {
    pub entries_written: u64,
    pub entries_dropped: u64,
    pub flush_count: u64,
    pub flush_errors: u64,
    pub last_flush_ms: u64,
    pub queue_depth: u32,
    pub status: &'static str,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /api/logs — query structured logs with optional filters.
///
/// Non-admin users have `project_id` forced from their auth context.
/// Platform admins can query any project or all.
async fn query_logs(
    State(state): State<Arc<AppState>>,
    Query(mut params): Query<LogsQueryParams>,
    req: axum::extract::Request,
) -> Result<Json<networker_log::query::LogQueryResponse>, StatusCode> {
    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Non-admins: force project_id from the ProjectContext injected by middleware,
    // ignoring any project_id the caller may have supplied in the query string.
    if !user.is_platform_admin {
        let ctx = req.extensions().get::<ProjectContext>().cloned();
        params.project_id = ctx.map(|c| c.project_id);
    }

    let to = params.to.unwrap_or_else(Utc::now);
    let from = params
        .from
        .unwrap_or_else(|| to - chrono::Duration::hours(1));

    let limit = params
        .limit
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    let min_level = params
        .level
        .as_deref()
        .and_then(|l| l.parse::<networker_log::Level>().ok())
        .map(|lv| lv.as_db());

    let q = networker_log::query::LogQuery {
        service: params.service,
        min_level,
        config_id: params.config_id,
        project_id: params.project_id,
        search: params.search,
        from,
        to,
        limit,
        offset,
    };

    let client = state.logs_db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in query_logs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let result = networker_log::query::list(&client, &q).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to query logs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(result))
}

/// GET /api/logs/stats — per-service level-bucket counts over a time window.
async fn query_logs_stats(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StatsQueryParams>,
    req: axum::extract::Request,
) -> Result<Json<networker_log::query::LogStats>, StatusCode> {
    let _user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let to = params.to.unwrap_or_else(Utc::now);
    let from = params
        .from
        .unwrap_or_else(|| to - chrono::Duration::hours(1));

    let client = state.logs_db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in query_logs_stats");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stats = networker_log::query::stats(&client, from, to)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to compute log stats");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(stats))
}

/// GET /api/logs/pipeline-status — live metrics from the log pipeline.
async fn pipeline_status(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<PipelineStatusResponse>, StatusCode> {
    let _user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let snap = state.log_metrics.snapshot();
    let status = snap.status();

    Ok(Json(PipelineStatusResponse {
        entries_written: snap.entries_written,
        entries_dropped: snap.entries_dropped,
        flush_count: snap.flush_count,
        flush_errors: snap.flush_errors,
        last_flush_ms: snap.last_flush_ms,
        queue_depth: snap.queue_depth,
        status,
    }))
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/logs", get(query_logs))
        .route("/logs/stats", get(query_logs_stats))
        .route("/logs/pipeline-status", get(pipeline_status))
        .with_state(state)
}
