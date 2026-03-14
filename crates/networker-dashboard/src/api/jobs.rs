use std::sync::Arc;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateJobRequest>,
) -> Result<Json<CreateJobResponse>, StatusCode> {
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
    let agent_id = req.agent_id.or_else(|| state.agents.any_online_agent());

    if let Some(aid) = agent_id {
        tracing::info!(correlation_id, agent_id = %aid, "Dispatching job to agent");
        let msg = ControlMessage::JobAssign {
            job_id,
            config: req.config,
        };
        if state.agents.send_to_agent(&aid, &msg).is_ok() {
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
        tracing::warn!(correlation_id, "No online agent available — job queued as pending");
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
        q.limit.unwrap_or(50),
        q.offset.unwrap_or(0),
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
    Path(job_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
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
            let _ = state.agents.send_to_agent(aid, &msg);
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
