use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::auth::ProjectContext;
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Deserialize)]
pub struct ListBenchmarksQuery {
    pub target_host: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CompareBenchmarksRequest {
    pub run_ids: Vec<Uuid>,
    pub baseline_run_id: Option<Uuid>,
}

async fn list_benchmark_presets_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::benchmark_presets::BenchmarkComparePreset>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_benchmark_presets_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let presets = crate::db::benchmark_presets::list(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list benchmark compare presets");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(presets))
}

async fn save_benchmark_preset_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::benchmark_presets::BenchmarkComparePreset>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    let body = axum::body::to_bytes(req.into_body(), 32 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: crate::db::benchmark_presets::BenchmarkComparePresetInput =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in save_benchmark_preset_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    crate::db::benchmark_presets::upsert(&client, &ctx.project_id, &user.user_id, payload)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to save benchmark compare preset");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let presets = crate::db::benchmark_presets::list(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to reload benchmark compare presets");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(presets))
}

async fn delete_benchmark_preset_scoped(
    State(state): State<Arc<AppState>>,
    Path((_, preset_id)): Path<(Uuid, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::benchmark_presets::BenchmarkComparePreset>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_benchmark_preset_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    crate::db::benchmark_presets::delete(&client, &ctx.project_id, &preset_id)
        .await
        .map_err(|e| {
            tracing::error!(preset_id = %preset_id, error = %e, "Failed to delete benchmark compare preset");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let presets = crate::db::benchmark_presets::list(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to reload benchmark compare presets after delete");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(presets))
}

async fn list_benchmarks_scoped(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListBenchmarksQuery>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::benchmarks::BenchmarkRunSummary>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_benchmarks_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let rows = crate::db::benchmarks::list(
        &client,
        &ctx.project_id,
        q.target_host.as_deref(),
        q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        q.offset.unwrap_or(0).max(0),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list benchmark runs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows))
}

async fn get_benchmark_scoped(
    State(state): State<Arc<AppState>>,
    Path((_, run_id)): Path<(Uuid, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<networker_tester::output::json::BenchmarkArtifact>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_benchmark_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let artifact = crate::db::benchmarks::get_artifact(&client, &ctx.project_id, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to load benchmark artifact");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(artifact))
}

async fn compare_benchmarks_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<crate::db::benchmarks::BenchmarkComparisonReport>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let compare_req: CompareBenchmarksRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if compare_req.run_ids.len() < 2 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in compare_benchmarks_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let report = crate::db::benchmarks::compare(
        &client,
        &ctx.project_id,
        &compare_req.run_ids,
        compare_req.baseline_run_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to compare benchmark runs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(report))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmarks", get(list_benchmarks_scoped))
        .route("/benchmarks/compare", post(compare_benchmarks_scoped))
        .route("/benchmarks/:run_id", get(get_benchmark_scoped))
        .route("/benchmarks/presets", get(list_benchmark_presets_scoped))
        .route("/benchmarks/presets", post(save_benchmark_preset_scoped))
        .route(
            "/benchmarks/presets/:preset_id",
            axum::routing::delete(delete_benchmark_preset_scoped),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{ListBenchmarksQuery, DEFAULT_LIMIT, MAX_LIMIT};

    #[test]
    fn benchmark_query_defaults_apply_expected_clamps() {
        let q = ListBenchmarksQuery {
            target_host: None,
            limit: Some(999),
            offset: Some(-10),
        };

        let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = q.offset.unwrap_or(0).max(0);
        assert_eq!(limit, MAX_LIMIT);
        assert_eq!(offset, 0);
    }

    #[test]
    fn benchmark_query_deserializes_empty_payload() {
        let q: ListBenchmarksQuery = serde_json::from_str("{}").unwrap();
        assert!(q.target_host.is_none());
        assert!(q.limit.is_none());
        assert!(q.offset.is_none());
    }
}
