use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Deserialize)]
pub struct ListRunsQuery {
    pub target_host: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListRunsQuery>,
) -> Result<Json<Vec<crate::db::runs::RunSummary>>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "DB pool error in list_runs");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let runs = crate::db::runs::list(
        &client,
        q.target_host.as_deref(),
        q.limit.unwrap_or(50),
        q.offset.unwrap_or(0),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list runs from DB");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(runs))
}

async fn get_run_attempts(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "DB pool error in get_run_attempts");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let attempts = crate::db::runs::get_attempts(&client, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to load run attempts");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(attempts))
}

async fn get_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let row = client
        .query_opt(
            "SELECT RunId, StartedAt, FinishedAt, TargetUrl, TargetHost, Modes,
                    TotalRuns, SuccessCount, FailureCount, ClientOs, ClientVersion
             FROM TestRun WHERE RunId = $1",
            &[&run_id],
        )
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to load run");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let target_url: String = row.get("targeturl");

    // Try to get endpoint version from target
    let endpoint_version = fetch_endpoint_version(&target_url).await;

    Ok(Json(serde_json::json!({
        "run_id": row.get::<_, Uuid>("runid").to_string(),
        "started_at": row.get::<_, chrono::DateTime<chrono::Utc>>("startedat").to_rfc3339(),
        "finished_at": row.get::<_, Option<chrono::DateTime<chrono::Utc>>>("finishedat").map(|d| d.to_rfc3339()),
        "target_url": target_url,
        "target_host": row.get::<_, String>("targethost"),
        "modes": row.get::<_, String>("modes"),
        "total_runs": row.get::<_, i32>("totalruns"),
        "success_count": row.get::<_, i32>("successcount"),
        "failure_count": row.get::<_, i32>("failurecount"),
        "client_os": row.get::<_, String>("clientos"),
        "client_version": row.get::<_, String>("clientversion"),
        "endpoint_version": endpoint_version,
    })))
}

async fn fetch_endpoint_version(target_url: &str) -> Option<String> {
    let base = url::Url::parse(target_url).ok()?;
    let host = base.host_str()?;
    let port = base.port_or_known_default()?;
    let health_url = format!("https://{host}:{port}/health");
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let resp = client.get(&health_url).send().await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/runs", get(list_runs))
        .route("/runs/:run_id", get(get_run))
        .route("/runs/:run_id/attempts", get(get_run_attempts))
        .with_state(state)
}
