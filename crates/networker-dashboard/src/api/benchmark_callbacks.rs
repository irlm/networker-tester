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
    pub testbed_id: Option<Uuid>,
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
    pub testbed_id: Option<Uuid>,
    pub lines: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResultPayload {
    pub config_id: Uuid,
    #[allow(dead_code)]
    pub testbed_id: Option<Uuid>,
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

    // Update testbed status if testbed_id provided
    if let Some(testbed_id) = &payload.testbed_id {
        crate::db::benchmark_testbeds::update_status(&client, testbed_id, &payload.status)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to update testbed status");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // Update config status if it's a config-level status (no testbed_id)
    if payload.testbed_id.is_none() {
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
                "testbed_id": payload.testbed_id,
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
        testbed_id = ?payload.testbed_id,
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

    // Persist log lines to logs database
    let client = state.logs_db.get().await.map_err(|e| {
        tracing::error!(error = %e, "Logs DB pool error in callback_log");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if let Err(e) = crate::db::benchmark_log::insert_batch(
        &client,
        &payload.config_id,
        payload.testbed_id.as_ref(),
        &payload.lines,
    )
    .await
    {
        tracing::error!(error = %e, "Failed to persist benchmark log lines");
    }

    // Broadcast log lines to dashboard WebSocket clients
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkUpdate {
            config_id: payload.config_id,
            event_type: "log".into(),
            payload: serde_json::json!({
                "testbed_id": payload.testbed_id,
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

    // Try to deserialize as the full BenchmarkArtifact (--benchmark-mode output).
    // Fall back to the legacy flat format (--json-stdout without --benchmark-mode).
    let artifact_opt: Option<networker_tester::output::json::BenchmarkArtifact> =
        serde_json::from_value(payload.artifact.clone()).ok();

    let run_name = format!("{} - {}", config.name, payload.language);

    let (run_id, pipeline_run_id) = if let Some(ref artifact) = artifact_opt {
        // Full BenchmarkArtifact: save to pipeline tables with proper run_id linkage
        let artifact_run_id = artifact.metadata.run_id;
        let rid = crate::db::benchmarks::create_run_linked(
            &client,
            &artifact_run_id,
            &run_name,
            &payload.artifact,
            &payload.config_id,
            payload.testbed_id.as_ref(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark run for result");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        crate::db::benchmarks::finish_run(&client, &rid).await.ok();

        let pid = crate::db::benchmarks::save_artifact(
            &client,
            &config.project_id,
            &payload.config_id,
            payload.testbed_id.as_ref(),
            &payload.language,
            artifact,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to save benchmark artifact to pipeline tables");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Leaderboard row
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
        if let Err(e) = crate::db::benchmarks::add_result(&client, &rid, &result).await {
            tracing::warn!(error = %e, "Failed to add benchmark_result row (non-fatal)");
        }

        (rid, pid)
    } else {
        // Legacy flat format: save as lightweight benchmark_run only (no pipeline tables)
        tracing::info!(
            config_id = %payload.config_id,
            language = %payload.language,
            "Result callback received legacy format — saving as lightweight run"
        );
        let rid = crate::db::benchmarks::create_run(&client, &run_name, &payload.artifact)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create benchmark run for legacy result");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        // Link to config/testbed
        client.execute(
            "UPDATE benchmark_run SET config_id = $1, testbed_id = $2, status = 'completed', finished_at = now() WHERE run_id = $3",
            &[&payload.config_id, &payload.testbed_id, &rid],
        ).await.ok();

        (rid, rid)
    };

    // Broadcast result to dashboard WebSocket clients
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkUpdate {
            config_id: payload.config_id,
            event_type: "result".into(),
            payload: serde_json::json!({
                "testbed_id": payload.testbed_id,
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
        artifact_format = if artifact_opt.is_some() { "full" } else { "legacy" },
        "Benchmark callback: result saved"
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

    // Run regression detection on successful completion
    if payload.status == "completed" {
        let state_clone = state.clone();
        let config_id = payload.config_id;
        tokio::spawn(async move {
            if let Err(e) = run_regression_detection(&state_clone, &config_id).await {
                tracing::error!(
                    error = %e,
                    config_id = %config_id,
                    "Regression detection failed"
                );
            }
        });
    }

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

/// Run regression detection after a benchmark completes, notify via WS and email.
async fn run_regression_detection(state: &Arc<AppState>, config_id: &Uuid) -> anyhow::Result<()> {
    let client = state.db.get().await?;

    let regressions = crate::regression::detect(&client, config_id, None, None).await?;

    if regressions.is_empty() {
        tracing::info!(config_id = %config_id, "No regressions detected");
        return Ok(());
    }

    // Load config name for notifications
    let config = crate::db::benchmark_configs::get(&client, config_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Config not found: {config_id}"))?;

    // Broadcast regression event to dashboard WebSocket clients
    let regressions_json = serde_json::to_value(&regressions)?;
    let _ = state.events_tx.send(
        networker_common::messages::DashboardEvent::BenchmarkRegression {
            config_id: *config_id,
            config_name: config.name.clone(),
            regression_count: regressions.len(),
            regressions: regressions_json,
        },
    );

    // Send email notifications to project members
    let members = crate::db::projects::list_members(&client, &config.project_id).await?;
    let emails: Vec<String> = members.iter().map(|m| m.email.clone()).collect();

    crate::regression::notify_regressions(config_id, &config.name, &regressions, &emails).await;

    tracing::info!(
        config_id = %config_id,
        regression_count = regressions.len(),
        "Regression detection complete — notifications sent"
    );

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct RequestProgressPayload {
    pub config_id: Uuid,
    pub testbed_id: Option<Uuid>,
    pub language: String,
    pub mode: String,
    pub request_index: i32,
    pub total_requests: i32,
    pub latency_ms: f64,
    pub success: bool,
}

/// POST /api/benchmarks/callback/request-progress
async fn callback_request_progress(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<RequestProgressPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let client = state.logs_db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in callback_request_progress");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    crate::db::benchmark_progress::insert_single(
        &client,
        &payload.config_id,
        payload.testbed_id.as_ref(),
        &payload.language,
        &payload.mode,
        payload.request_index,
        payload.total_requests,
        payload.latency_ms,
        payload.success,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to insert request progress");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Throttle WS broadcasts to every 10th request
    if payload.request_index % 10 == 0 {
        let _ = state.events_tx.send(
            networker_common::messages::DashboardEvent::BenchmarkUpdate {
                config_id: payload.config_id,
                event_type: "request_progress".into(),
                payload: serde_json::json!({
                    "testbed_id": payload.testbed_id,
                    "language": payload.language,
                    "mode": payload.mode,
                    "request_index": payload.request_index,
                    "total_requests": payload.total_requests,
                    "latency_ms": payload.latency_ms,
                    "success": payload.success,
                }),
            },
        );
    }

    Ok(Json(serde_json::json!({"ok": true})))
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
            "/benchmarks/callback/request-progress",
            post(callback_request_progress),
        )
        .route(
            "/benchmarks/callback/cancelled/:config_id",
            get(callback_cancelled),
        )
        .with_state(state)
}
