use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct DashboardSummary {
    pub agents_online: i64,
    pub jobs_running: i64,
    pub runs_24h: i64,
    pub jobs_pending: i64,
}

async fn summary(State(state): State<Arc<AppState>>) -> Result<Json<DashboardSummary>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let agents_online: i64 = client
        .query_one("SELECT COUNT(*) FROM agent WHERE status = 'online'", &[])
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    let jobs_running: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM job WHERE status IN ('running', 'assigned')",
            &[],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    let runs_24h: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM job WHERE created_at > now() - interval '24 hours'",
            &[],
        )
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    let jobs_pending: i64 = client
        .query_one("SELECT COUNT(*) FROM job WHERE status = 'pending'", &[])
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

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/dashboard/summary", get(summary))
        .with_state(state)
}
