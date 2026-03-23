use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_role, AuthUser, ProjectContext, ProjectRole, Role, DEFAULT_PROJECT_ID};
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub cron_expr: String,
    pub config: serde_json::Value,
    pub agent_id: Option<Uuid>,
    pub deployment_id: Option<Uuid>,
    pub auto_start_vm: Option<bool>,
    pub auto_stop_vm: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateScheduleRequest {
    pub name: String,
    pub cron_expr: String,
    pub config: serde_json::Value,
    pub agent_id: Option<Uuid>,
    pub deployment_id: Option<Uuid>,
    pub auto_start_vm: Option<bool>,
    pub auto_stop_vm: Option<bool>,
}

fn compute_next_run(cron_expr: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use std::str::FromStr;
    let schedule = cron::Schedule::from_str(cron_expr).ok()?;
    schedule.upcoming(chrono::Utc).next()
}

async fn create_schedule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateScheduleRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
    // Validate cron expression
    let next_run = compute_next_run(&req.cron_expr).ok_or_else(|| {
        tracing::warn!(cron = %req.cron_expr, "Invalid cron expression");
        StatusCode::BAD_REQUEST
    })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let id = crate::db::schedules::create(
        &client,
        &req.name,
        &req.cron_expr,
        &req.config,
        req.agent_id.as_ref(),
        req.deployment_id.as_ref(),
        req.auto_start_vm.unwrap_or(false),
        req.auto_stop_vm.unwrap_or(false),
        Some(next_run),
        &DEFAULT_PROJECT_ID,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create schedule");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(schedule_id = %id, name = %req.name, "Schedule created");
    Ok(Json(serde_json::json!({
        "schedule_id": id,
        "next_run_at": next_run,
    })))
}

async fn list_schedules(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::db::schedules::ScheduleRow>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let schedules = crate::db::schedules::list(&client, &DEFAULT_PROJECT_ID)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list schedules");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(schedules))
}

async fn get_schedule(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let schedule = crate::db::schedules::get(&client, &schedule_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get schedule");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Also fetch recent jobs for this schedule
    let jobs = crate::db::jobs::list(&client, &DEFAULT_PROJECT_ID, None, 10, 0)
        .await
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "schedule": schedule,
        "recent_jobs": jobs,
    })))
}

async fn update_schedule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(schedule_id): Path<Uuid>,
    Json(req): Json<UpdateScheduleRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
    let next_run = compute_next_run(&req.cron_expr).ok_or_else(|| {
        tracing::warn!(cron = %req.cron_expr, "Invalid cron expression");
        StatusCode::BAD_REQUEST
    })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let updated = crate::db::schedules::update(
        &client,
        &schedule_id,
        &req.name,
        &req.cron_expr,
        &req.config,
        req.agent_id.as_ref(),
        req.deployment_id.as_ref(),
        req.auto_start_vm.unwrap_or(false),
        req.auto_stop_vm.unwrap_or(false),
        Some(next_run),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to update schedule");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !updated {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(schedule_id = %schedule_id, "Schedule updated");
    Ok(Json(
        serde_json::json!({"status": "updated", "next_run_at": next_run}),
    ))
}

