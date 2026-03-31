use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_role, AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Deserialize)]
pub struct CreateBenchmarkConfigRequest {
    pub name: String,
    pub template: Option<String>,
    pub config_json: serde_json::Value,
    #[serde(default = "default_max_duration")]
    pub max_duration_secs: i32,
    pub baseline_run_id: Option<Uuid>,
    #[serde(default)]
    pub cells: Vec<CellInput>,
}

fn default_max_duration() -> i32 {
    14400
}

#[derive(Debug, Deserialize)]
pub struct CellInput {
    pub cloud: String,
    pub region: String,
    #[serde(default = "default_topology")]
    pub topology: String,
    pub languages: serde_json::Value,
    pub vm_size: Option<String>,
}

fn default_topology() -> String {
    "loopback".to_string()
}

#[derive(Debug, Serialize)]
pub struct CreateBenchmarkConfigResponse {
    pub config_id: Uuid,
    pub cell_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkConfigWithCells {
    #[serde(flatten)]
    pub config: crate::db::benchmark_configs::BenchmarkConfigRow,
    pub cells: Vec<crate::db::benchmark_cells::BenchmarkCellRow>,
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

/// GET /projects/:pid/benchmark-configs
async fn list_configs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
    req: Request,
) -> Result<Json<Vec<crate::db::benchmark_configs::BenchmarkConfigRow>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_benchmark_configs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let configs = crate::db::benchmark_configs::list(
        &client,
        &ctx.project_id,
        q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        q.offset.unwrap_or(0).max(0),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list benchmark configs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(configs))
}

/// POST /projects/:pid/benchmark-configs
async fn create_config(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Json<CreateBenchmarkConfigResponse>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 256 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: CreateBenchmarkConfigRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if payload.name.is_empty() || payload.name.len() > 200 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_benchmark_config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config_id = crate::db::benchmark_configs::create(
        &client,
        &ctx.project_id,
        &payload.name,
        payload.template.as_deref(),
        &payload.config_json,
        Some(&user.user_id),
        payload.max_duration_secs,
        payload.baseline_run_id.as_ref(),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create benchmark config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut cell_ids = Vec::new();
    for cell in &payload.cells {
        let cell_id = crate::db::benchmark_cells::create(
            &client,
            &config_id,
            &cell.cloud,
            &cell.region,
            &cell.topology,
            &cell.languages,
            cell.vm_size.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark cell");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        cell_ids.push(cell_id);
    }

    tracing::info!(
        audit_event = "benchmark_config_created",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        config_id = %config_id,
        name = %payload.name,
        cell_count = cell_ids.len(),
        "Benchmark config created"
    );

    Ok(Json(CreateBenchmarkConfigResponse {
        config_id,
        cell_ids,
    }))
}

/// GET /projects/:pid/benchmark-configs/:id
async fn get_config(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<BenchmarkConfigWithCells>, StatusCode> {
    let _ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_benchmark_config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config = crate::db::benchmark_configs::get(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark config");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let cells = crate::db::benchmark_cells::list_for_config(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list benchmark cells");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(BenchmarkConfigWithCells { config, cells }))
}

/// POST /projects/:pid/benchmark-configs/:id/launch
async fn launch_config(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in launch_benchmark_config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config = crate::db::benchmark_configs::get(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark config for launch");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if config.status != "draft" {
        return Err(StatusCode::CONFLICT);
    }

    crate::db::benchmark_configs::update_status(&client, &config_id, "queued", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to queue benchmark config");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(
        audit_event = "benchmark_config_launched",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        config_id = %config_id,
        "Benchmark config queued for execution"
    );

    Ok(Json(serde_json::json!({"status": "queued"})))
}

/// POST /projects/:pid/benchmark-configs/:id/cancel
async fn cancel_config(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in cancel_benchmark_config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config = crate::db::benchmark_configs::get(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark config for cancel");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    match config.status.as_str() {
        "queued" | "running" | "provisioning" | "deploying" => {}
        _ => return Err(StatusCode::CONFLICT),
    }

    crate::db::benchmark_configs::update_status(&client, &config_id, "cancelled", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to cancel benchmark config");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(
        audit_event = "benchmark_config_cancelled",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        config_id = %config_id,
        "Benchmark config cancelled"
    );

    Ok(Json(serde_json::json!({"status": "cancelled"})))
}

/// Response for the config results endpoint.
#[derive(Debug, Serialize)]
pub struct BenchmarkConfigResults {
    pub config: crate::db::benchmark_configs::BenchmarkConfigRow,
    pub cells: Vec<crate::db::benchmark_cells::BenchmarkCellRow>,
    pub results: Vec<crate::db::benchmarks::ConfigCellResult>,
}

/// GET /projects/:pid/benchmark-configs/:id/results
async fn get_config_results(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<BenchmarkConfigResults>, StatusCode> {
    let _ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_benchmark_config_results");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config = crate::db::benchmark_configs::get(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark config");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let cells = crate::db::benchmark_cells::list_for_config(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list benchmark cells");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let results = crate::db::benchmarks::get_config_results(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark config results");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(BenchmarkConfigResults {
        config,
        cells,
        results,
    }))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmark-configs", get(list_configs).post(create_config))
        .route("/benchmark-configs/:config_id", get(get_config))
        .route("/benchmark-configs/:config_id/launch", post(launch_config))
        .route("/benchmark-configs/:config_id/cancel", post(cancel_config))
        .route(
            "/benchmark-configs/:config_id/results",
            get(get_config_results),
        )
        .with_state(state)
}
