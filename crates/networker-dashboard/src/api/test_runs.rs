//! REST v2: `/api/v2/projects/:pid/test-runs` + `/api/v2/test-runs/:id`.
//!
//! Spec: `.critique/refactor/03-spec.md` §3.2.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::ProjectContext;
use crate::AppState;
use networker_common::{RunStatus, TestRun};

#[derive(Deserialize, Default)]
pub struct ListRunsQuery {
    pub status: Option<String>,
    pub endpoint_kind: Option<String>,
    pub has_artifact: Option<bool>,
    pub comparison_group_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub before: Option<chrono::DateTime<chrono::Utc>>,
}

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

async fn list_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListRunsQuery>,
    req: axum::extract::Request,
) -> Result<Json<Vec<TestRun>>, StatusCode> {
    let ctx = req
        .extensions()
        .get::<ProjectContext>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let status_filter = q.status.as_deref().and_then(RunStatus::parse_str);
    // NOTE: endpoint_kind + before pagination not yet wired through db::test_runs::list;
    // they're parsed here so the client contract is correct, with TODO to expose
    // them in the DB layer once Agent A adds the helper.
    let _ = (q.endpoint_kind, q.before);
    let rows = crate::db::test_runs::list(
        &client,
        &ctx.project_id,
        status_filter,
        q.has_artifact,
        q.comparison_group_id.as_ref(),
        limit,
        0,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "list test_run failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(rows))
}

async fn get_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<TestRun>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = crate::db::test_runs::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(run))
}

async fn artifact_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::db::benchmark_artifacts::BenchmarkArtifact>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let art = crate::db::benchmark_artifacts::get_for_run(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(art))
}

async fn attempts_handler(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // TODO(agent-A): expose per-protocol phase tables (Dns/Tcp/Tls/Http/Udp/...)
    // joined on test_run_id once the rename from run_id is finalised. Returning
    // an empty placeholder so the route exists and the dashboard can wire UI.
    Ok(Json(serde_json::json!({ "attempts": [] })))
}

async fn cancel_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<TestRun>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = crate::db::test_runs::update_status(&client, &id, RunStatus::Cancelled)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Fan-out cancel to any agent currently owning the run.
    let msg = networker_common::messages::ControlMessage::CancelRun { run_id: id };
    if let Some(agent_id) = state.agents.any_online_agent().await {
        let _ = state.agents.send_to_agent(&agent_id, &msg).await;
    }
    Ok(Json(run))
}

#[derive(Deserialize)]
pub struct CompareRequest {
    pub run_ids: Vec<Uuid>,
}

async fn compare_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompareRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut runs = Vec::with_capacity(req.run_ids.len());
    let mut artifacts = Vec::with_capacity(req.run_ids.len());
    for id in &req.run_ids {
        if let Ok(Some(r)) = crate::db::test_runs::get(&client, id).await {
            runs.push(r);
        }
        if let Ok(Some(a)) = crate::db::benchmark_artifacts::get_for_run(&client, id).await {
            artifacts.push(a);
        }
    }
    Ok(Json(serde_json::json!({
        "runs": runs,
        "artifacts": artifacts,
    })))
}

/// Project-scoped router (mounted under `/api/v2/projects/{project_id}`).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/test-runs", get(list_handler))
        .with_state(state)
}

/// Flat router (mounted under `/api/v2`).
pub fn flat_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/test-runs/{id}", get(get_handler))
        .route("/test-runs/{id}/artifact", get(artifact_handler))
        .route("/test-runs/{id}/attempts", get(attempts_handler))
        .route("/test-runs/{id}/cancel", post(cancel_handler))
        .route("/test-runs/compare", post(compare_handler))
        .with_state(state)
}
