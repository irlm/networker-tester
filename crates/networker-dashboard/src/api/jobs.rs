use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_role, AuthUser, Role};
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

async fn create_job(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateJobRequest>,
) -> Result<Json<CreateJobResponse>, StatusCode> {
    require_role(&user, Role::Operator)?;
    // Validate job config limits
    if req.config.runs > MAX_RUNS {
        tracing::warn!(runs = req.config.runs, "Rejecting job: runs exceeds limit");
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.config.timeout_secs > MAX_TIMEOUT_SECS {
        tracing::warn!(
            timeout = req.config.timeout_secs,
            "Rejecting job: timeout exceeds limit"
        );
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.config.concurrency > MAX_CONCURRENCY {
        tracing::warn!(
            concurrency = req.config.concurrency,
            "Rejecting job: concurrency exceeds limit"
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    let config_json = serde_json::to_value(&req.config).map_err(|e| {
        tracing::error!(error = %e, "Failed to serialize job config");
        StatusCode::BAD_REQUEST
    })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_job");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let job_id = crate::db::jobs::create(&client, &config_json, req.agent_id.as_ref(), None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to insert job into DB");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let correlation_id = job_id.to_string();
    tracing::info!(correlation_id, target = %req.config.target, modes = ?req.config.modes, "Job created");

    // Try to dispatch to a connected agent
    let agent_id = match req.agent_id {
        Some(id) => Some(id),
        None => state.agents.any_online_agent().await,
    };

    if let Some(aid) = agent_id {
        tracing::info!(correlation_id, agent_id = %aid, "Dispatching job to agent");
        let msg = ControlMessage::JobAssign {
            job_id,
            config: req.config,
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
            tracing::info!(correlation_id, "Job dispatched successfully");
        } else {
            tracing::warn!(correlation_id, agent_id = %aid, "Failed to send job to agent");
        }
    } else {
        tracing::warn!(
            correlation_id,
            "No online agent available — job queued as pending"
        );
    }

    Ok(Json(CreateJobResponse {
        job_id,
        status: "pending".into(),
    }))
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListJobsQuery>,
) -> Result<Json<Vec<crate::db::jobs::JobRow>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_jobs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let jobs = crate::db::jobs::list(
        &client,
        q.status.as_deref(),
        q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        q.offset.unwrap_or(0).max(0),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list jobs from DB");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(jobs))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
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

async fn cancel_job(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
    let correlation_id = job_id.to_string();
    tracing::info!(correlation_id, "Cancel request received");

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(correlation_id, error = %e, "DB pool error in cancel_job");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let job = crate::db::jobs::get(&client, &job_id)
        .await
        .map_err(|e| {
            tracing::error!(correlation_id, error = %e, "Failed to get job for cancel");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if job.status == "running" || job.status == "assigned" || job.status == "pending" {
        if let Some(aid) = &job.agent_id {
            tracing::info!(correlation_id, agent_id = %aid, "Sending cancel to agent");
            let msg = ControlMessage::JobCancel { job_id };
            let _ = state.agents.send_to_agent(aid, &msg).await;
        }
        crate::db::jobs::update_status(&client, &job_id, "cancelled")
            .await
            .map_err(|e| {
                tracing::error!(correlation_id, error = %e, "Failed to update job status to cancelled");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        let _ = state.events_tx.send(DashboardEvent::JobUpdate {
            job_id,
            status: "cancelled".into(),
            agent_id: job.agent_id,
            started_at: job.started_at,
            finished_at: Some(chrono::Utc::now()),
        });
        tracing::info!(correlation_id, "Job cancelled");
    }

    Ok(Json(serde_json::json!({"status": "cancelled"})))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/:job_id", get(get_job))
        .route("/jobs/:job_id/cancel", post(cancel_job))
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
