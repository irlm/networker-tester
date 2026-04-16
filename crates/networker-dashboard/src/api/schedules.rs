//! REST v2: `/api/v2/projects/:pid/schedules` + `/api/v2/schedules/:id`.
//!
//! Spec: `.critique/refactor/03-spec.md` §3.3.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;
use networker_common::TestSchedule;

#[derive(Deserialize)]
pub struct CreateScheduleRequest {
    pub test_config_id: Uuid,
    pub cron_expr: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_tz() -> String {
    "UTC".into()
}
fn default_enabled() -> bool {
    true
}

#[derive(Deserialize, Default)]
pub struct UpdateScheduleRequest {
    #[serde(default)]
    pub cron_expr: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

async fn create_handler(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<TestSchedule>, StatusCode> {
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

    let body = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: CreateScheduleRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = crate::db::test_schedules::create(
        &client,
        &crate::db::test_schedules::NewTestSchedule {
            test_config_id: &payload.test_config_id,
            project_id: &ctx.project_id,
            cron_expr: &payload.cron_expr,
            timezone: &payload.timezone,
            enabled: payload.enabled,
            created_by: Some(&user.user_id),
        },
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "create test_schedule failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(row))
}

async fn list_handler(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<TestSchedule>>, StatusCode> {
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
    let rows = crate::db::test_schedules::list(&client, &ctx.project_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows))
}

async fn patch_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateScheduleRequest>,
) -> Result<Json<TestSchedule>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let patch = crate::db::test_schedules::UpdateTestSchedule {
        cron_expr: payload.cron_expr.as_deref(),
        timezone: payload.timezone.as_deref(),
        enabled: payload.enabled,
        next_fire_at: None,
    };
    let row = crate::db::test_schedules::update(&client, &id, &patch)
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
    let deleted = crate::db::test_schedules::delete(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Trigger a schedule immediately — queues a test_run from the linked config.
async fn trigger_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<networker_common::TestRun>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sched = crate::db::test_schedules::get(&client, &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let cfg = crate::db::test_configs::get(&client, &sched.test_config_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let run = crate::db::test_runs::create(
        &client,
        &crate::db::test_runs::NewTestRun {
            test_config_id: &cfg.id,
            project_id: &cfg.project_id,
            tester_id: None,
            worker_id: None,
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = crate::db::test_schedules::mark_fired(&client, &id, &run.id, None).await;
    Ok(Json(run))
}

/// Project-scoped router (mounted under `/api/v2/projects/{project_id}`).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/schedules",
            post(create_handler).get(list_handler),
        )
        .with_state(state)
}

/// Flat router (mounted under `/api/v2`).
pub fn flat_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/schedules/{id}",
            axum::routing::patch(patch_handler).delete(delete_handler),
        )
        .route("/schedules/{id}/trigger", post(trigger_handler))
        .with_state(state)
}