async fn delete_schedule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(schedule_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deleted = crate::db::schedules::delete(&client, &schedule_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete schedule");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !deleted {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(schedule_id = %schedule_id, "Schedule deleted");
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn toggle_schedule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(schedule_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let schedule = crate::db::schedules::get(&client, &schedule_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get schedule");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let new_enabled = !schedule.enabled;

    // If re-enabling, recompute next_run_at
    if new_enabled {
        if let Some(next) = compute_next_run(&schedule.cron_expr) {
            crate::db::schedules::mark_run(&client, &schedule_id, Some(next))
                .await
                .ok();
            // Don't update last_run_at — just set next_run_at
            client
                .execute(
                    "UPDATE schedule SET next_run_at = $1 WHERE schedule_id = $2",
                    &[&next, &schedule_id],
                )
                .await
                .ok();
        }
    }

    crate::db::schedules::set_enabled(&client, &schedule_id, new_enabled)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to toggle schedule");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(schedule_id = %schedule_id, enabled = new_enabled, "Schedule toggled");
    Ok(Json(serde_json::json!({"enabled": new_enabled})))
}

async fn trigger_schedule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(schedule_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let schedule = crate::db::schedules::get(&client, &schedule_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get schedule");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let config = schedule.config.ok_or_else(|| {
        tracing::warn!(schedule_id = %schedule_id, "Schedule has no config");
        StatusCode::BAD_REQUEST
    })?;

    // Create a job from the schedule's config
    let project_id = schedule.project_id.unwrap_or(DEFAULT_PROJECT_ID);
    let job_id = crate::db::jobs::create(
        &client,
        &config,
        schedule.agent_id.as_ref(),
        None,
        &project_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create job from schedule trigger");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Try to dispatch
    let agent_id = match schedule.agent_id {
        Some(id) => Some(id),
        None => state.agents.any_online_agent().await,
    };

    if let Some(aid) = agent_id {
        let job_config: networker_common::messages::JobConfig = serde_json::from_value(config)
            .map_err(|e| {
                tracing::error!(error = %e, "Invalid schedule config");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let msg = networker_common::messages::ControlMessage::JobAssign {
            job_id,
            config: job_config,
        };
        if state.agents.send_to_agent(&aid, &msg).await.is_ok() {
            crate::db::jobs::update_status(&client, &job_id, "assigned")
                .await
                .ok();
            let _ = state
                .events_tx
                .send(networker_common::messages::DashboardEvent::JobUpdate {
                    job_id,
                    status: "assigned".into(),
                    agent_id: Some(aid),
                    started_at: None,
                    finished_at: None,
                });
        }
    }

    tracing::info!(schedule_id = %schedule_id, job_id = %job_id, "Schedule triggered manually");
    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "status": "pending",
    })))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route(
            "/schedules/:schedule_id",
            get(get_schedule)
                .put(update_schedule)
                .delete(delete_schedule),
        )
        .route("/schedules/:schedule_id/toggle", post(toggle_schedule))
        .route("/schedules/:schedule_id/trigger", post(trigger_schedule))
        .with_state(state)
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn list_schedules_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::schedules::ScheduleRow>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let schedules = crate::db::schedules::list(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list schedules");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(schedules))
}

async fn create_schedule_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 256)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let create_req: CreateScheduleRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let next_run = compute_next_run(&create_req.cron_expr).ok_or(StatusCode::BAD_REQUEST)?;
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let id = crate::db::schedules::create(
        &client,
        &create_req.name,
        &create_req.cron_expr,
        &create_req.config,
        create_req.agent_id.as_ref(),
        create_req.deployment_id.as_ref(),
        create_req.auto_start_vm.unwrap_or(false),
        create_req.auto_stop_vm.unwrap_or(false),
        Some(next_run),
        &ctx.project_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create schedule");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({
        "schedule_id": id,
        "next_run_at": next_run,
    })))
}

async fn get_schedule_scoped(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let schedule = crate::db::schedules::get(&client, &schedule_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let jobs = crate::db::jobs::list(&client, &ctx.project_id, None, 10, 0)
        .await
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "schedule": schedule,
        "recent_jobs": jobs,
    })))
}

async fn update_schedule_scoped(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 256)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let update_req: UpdateScheduleRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let next_run = compute_next_run(&update_req.cron_expr).ok_or(StatusCode::BAD_REQUEST)?;
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let updated = crate::db::schedules::update(
        &client,
        &schedule_id,
        &update_req.name,
        &update_req.cron_expr,
        &update_req.config,
        update_req.agent_id.as_ref(),
        update_req.deployment_id.as_ref(),
        update_req.auto_start_vm.unwrap_or(false),
        update_req.auto_stop_vm.unwrap_or(false),
        Some(next_run),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !updated {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(
        serde_json::json!({"status": "updated", "next_run_at": next_run}),
    ))
}

async fn delete_schedule_scoped(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let deleted = crate::db::schedules::delete(&client, &schedule_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !deleted {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn toggle_schedule_scoped(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let schedule = crate::db::schedules::get(&client, &schedule_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let new_enabled = !schedule.enabled;
    if new_enabled {
        if let Some(next) = compute_next_run(&schedule.cron_expr) {
            client
                .execute(
                    "UPDATE schedule SET next_run_at = $1 WHERE schedule_id = $2",
                    &[&next, &schedule_id],
                )
                .await
                .ok();
        }
    }

    crate::db::schedules::set_enabled(&client, &schedule_id, new_enabled)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"enabled": new_enabled})))
}

