use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

/// Extract and validate the callback JWT from the Authorization header.
fn extract_callback_token(
    headers: &axum::http::HeaderMap,
    jwt_secret: &str,
) -> Result<crate::auth::Claims, StatusCode> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    crate::auth::validate_token(token, jwt_secret).map_err(|_| StatusCode::UNAUTHORIZED)
}

#[derive(Debug, Deserialize)]
pub struct StatusPayload {
    pub config_id: Uuid,
    pub cell_id: Option<Uuid>,
    pub status: String,
    pub current_language: Option<String>,
    #[allow(dead_code)]
    pub language_index: Option<i32>,
    #[allow(dead_code)]
    pub language_total: Option<i32>,
    #[allow(dead_code)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LogPayload {
    pub config_id: Uuid,
    #[allow(dead_code)]
    pub cell_id: Option<Uuid>,
    pub lines: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResultPayload {
    pub config_id: Uuid,
    #[allow(dead_code)]
    pub cell_id: Option<Uuid>,
    pub language: String,
    pub artifact: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CompletePayload {
    pub config_id: Uuid,
    pub status: String,
    #[allow(dead_code)]
    pub teardown_status: Option<String>,
    pub duration_seconds: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatPayload {
    pub config_id: Uuid,
}

/// POST /api/benchmarks/callback/status
async fn callback_status(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<StatusPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in callback_status");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Update cell status if cell_id provided
    if let Some(cell_id) = &payload.cell_id {
        crate::db::benchmark_cells::update_status(&client, cell_id, &payload.status)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to update cell status");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // Update config status if it's a config-level status (no cell_id)
    if payload.cell_id.is_none() {
        crate::db::benchmark_configs::update_status(
            &client,
            &payload.config_id,
            &payload.status,
            None,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update config status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Broadcast benchmark status to dashboard WebSocket clients
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkUpdate {
            config_id: payload.config_id,
            event_type: "status".into(),
            payload: serde_json::json!({
                "cell_id": payload.cell_id,
                "status": payload.status,
                "current_language": payload.current_language,
                "language_index": payload.language_index,
                "language_total": payload.language_total,
                "message": payload.message,
            }),
        },
    );
    tracing::debug!(
        config_id = %payload.config_id,
        status = %payload.status,
        cell_id = ?payload.cell_id,
        language = ?payload.current_language,
        "Benchmark callback: status"
    );

    Ok(Json(serde_json::json!({"ok": true})))
}

/// POST /api/benchmarks/callback/log
async fn callback_log(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<LogPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    // Broadcast log lines to dashboard WebSocket clients
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkUpdate {
            config_id: payload.config_id,
            event_type: "log".into(),
            payload: serde_json::json!({
                "cell_id": payload.cell_id,
                "lines": payload.lines,
            }),
        },
    );
    tracing::debug!(
        config_id = %payload.config_id,
        line_count = payload.lines.len(),
        "Benchmark callback: log"
    );

    Ok(Json(serde_json::json!({"ok": true})))
}

/// POST /api/benchmarks/callback/result
async fn callback_result(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ResultPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in callback_result");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Get config to find project_id
    let config = crate::db::benchmark_configs::get(&client, &payload.config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get config for result callback");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Deserialize the artifact from the callback JSON
    let artifact: networker_tester::output::json::BenchmarkArtifact =
        serde_json::from_value(payload.artifact.clone()).map_err(|e| {
            tracing::error!(error = %e, "Failed to deserialize BenchmarkArtifact from callback");
            StatusCode::BAD_REQUEST
        })?;

    // Save as a lightweight benchmark_run row (lowercase table)
    let run_name = format!("{} - {}", config.name, payload.language);
    let run_id = crate::db::benchmarks::create_run(&client, &run_name, &payload.artifact)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark run for result");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    crate::db::benchmarks::finish_run(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to finish benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Persist the full artifact into pipeline tables (BenchmarkRun, BenchmarkCase,
    // BenchmarkSample, BenchmarkSummary, etc.) and link to project via job row
    let pipeline_run_id = crate::db::benchmarks::save_artifact(
        &client,
        &config.project_id,
        &payload.config_id,
        payload.cell_id.as_ref(),
        &payload.language,
        &artifact,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to save benchmark artifact to pipeline tables");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Also save a benchmark_result row for the leaderboard
    let metrics = serde_json::json!({
        "mean_ms": artifact.summary.mean,
        "p50_ms": artifact.summary.p50,
        "p95_ms": artifact.summary.p95,
        "p99_ms": artifact.summary.p99,
        "stddev_ms": artifact.summary.stddev,
        "rps": artifact.summary.rps,
        "sample_count": artifact.summary.sample_count,
    });

    let result = crate::db::benchmarks::NewResult {
        language: payload.language.clone(),
        runtime: payload.language.clone(),
        server_os: None,
        client_os: Some(artifact.metadata.client_os.clone()),
        cloud: None,
        phase: Some("measured".to_string()),
        concurrency: Some(artifact.metadata.concurrency as i32),
        metrics,
        started_at: Some(artifact.metadata.generated_at),
        finished_at: Some(chrono::Utc::now()),
    };

    if let Err(e) = crate::db::benchmarks::add_result(&client, &run_id, &result).await {
        tracing::warn!(error = %e, "Failed to add benchmark_result row (non-fatal)");
    }

    // Broadcast result to dashboard WebSocket clients
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkUpdate {
            config_id: payload.config_id,
            event_type: "result".into(),
            payload: serde_json::json!({
                "cell_id": payload.cell_id,
                "language": payload.language,
                "run_id": run_id,
                "pipeline_run_id": pipeline_run_id.to_string(),
                "artifact": payload.artifact,
            }),
        },
    );

    tracing::info!(
        config_id = %payload.config_id,
        language = %payload.language,
        run_id = %run_id,
        pipeline_run_id = %pipeline_run_id,
        "Benchmark callback: result saved to pipeline tables"
    );

    Ok(Json(serde_json::json!({"ok": true, "run_id": run_id})))
}

/// POST /api/benchmarks/callback/complete
async fn callback_complete(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CompletePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in callback_complete");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    crate::db::benchmark_configs::update_status(
        &client,
        &payload.config_id,
        &payload.status,
        payload.error_message.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to update config on complete");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Broadcast complete to dashboard WebSocket clients
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkUpdate {
            config_id: payload.config_id,
            event_type: "complete".into(),
            payload: serde_json::json!({
                "status": payload.status,
                "duration_seconds": payload.duration_seconds,
                "error_message": payload.error_message,
            }),
        },
    );

    tracing::info!(
        config_id = %payload.config_id,
        status = %payload.status,
        duration_seconds = ?payload.duration_seconds,
        "Benchmark callback: complete"
    );

    Ok(Json(serde_json::json!({"ok": true})))
}

/// POST /api/benchmarks/callback/heartbeat
async fn callback_heartbeat(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<HeartbeatPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in callback_heartbeat");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    crate::db::benchmark_configs::update_heartbeat(&client, &payload.config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update heartbeat");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"ok": true})))
}

/// GET /api/benchmarks/callback/cancelled/:config_id
async fn callback_cancelled(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(config_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in callback_cancelled");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config = crate::db::benchmark_configs::get(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get config for cancellation check");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let cancelled = config.status == "cancelled";

    Ok(Json(serde_json::json!({"cancelled": cancelled})))
}

/// Public callback routes -- JWT verified internally per handler.
pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmarks/callback/status", post(callback_status))
        .route("/benchmarks/callback/log", post(callback_log))
        .route("/benchmarks/callback/result", post(callback_result))
        .route("/benchmarks/callback/complete", post(callback_complete))
        .route("/benchmarks/callback/heartbeat", post(callback_heartbeat))
        .route(
            "/benchmarks/callback/cancelled/:config_id",
            get(callback_cancelled),
        )
        .with_state(state)
}
