use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::benchmarks;
use crate::AppState;

// ── Public handlers (no auth) ────────────────────────────────────────────

async fn list_runs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<benchmarks::BenchmarkRunRow>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_benchmark_runs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let runs = benchmarks::list_runs(&client).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list benchmark runs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(runs))
}

async fn get_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<benchmarks::BenchmarkRunRow>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_benchmark_run");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let run = benchmarks::get_run(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to get benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(run))
}

async fn get_latest_leaderboard(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<benchmarks::LeaderboardEntry>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_latest_leaderboard");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let entries = benchmarks::get_latest_leaderboard(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark leaderboard");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(entries))
}

// ── Protected handler (auth required) ────────────────────────────────────

/// Accepts the full JSON output from the `alethabench` CLI.
/// Creates a benchmark_run and inserts all results, then marks it completed.
#[derive(Debug, Deserialize)]
struct UploadPayload {
    name: String,
    #[serde(default = "default_empty_object")]
    config: serde_json::Value,
    #[serde(default)]
    results: Vec<benchmarks::NewResult>,
}

fn default_empty_object() -> serde_json::Value {
    serde_json::json!({})
}

async fn upload_results(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UploadPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in upload_benchmark_results");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let run_id = benchmarks::create_run(&client, &payload.name, &payload.config)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    for result in &payload.results {
        benchmarks::add_result(&client, &run_id, result)
            .await
            .map_err(|e| {
                tracing::error!(run_id = %run_id, error = %e, "Failed to add benchmark result");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    benchmarks::finish_run(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to finish benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(run_id = %run_id, results = payload.results.len(), "Benchmark results uploaded");

    Ok(Json(serde_json::json!({ "run_id": run_id.to_string() })))
}

// ── Routers ──────────────────────────────────────────────────────────────

/// Public routes (no auth required for reads).
pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmarks", get(list_runs))
        .route(
            "/benchmarks/latest/leaderboard",
            get(get_latest_leaderboard),
        )
        .route("/benchmarks/:run_id", get(get_run))
        .with_state(state)
}

/// Protected routes (auth required for writes).
pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmarks", post(upload_results))
        .with_state(state)
}
