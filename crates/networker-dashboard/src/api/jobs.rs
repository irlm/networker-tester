use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;
use networker_common::messages::{ControlMessage, DashboardEvent, JobConfig};

#[derive(Deserialize)]
pub struct CreateJobRequest {
    pub config: JobConfig,
    pub agent_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct CreateJobResponse {
    pub job_id: Uuid,
    pub status: String,
}

#[derive(Deserialize)]
pub struct ListJobsQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Limits to prevent runaway jobs.
const MAX_RUNS: u32 = 1000;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_TIMEOUT_SECS: u64 = 300;
const MAX_CONCURRENCY: usize = 16;

async fn get_job(
    State(state): State<Arc<AppState>>,
    Path((_, job_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let correlation_id = job_id.to_string();
    tracing::debug!(correlation_id, "Fetching job details");

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(correlation_id, error = %e, "DB pool error in get_job");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let job = crate::db::jobs::get(&client, &job_id)
        .await
        .map_err(|e| {
            tracing::error!(correlation_id, error = %e, "Failed to get job from DB");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::warn!(correlation_id, "Job not found");
            StatusCode::NOT_FOUND
        })?;

    Ok(Json(serde_json::to_value(job).unwrap_or_default()))
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn list_jobs_scoped(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListJobsQuery>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::jobs::JobRow>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_jobs_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Apply visibility filtering for viewers only
    let visible_ids = if ctx.role == ProjectRole::Viewer {
        crate::db::visibility::visible_resources(&client, &ctx.project_id, &user.user_id, "job")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to check visibility rules");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    } else {
        None
    };

    let jobs = crate::db::jobs::list_filtered(
        &client,
        &ctx.project_id,
        q.status.as_deref(),
        q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        q.offset.unwrap_or(0).max(0),
        visible_ids.as_ref(),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list jobs from DB");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(jobs))
}

async fn create_job_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<CreateJobResponse>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 256)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let create_req: CreateJobRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if create_req.config.runs > MAX_RUNS {
        return Err(StatusCode::BAD_REQUEST);
    }
    if create_req.config.timeout_secs > MAX_TIMEOUT_SECS {
        return Err(StatusCode::BAD_REQUEST);
    }
    if create_req.config.concurrency > MAX_CONCURRENCY {
        return Err(StatusCode::BAD_REQUEST);
    }

    let config_json =
        serde_json::to_value(&create_req.config).map_err(|_| StatusCode::BAD_REQUEST)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_job_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let job_id = crate::db::jobs::create(
        &client,
        &config_json,
        create_req.agent_id.as_ref(),
        Some(&user.user_id),
        &ctx.project_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to insert job into DB");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let agent_id = match create_req.agent_id {
        Some(id) => Some(id),
        None => state.agents.any_online_agent().await,
    };

    if let Some(aid) = agent_id {
        let msg = ControlMessage::JobAssign {
            job_id,
            config: create_req.config,
        };
        if state.agents.send_to_agent(&aid, &msg).await.is_ok() {
            crate::db::jobs::update_status(&client, &job_id, "assigned")
                .await
                .ok();
            let _ = state.events_tx.send(DashboardEvent::JobUpdate {
                job_id,
                status: "assigned".into(),
                agent_id: Some(aid),
                started_at: None,
                finished_at: None,
            });
        }
    }

    Ok(Json(CreateJobResponse {
        job_id,
        status: "pending".into(),
    }))
}

async fn cancel_job_scoped(
    State(state): State<Arc<AppState>>,
    Path((_, job_id)): Path<(Uuid, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let job = crate::db::jobs::get(&client, &job_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if job.status == "running" || job.status == "assigned" || job.status == "pending" {
        if let Some(aid) = &job.agent_id {
            let msg = ControlMessage::JobCancel { job_id };
            let _ = state.agents.send_to_agent(aid, &msg).await;
        }
        crate::db::jobs::update_status(&client, &job_id, "cancelled")
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let _ = state.events_tx.send(DashboardEvent::JobUpdate {
            job_id,
            status: "cancelled".into(),
            agent_id: job.agent_id,
            started_at: job.started_at,
            finished_at: Some(chrono::Utc::now()),
        });
    }

    Ok(Json(serde_json::json!({"status": "cancelled"})))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/jobs", get(list_jobs_scoped).post(create_job_scoped))
        .route("/jobs/:job_id", get(get_job))
        .route("/jobs/:job_id/cancel", post(cancel_job_scoped))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENCY, MAX_RUNS, MAX_TIMEOUT_SECS};

    /// Compile-time bounds on safety limits.
    const _: () = {
        assert!(MAX_RUNS >= 1 && MAX_RUNS <= 10_000);
        assert!(MAX_TIMEOUT_SECS >= 1 && MAX_TIMEOUT_SECS <= 600);
        assert!(MAX_CONCURRENCY >= 1 && MAX_CONCURRENCY <= 64);
    };

    /// Verify the safety limits are set to expected values.
    mod limits {
        use super::*;

        #[test]
        fn specific_limits() {
            // Document the current values so changes are intentional
            assert_eq!(MAX_RUNS, 1000);
            assert_eq!(MAX_TIMEOUT_SECS, 300);
            assert_eq!(MAX_CONCURRENCY, 16);
        }
    }

    /// ListJobsQuery default handling.
    mod query_defaults {
        use super::super::{ListJobsQuery, DEFAULT_LIMIT, MAX_LIMIT};

        #[test]
        fn defaults_are_none() {
            let json = "{}";
            let q: ListJobsQuery = serde_json::from_str(json).unwrap();
            assert!(q.status.is_none());
            assert!(q.limit.is_none());
            assert!(q.offset.is_none());
        }

        #[test]
        fn parses_all_fields() {
            let json = r#"{"status":"running","limit":10,"offset":5}"#;
            let q: ListJobsQuery = serde_json::from_str(json).unwrap();
            assert_eq!(q.status.as_deref(), Some("running"));
            assert_eq!(q.limit, Some(10));
            assert_eq!(q.offset, Some(5));
        }

        #[test]
        fn clamp_limit_caps_at_max() {
            let q = ListJobsQuery {
                status: None,
                limit: Some(9999),
                offset: Some(0),
            };
            let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
            assert_eq!(limit, MAX_LIMIT);
        }

        #[test]
        fn clamp_negative_limit_becomes_one() {
            let q = ListJobsQuery {
                status: None,
                limit: Some(-5),
                offset: None,
            };
            let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
            assert_eq!(limit, 1);
        }

        #[test]
        fn clamp_negative_offset_becomes_zero() {
            let q = ListJobsQuery {
                status: None,
                limit: None,
                offset: Some(-10),
            };
            let offset = q.offset.unwrap_or(0).max(0);
            assert_eq!(offset, 0);
        }
    }
}
