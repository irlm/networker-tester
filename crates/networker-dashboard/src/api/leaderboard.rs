use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::benchmarks::{BenchmarkRunRow, GroupedLeaderboard, LeaderboardEntry, NewResult};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct UploadPayload {
    pub name: String,
    #[serde(default = "default_empty_object")]
    pub config: serde_json::Value,
    pub results: Vec<NewResult>,
}

fn default_empty_object() -> serde_json::Value {
    serde_json::json!({})
}

/// GET /api/leaderboard — latest result per language (public, no auth)
async fn leaderboard(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<LeaderboardEntry>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in leaderboard");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let entries = crate::db::benchmarks::get_latest_leaderboard(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to fetch leaderboard");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(entries))
}

/// GET /api/leaderboard/runs — list all simple benchmark runs (public, no auth)
async fn list_runs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<BenchmarkRunRow>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in leaderboard list_runs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let runs = crate::db::benchmarks::list_runs(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list benchmark runs");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(runs))
}

/// GET /api/leaderboard/runs/:run_id — get run with results (public, no auth)
async fn get_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<BenchmarkRunRow>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in leaderboard get_run");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let run = crate::db::benchmarks::get_run(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to load benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    match run {
        Some(r) => Ok(Json(r)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// POST /api/leaderboard/upload — upload results (auth required, handled by middleware)
async fn upload(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UploadPayload>,
) -> Result<Json<BenchmarkRunRow>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in leaderboard upload");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let run_id = crate::db::benchmarks::create_run(&client, &payload.name, &payload.config)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    for result in &payload.results {
        crate::db::benchmarks::add_result(&client, &run_id, result)
            .await
            .map_err(|e| {
                tracing::error!(run_id = %run_id, error = %e, "Failed to add benchmark result");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    crate::db::benchmarks::finish_run(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to finish benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Reload the completed run with results
    let run = crate::db::benchmarks::get_run(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to reload benchmark run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    match run {
        Some(r) => {
            tracing::info!(
                audit_event = "leaderboard_upload",
                run_id = %run_id,
                name = %payload.name,
                result_count = payload.results.len(),
                "Leaderboard benchmark results uploaded"
            );
            Ok(Json(r))
        }
        None => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Debug, Deserialize)]
pub struct GroupedLeaderboardParams {
    pub group: Option<String>,
}

/// GET /api/leaderboard/grouped — grouped leaderboard by cloud/region/topology (public, no auth)
async fn grouped_leaderboard(
    State(state): State<Arc<AppState>>,
    Query(params): Query<GroupedLeaderboardParams>,
) -> Result<Json<GroupedLeaderboard>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in grouped_leaderboard");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let result = crate::db::benchmarks::get_grouped_leaderboard(
        &client,
        params.group.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to fetch grouped leaderboard");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(result))
}

/// Public leaderboard routes (no auth): GET /leaderboard, GET /leaderboard/runs, GET /leaderboard/runs/:run_id
pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/leaderboard", get(leaderboard))
        .route("/leaderboard/grouped", get(grouped_leaderboard))
        .route("/leaderboard/runs", get(list_runs))
        .route("/leaderboard/runs/:run_id", get(get_run))
        .with_state(state)
}

/// Protected leaderboard routes (auth required): POST /leaderboard/upload
pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/leaderboard/upload", post(upload))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::UploadPayload;

    #[test]
    fn upload_payload_deserializes_minimal() {
        let json = r#"{"name":"test run","results":[]}"#;
        let payload: UploadPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, "test run");
        assert!(payload.results.is_empty());
        assert!(payload.config.is_object());
    }

    #[test]
    fn upload_payload_deserializes_with_results() {
        let json = r#"{
            "name": "v1",
            "config": {"concurrency": 10},
            "results": [
                {"language": "rust", "runtime": "tokio", "metrics": {"latency_mean_ms": 1.23}}
            ]
        }"#;
        let payload: UploadPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.results.len(), 1);
        assert_eq!(payload.results[0].language, "rust");
    }
}
