use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::ProjectContext;
use crate::AppState;

#[derive(Serialize)]
pub struct DashboardSummary {
    pub agents_online: i64,
    pub jobs_running: i64,
    pub runs_24h: i64,
    pub jobs_pending: i64,
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn summary_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<DashboardSummary>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let project_id = ctx.project_id;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in dashboard summary (scoped)");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let agents_online: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM agent WHERE status = 'online' AND project_id = $1",
            &[&project_id],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    let jobs_running: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM test_run WHERE status IN ('running', 'queued') AND project_id = $1",
            &[&project_id],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    let runs_24h: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM test_run WHERE created_at > now() - interval '24 hours' AND project_id = $1",
            &[&project_id],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    let jobs_pending: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM test_run WHERE status = 'queued' AND project_id = $1",
            &[&project_id],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    Ok(Json(DashboardSummary {
        agents_online,
        jobs_running,
        runs_24h,
        jobs_pending,
    }))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/dashboard/summary", get(summary_scoped))
        .with_state(state)
}
