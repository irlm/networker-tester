use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

use crate::auth::{require_project_role, AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_COMPARE_RUNS: usize = 10;
const COMPARE_BODY_LIMIT_BYTES: usize = 32 * 1024;
const COMPARE_TIMEOUT: Duration = Duration::from_secs(10);

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
    req: Request,
) -> Result<Json<Vec<crate::db::benchmark_presets::BenchmarkComparePreset>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
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
    req: Request,
) -> Result<Json<Vec<crate::db::benchmark_presets::BenchmarkComparePreset>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;
    let body = axum::body::to_bytes(req.into_body(), 32 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: crate::db::benchmark_presets::BenchmarkComparePresetInput =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in save_benchmark_preset_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let preset =
        crate::db::benchmark_presets::upsert(&client, &ctx.project_id, &user.user_id, payload)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to save benchmark compare preset");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    tracing::info!(
        audit_event = "benchmark_preset_saved",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        preset_id = %preset.id,
        preset_name = %preset.name,
        run_count = preset.run_ids.len(),
        baseline_run_id = ?preset.baseline_run_id,
        "Benchmark compare preset saved"
    );

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
    Path((_, preset_id)): Path<(String, Uuid)>,
    req: Request,
) -> Result<Json<Vec<crate::db::benchmark_presets::BenchmarkComparePreset>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;
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
    tracing::info!(
        audit_event = "benchmark_preset_deleted",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        preset_id = %preset_id,
        "Benchmark compare preset deleted"
    );

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
    req: Request,
) -> Result<Json<Vec<crate::db::benchmarks::BenchmarkRunSummary>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
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
    Path((_, run_id)): Path<(String, Uuid)>,
    req: Request,
) -> Result<Json<networker_tester::output::json::BenchmarkArtifact>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_benchmark_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let artifact = crate::db::benchmarks::get_artifact(&client, &ctx.project_id, &run_id)
        .await
        .map_err(|e| {
            tracing::error!(run_id = %run_id, error = %e, "Failed to load benchmark artifact");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let artifact = match artifact {
        Some(artifact) => artifact,
        None => {
            tracing::warn!(
                audit_event = "benchmark_artifact_missing",
                project_id = %ctx.project_id,
                user_id = %user.user_id,
                run_id = %run_id,
                "Requested benchmark artifact was not found"
            );
            return Err(StatusCode::NOT_FOUND);
        }
    };

    tracing::info!(
        audit_event = "benchmark_artifact_read",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        run_id = %run_id,
        "Benchmark artifact loaded"
    );

    Ok(Json(artifact))
}

async fn compare_benchmarks_scoped(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Json<crate::db::benchmarks::BenchmarkComparisonReport>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    let body = axum::body::to_bytes(req.into_body(), COMPARE_BODY_LIMIT_BYTES)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let body_len = body.len();
    let compare_req: CompareBenchmarksRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    validate_compare_request(&compare_req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in compare_benchmarks_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let started_at = Instant::now();

    let report = match tokio::time::timeout(
        COMPARE_TIMEOUT,
        crate::db::benchmarks::compare(
            &client,
            &ctx.project_id,
            &compare_req.run_ids,
            compare_req.baseline_run_id,
        ),
    )
    .await
    {
        Ok(Ok(report)) => report,
        Ok(Err(e)) => {
            tracing::error!(
                audit_event = "benchmark_compare_failed",
                project_id = %ctx.project_id,
                user_id = %user.user_id,
                run_count = compare_req.run_ids.len(),
                baseline_run_id = ?compare_req.baseline_run_id,
                body_bytes = body_len,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                error = %e,
                "Failed to compare benchmark runs"
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(_) => {
            tracing::warn!(
                audit_event = "benchmark_compare_timed_out",
                project_id = %ctx.project_id,
                user_id = %user.user_id,
                run_count = compare_req.run_ids.len(),
                baseline_run_id = ?compare_req.baseline_run_id,
                body_bytes = body_len,
                timeout_ms = COMPARE_TIMEOUT.as_millis() as u64,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                "Benchmark comparison timed out"
            );
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    tracing::info!(
        audit_event = "benchmark_compare_completed",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        run_count = compare_req.run_ids.len(),
        baseline_run_id = ?compare_req.baseline_run_id,
        body_bytes = body_len,
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        case_count = report.cases.len(),
        gated_candidate_count = report.gated_candidate_count,
        "Benchmark comparison generated"
    );

    Ok(Json(report))
}

fn request_extension<T>(req: &Request, name: &'static str) -> Result<T, StatusCode>
where
    T: Clone + Send + Sync + 'static,
{
    req.extensions().get::<T>().cloned().ok_or_else(|| {
        tracing::error!(extension = name, "Missing required request extension");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

fn validate_compare_request(compare_req: &CompareBenchmarksRequest) -> Result<(), StatusCode> {
    if compare_req.run_ids.len() < 2 || compare_req.run_ids.len() > MAX_COMPARE_RUNS {
        return Err(StatusCode::BAD_REQUEST);
    }

    Ok(())
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmarks", get(list_benchmarks_scoped))
        .route("/benchmarks/compare", post(compare_benchmarks_scoped))
        .route("/benchmarks/{run_id}", get(get_benchmark_scoped))
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
    use super::{
        request_extension, validate_compare_request, CompareBenchmarksRequest, ListBenchmarksQuery,
        DEFAULT_LIMIT, MAX_COMPARE_RUNS, MAX_LIMIT,
    };
    use crate::auth::{ProjectContext, ProjectRole};
    use axum::{body::Body, extract::Request, http::StatusCode};
    use uuid::Uuid;

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

    #[test]
    fn compare_request_rejects_too_few_runs() {
        let request = CompareBenchmarksRequest {
            run_ids: vec![Uuid::new_v4()],
            baseline_run_id: None,
        };

        assert_eq!(
            validate_compare_request(&request),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn compare_request_rejects_too_many_runs() {
        let request = CompareBenchmarksRequest {
            run_ids: (0..=MAX_COMPARE_RUNS).map(|_| Uuid::new_v4()).collect(),
            baseline_run_id: None,
        };

        assert_eq!(
            validate_compare_request(&request),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn compare_request_accepts_bounded_selection() {
        let request = CompareBenchmarksRequest {
            run_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            baseline_run_id: None,
        };

        assert_eq!(validate_compare_request(&request), Ok(()));
    }

    #[test]
    fn request_extension_returns_internal_error_when_missing() {
        let req = Request::builder().body(Body::empty()).unwrap();

        assert!(matches!(
            request_extension::<ProjectContext>(&req, "ProjectContext"),
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        ));
    }

    #[test]
    fn request_extension_clones_present_value() {
        let mut req = Request::builder().body(Body::empty()).unwrap();
        let expected = ProjectContext {
            project_id: "test00000000x0".to_string(),
            project_slug: "demo".into(),
            role: ProjectRole::Viewer,
        };
        req.extensions_mut().insert(expected.clone());

        let actual = request_extension::<ProjectContext>(&req, "ProjectContext").unwrap();
        assert_eq!(actual.project_id, expected.project_id);
        assert_eq!(actual.project_slug, expected.project_slug);
        assert_eq!(actual.role, expected.role);
    }
}