async fn trigger_schedule_scoped(
    State(state): State<Arc<AppState>>,
    Path(schedule_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let schedule = crate::db::schedules::get(&client, &schedule_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let config = schedule.config.ok_or(StatusCode::BAD_REQUEST)?;
    let job_id = crate::db::jobs::create(
        &client,
        &config,
        schedule.agent_id.as_ref(),
        None,
        &ctx.project_id,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let agent_id = match schedule.agent_id {
        Some(id) => Some(id),
        None => state.agents.any_online_agent().await,
    };

    if let Some(aid) = agent_id {
        if let Ok(job_config) =
            serde_json::from_value::<networker_common::messages::JobConfig>(config)
        {
            let msg = networker_common::messages::ControlMessage::JobAssign {
                job_id,
                config: job_config,
            };
            if state.agents.send_to_agent(&aid, &msg).await.is_ok() {
                crate::db::jobs::update_status(&client, &job_id, "assigned")
                    .await
                    .ok();
                let _ =
                    state
                        .events_tx
                        .send(networker_common::messages::DashboardEvent::JobUpdate {
                            job_id,
                            status: "assigned".into(),
                            agent_id: Some(aid),
                            started_at: None,
                            finished_at: None,
                        });
            }
        }
    }

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "status": "pending",
    })))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/schedules",
            get(list_schedules_scoped).post(create_schedule_scoped),
        )
        .route(
            "/schedules/:schedule_id",
            get(get_schedule_scoped)
                .put(update_schedule_scoped)
                .delete(delete_schedule_scoped),
        )
        .route(
            "/schedules/:schedule_id/toggle",
            post(toggle_schedule_scoped),
        )
        .route(
            "/schedules/:schedule_id/trigger",
            post(trigger_schedule_scoped),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::compute_next_run;

    /// Cron parsing: valid expressions produce a future timestamp.
    mod cron_valid {
        use super::*;

        #[test]
        fn every_minute() {
            assert!(compute_next_run("0 * * * * *").is_some());
        }

        #[test]
        fn result_is_in_the_future() {
            let result = compute_next_run("0 * * * * *").expect("should parse");
            assert!(result > chrono::Utc::now());
        }

        #[test]
        fn hourly() {
            assert!(compute_next_run("0 0 * * * *").is_some());
        }

        #[test]
        fn daily_midnight() {
            assert!(compute_next_run("0 0 0 * * *").is_some());
        }

        #[test]
        fn weekly_monday() {
            assert!(compute_next_run("0 0 9 * * Mon").is_some());
        }

        #[test]
        fn specific_time_fields() {
            assert!(compute_next_run("0 15 0 1 * *").is_some());
        }

        #[test]
        fn step_expression() {
            assert!(compute_next_run("0 */5 * * * *").is_some());
        }

        #[test]
        fn range_expression() {
            assert!(compute_next_run("0-30 0 * * * *").is_some());
        }

        #[test]
        fn list_expression() {
            assert!(compute_next_run("0 0 8,12,18 * * *").is_some());
        }

        #[test]
        fn with_year_field() {
            assert!(compute_next_run("0 0 0 1 1 * 2099").is_some());
        }
    }

    /// Cron parsing: invalid or malformed expressions return None.
    mod cron_invalid {
        use super::*;

        #[test]
        fn empty_string() {
            assert!(compute_next_run("").is_none());
        }

        #[test]
        fn five_field_unix_cron_rejected() {
            assert!(compute_next_run("* * * * *").is_none());
        }

        #[test]
        fn garbage_string() {
            assert!(compute_next_run("not a cron expression").is_none());
        }

        #[test]
        fn out_of_range_minute() {
            assert!(compute_next_run("0 60 * * * *").is_none());
        }

        #[test]
        fn out_of_range_hour() {
            assert!(compute_next_run("0 0 25 * * *").is_none());
        }

        #[test]
        fn invalid_dow_name() {
            assert!(compute_next_run("0 0 9 * * Blursday").is_none());
        }

        #[test]
        fn invalid_month_name() {
            assert!(compute_next_run("0 0 0 1 Octember *").is_none());
        }
    }

    /// Temporal properties: next_run_at satisfies the schedule and is deterministic.
    mod cron_temporal {
        use super::*;

        #[test]
        fn result_second_matches_schedule() {
            let result = compute_next_run("0 * * * * *").expect("should parse");
            assert_eq!(result.timestamp() % 60, 0);
        }

        #[test]
        fn two_calls_agree() {
            let a = compute_next_run("0 * * * * *").expect("first");
            let b = compute_next_run("0 * * * * *").expect("second");
            assert!((b - a).num_seconds().abs() <= 60);
        }
    }
}
