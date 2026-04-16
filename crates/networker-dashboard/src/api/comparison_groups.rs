//! REST v2: `/api/v2/projects/:pid/comparison-groups` + `/api/v2/comparison-groups/:id`.
//!
//! Comparison groups launch N `TestConfig` + `TestRun` pairs in a batch,
//! varying endpoint/runner across cells while sharing a common workload.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;
use networker_common::{ComparisonCell, ComparisonGroup, Methodology, TestRun, Workload};

// ── request / response types ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateComparisonGroupRequest {
    pub name: String,
    pub base_workload: Workload,
    #[serde(default)]
    pub methodology: Option<Methodology>,
    pub cells: Vec<ComparisonCell>,
}

#[derive(Serialize)]
pub struct ComparisonGroupDetail {
    #[serde(flatten)]
    pub group: ComparisonGroup,
    pub runs: Vec<TestRun>,
}

// ── handlers ────────────────────────────────────────────────────────────

/// POST /api/v2/projects/:pid/comparison-groups
///
/// Creates the group, then for EACH cell creates a TestConfig + TestRun
/// (status = queued) with `comparison_group_id` set.
async fn create_handler(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<ComparisonGroupDetail>, StatusCode> {
    let ctx = req
        .extensions()
        .get::<ProjectContext>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();
    let user = req
        .extensions()
        .get::<AuthUser>()
        .ok_or(StatusCode::UNAUTHORIZED)?
        .clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: CreateComparisonGroupRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if payload.cells.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // 1. Create the comparison_group row.
    let group_id = crate::db::comparison_groups::create(
        &client,
        &ctx.project_id,
        &payload.name,
        &payload.base_workload,
        payload.methodology.as_ref(),
        &payload.cells,
        Some(&user.user_id),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "create comparison_group failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 2. For each cell, create a test_config + test_run.
    let mut runs = Vec::with_capacity(payload.cells.len());
    for cell in &payload.cells {
        // Create a per-cell TestConfig sharing the group's workload/methodology.
        let config_name = format!("{} — {}", payload.name, cell.label);
        let cfg = crate::db::test_configs::create(
            &client,
            &crate::db::test_configs::NewTestConfig {
                project_id: &ctx.project_id,
                name: &config_name,
                description: Some(&format!(
                    "Auto-created by comparison group: {}",
                    payload.name
                )),
                endpoint: &cell.endpoint,
                workload: &payload.base_workload,
                methodology: payload.methodology.as_ref(),
                max_duration_secs: 900,
                created_by: Some(&user.user_id),
            },
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cell_label = %cell.label, "create test_config for cell failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Create the test_run linked to the comparison group.
        let run = crate::db::test_runs::create(
            &client,
            &crate::db::test_runs::NewTestRun {
                test_config_id: &cfg.id,
                project_id: &ctx.project_id,
                tester_id: cell.runner_id.as_ref(),
                worker_id: None,
                comparison_group_id: Some(&group_id),
            },
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cell_label = %cell.label, "create test_run for cell failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        runs.push(run);
    }

    // 3. Fetch the group back (for timestamps / defaults).
    let group = crate::db::comparison_groups::get(&client, &group_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(ComparisonGroupDetail { group, runs }))
}

/// GET /api/v2/projects/:pid/comparison-groups
async fn list_handler(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<ComparisonGroup>>, StatusCode> {
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
    let rows = crate::db::comparison_groups::list(&client, &ctx.project_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows))
}

/// GET /api/v2/comparison-groups/:id
async fn get_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ComparisonGroupDetail>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let group = crate::db::comparison_groups::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let runs = crate::db::comparison_groups::get_runs(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(ComparisonGroupDetail { group, runs }))
}

/// POST /api/v2/comparison-groups/:id/launch
///
/// Dispatches any still-queued runs in this group to online agents.
async fn launch_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ComparisonGroupDetail>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Verify the group exists before proceeding.
    let _group = crate::db::comparison_groups::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Mark the group as running.
    crate::db::comparison_groups::update_status(&client, &id, "running")
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let runs = crate::db::comparison_groups::get_runs(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Best-effort dispatch each queued run to an online agent.
    for run in &runs {
        if run.status == networker_common::RunStatus::Queued {
            if let Some(agent_id) = state.agents.any_online_agent().await {
                if let Ok(Some(cfg)) =
                    crate::db::test_configs::get(&client, &run.test_config_id).await
                {
                    let msg = networker_common::messages::ControlMessage::AssignRun {
                        run: Box::new(run.clone()),
                        config: Box::new(cfg),
                    };
                    let _ = state.agents.send_to_agent(&agent_id, &msg).await;
                }
            }
        }
    }

    // Re-fetch group with updated status.
    let group = crate::db::comparison_groups::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(ComparisonGroupDetail { group, runs }))
}

/// Project-scoped router (mounted under `/api/v2/projects/{project_id}`).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/comparison-groups",
            post(create_handler).get(list_handler),
        )
        .with_state(state)
}

/// Flat router (mounted under `/api/v2`).
pub fn flat_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/comparison-groups/{id}", get(get_handler))
        .route("/comparison-groups/{id}/launch", post(launch_handler))
        .with_state(state)
}
