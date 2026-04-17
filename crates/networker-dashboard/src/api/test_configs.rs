//! REST v2: `/api/v2/projects/:pid/test-configs` + `/api/v2/test-configs/:id`.
//!
//! Spec: `.critique/refactor/03-spec.md` §3.1.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;
use networker_common::{EndpointRef, Methodology, TestConfig, Workload};

#[derive(Deserialize)]
pub struct CreateTestConfigRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub endpoint: EndpointRef,
    pub workload: Workload,
    #[serde(default)]
    pub methodology: Option<Methodology>,
    #[serde(default = "default_max_duration")]
    pub max_duration_secs: u32,
}

fn default_max_duration() -> u32 {
    900
}

#[derive(Deserialize, Default)]
pub struct UpdateTestConfigRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub endpoint: Option<EndpointRef>,
    #[serde(default)]
    pub workload: Option<Workload>,
    #[serde(default)]
    pub methodology: Option<Option<Methodology>>,
    #[serde(default)]
    pub baseline_run_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub max_duration_secs: Option<u32>,
}

#[derive(Deserialize, Default)]
pub struct LaunchRequest {
    #[serde(default)]
    pub tester_id: Option<Uuid>,
}

async fn create_handler(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<TestConfig>, StatusCode> {
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
    let payload: CreateTestConfigRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = crate::db::test_configs::create(
        &client,
        &crate::db::test_configs::NewTestConfig {
            project_id: &ctx.project_id,
            name: &payload.name,
            description: payload.description.as_deref(),
            endpoint: &payload.endpoint,
            workload: &payload.workload,
            methodology: payload.methodology.as_ref(),
            max_duration_secs: payload.max_duration_secs,
            created_by: Some(&user.user_id),
        },
    )
    .await
    .map_err(|e| {
        // Surface UNIQUE(project_id, name) violations as 409 so the client
        // can say "this name already exists" instead of a nebulous 500.
        // Postgres error code 23505 == unique_violation.
        if let Some(db) = e.downcast_ref::<tokio_postgres::Error>() {
            if db
                .as_db_error()
                .map(|d| d.code().code())
                .map(|c| c == "23505")
                .unwrap_or(false)
            {
                tracing::warn!(name = %payload.name, "duplicate test_config name");
                return StatusCode::CONFLICT;
            }
        }
        tracing::error!(error = %e, "create test_config failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(row))
}

async fn list_handler(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<TestConfig>>, StatusCode> {
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
    let rows = crate::db::test_configs::list(&client, &ctx.project_id, 200, 0)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows))
}

async fn get_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<TestConfig>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = crate::db::test_configs::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(row))
}

async fn patch_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateTestConfigRequest>,
) -> Result<Json<TestConfig>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let patch = crate::db::test_configs::UpdateTestConfig {
        name: payload.name.as_deref(),
        description: payload.description.as_ref().map(|o| o.as_deref()),
        endpoint: payload.endpoint.as_ref(),
        workload: payload.workload.as_ref(),
        methodology: payload.methodology.as_ref().map(|o| o.as_ref()),
        baseline_run_id: payload.baseline_run_id,
        max_duration_secs: payload.max_duration_secs,
    };
    let row = crate::db::test_configs::update(&client, &id, &patch)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(row))
}

async fn delete_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let deleted = crate::db::test_configs::delete(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// POST /api/v2/test-configs/:id/launch — queue a test_run for this config.
async fn launch_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<LaunchRequest>,
) -> Result<Json<networker_common::TestRun>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let cfg = crate::db::test_configs::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let run = crate::db::test_runs::create(
        &client,
        &crate::db::test_runs::NewTestRun {
            test_config_id: &cfg.id,
            project_id: &cfg.project_id,
            tester_id: payload.tester_id.as_ref(),
            worker_id: None,
            comparison_group_id: None,
        },
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "create test_run failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Dispatch now or kick off provisioning if the endpoint is Pending.
    if let Err(e) = crate::provisioning::dispatch_or_provision(&state, &run, &cfg).await {
        tracing::error!(error = %e, run_id = %run.id, "dispatch_or_provision failed");
    }

    // Re-fetch in case provisioning transitioned the run to `provisioning`.
    let run = match crate::db::test_runs::get(&client, &run.id).await {
        Ok(Some(r)) => r,
        _ => run,
    };
    Ok(Json(run))
}

/// Project-scoped router (mounted under `/api/v2/projects/{project_id}`).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/test-configs", post(create_handler).get(list_handler))
        .with_state(state)
}

/// Flat router (mounted under `/api/v2`).
pub fn flat_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/test-configs/{id}",
            get(get_handler).patch(patch_handler).delete(delete_handler),
        )
        .route("/test-configs/{id}/launch", post(launch_handler))
        .with_state(state)
}
