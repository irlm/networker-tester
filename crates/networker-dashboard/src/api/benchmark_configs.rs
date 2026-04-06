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
    #[serde(default)]
    pub config_json: Option<serde_json::Value>,
    #[serde(default = "default_max_duration")]
    pub max_duration_secs: i32,
    pub baseline_run_id: Option<Uuid>,
    #[serde(default)]
    pub testbeds: Vec<TestbedInput>,
    // Frontend sends these as top-level fields; we pack them into config_json
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub methodology: Option<serde_json::Value>,
    #[serde(default)]
    pub auto_teardown: Option<bool>,
    #[serde(default = "default_benchmark_type")]
    pub benchmark_type: String,
}

fn default_max_duration() -> i32 {
    14400
}

fn default_benchmark_type() -> String {
    "fullstack".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TestbedInput {
    pub cloud: String,
    pub region: String,
    #[serde(default = "default_topology")]
    pub topology: String,
    #[serde(default)]
    pub languages: serde_json::Value,
    pub vm_size: Option<String>,
    pub existing_vm_ip: Option<String>,
    pub os: Option<String>,
    #[serde(default)]
    pub proxies: Vec<String>,
    #[serde(default = "default_tester_os")]
    pub tester_os: String,
}

fn default_topology() -> String {
    "loopback".to_string()
}

fn default_tester_os() -> String {
    "server".to_string()
}

#[derive(Debug, Serialize)]
pub struct CreateBenchmarkConfigResponse {
    pub config_id: Uuid,
    pub testbed_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkConfigWithTestbeds {
    #[serde(flatten)]
    pub config: crate::db::benchmark_configs::BenchmarkConfigRow,
    pub testbeds: Vec<crate::db::benchmark_testbeds::BenchmarkTestbedRow>,
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
    let payload: CreateBenchmarkConfigRequest = serde_json::from_slice(&body).map_err(|e| {
        tracing::error!(error = %e, "Failed to parse benchmark config request");
        StatusCode::BAD_REQUEST
    })?;

    if payload.name.is_empty() || payload.name.len() > 200 {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate benchmark_type
    if !["fullstack", "application"].contains(&payload.benchmark_type.as_str()) {
        tracing::warn!(benchmark_type = %payload.benchmark_type, "Invalid benchmark type");
        return Err(StatusCode::BAD_REQUEST);
    }

    // For application mode, each testbed must have at least one proxy
    if payload.benchmark_type == "application" {
        for testbed in &payload.testbeds {
            if testbed.proxies.is_empty() {
                tracing::warn!("Application benchmark testbed missing proxies");
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    // Validate proxy names
    const VALID_PROXIES: &[&str] = &["nginx", "iis", "caddy", "traefik", "haproxy", "apache"];
    for testbed in &payload.testbeds {
        for proxy in &testbed.proxies {
            if !VALID_PROXIES.contains(&proxy.as_str()) {
                tracing::warn!(proxy = %proxy, "Invalid proxy name");
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    // Validate tester_os
    const VALID_TESTER_OS: &[&str] = &["server", "desktop-linux", "desktop-windows"];
    for testbed in &payload.testbeds {
        if !VALID_TESTER_OS.contains(&testbed.tester_os.as_str()) {
            tracing::warn!(tester_os = %testbed.tester_os, "Invalid tester OS");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Validate cloud provider
    const VALID_CLOUDS: &[&str] = &["azure", "aws", "gcp"];
    for testbed in &payload.testbeds {
        if !VALID_CLOUDS.contains(&testbed.cloud.as_str()) {
            tracing::warn!(cloud = %testbed.cloud, "Invalid cloud provider");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Validate region (alphanumeric + hyphens only)
    for testbed in &payload.testbeds {
        if testbed.region.is_empty()
            || !testbed
                .region
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            tracing::warn!(region = %testbed.region, "Invalid region format");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // If config_json is provided directly, validate its testbed values
    // against the same allowlists to prevent validation bypass
    if let Some(ref cj) = payload.config_json {
        if let Some(testbeds) = cj.get("testbeds").and_then(|v| v.as_array()) {
            for tb in testbeds {
                if let Some(proxies) = tb.get("proxies").and_then(|v| v.as_array()) {
                    for p in proxies {
                        if let Some(name) = p.as_str() {
                            if !VALID_PROXIES.contains(&name) {
                                tracing::warn!(proxy = %name, "Invalid proxy in config_json");
                                return Err(StatusCode::BAD_REQUEST);
                            }
                        }
                    }
                }
                if let Some(cloud) = tb.get("cloud").and_then(|v| v.as_str()) {
                    if !VALID_CLOUDS.contains(&cloud) {
                        tracing::warn!(cloud = %cloud, "Invalid cloud in config_json");
                        return Err(StatusCode::BAD_REQUEST);
                    }
                }
                if let Some(langs) = tb.get("languages").and_then(|v| v.as_array()) {
                    for l in langs {
                        if let Some(name) = l.as_str() {
                            if name.is_empty()
                                || !name.chars().all(|c| {
                                    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
                                })
                            {
                                tracing::warn!(language = %name, "Invalid language in config_json");
                                return Err(StatusCode::BAD_REQUEST);
                            }
                        }
                    }
                }
            }
        }
    }

    // Build config_json from top-level fields if not provided directly
    let config_json = payload.config_json.unwrap_or_else(|| {
        serde_json::json!({
            "benchmark_type": payload.benchmark_type,
            "languages": payload.languages,
            "methodology": payload.methodology,
            "auto_teardown": payload.auto_teardown.unwrap_or(true),
            "testbeds": payload.testbeds.iter().map(|t| serde_json::json!({
                "cloud": t.cloud,
                "region": t.region,
                "topology": t.topology,
                "vm_size": t.vm_size,
                "languages": t.languages,
                "existing_vm_ip": t.existing_vm_ip,
                "os": t.os,
                "proxies": t.proxies,
                "tester_os": t.tester_os,
            })).collect::<Vec<_>>(),
        })
    });

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_benchmark_config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let config_id = crate::db::benchmark_configs::create(
        &client,
        &ctx.project_id,
        &payload.name,
        payload.template.as_deref(),
        &config_json,
        Some(&user.user_id),
        payload.max_duration_secs,
        payload.baseline_run_id.as_ref(),
        &payload.benchmark_type,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create benchmark config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut testbed_ids = Vec::new();
    for testbed in &payload.testbeds {
        let proxies_json = serde_json::to_value(&testbed.proxies).unwrap_or_default();
        let testbed_id = crate::db::benchmark_testbeds::create(
            &client,
            &config_id,
            &testbed.cloud,
            &testbed.region,
            &testbed.topology,
            &testbed.languages,
            testbed.vm_size.as_deref(),
            testbed.os.as_deref().unwrap_or("linux"),
            &proxies_json,
            &testbed.tester_os,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark testbed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        testbed_ids.push(testbed_id);
    }

    tracing::info!(
        audit_event = "benchmark_config_created",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        config_id = %config_id,
        name = %payload.name,
        testbed_count = testbed_ids.len(),
        "Benchmark config created"
    );

    Ok(Json(CreateBenchmarkConfigResponse {
        config_id,
        testbed_ids,
    }))
}

/// GET /projects/:pid/benchmark-configs/:id
async fn get_config(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<BenchmarkConfigWithTestbeds>, StatusCode> {
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

    let testbeds = crate::db::benchmark_testbeds::list_for_config(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list benchmark testbeds");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(BenchmarkConfigWithTestbeds { config, testbeds }))
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

    // Block launch if any online tester is running an outdated version
    let dashboard_version = env!("CARGO_PKG_VERSION");
    let agents = crate::db::agents::list(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list agents for version check");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let online_agents: Vec<_> = agents.iter().filter(|a| a.status == "online").collect();
    if online_agents.is_empty() {
        tracing::warn!("No online testers available for benchmark launch");
        return Ok(Json(serde_json::json!({
            "error": "no_testers",
            "message": "No online testers available. Connect a tester before launching."
        })));
    }
    let outdated: Vec<_> = online_agents
        .iter()
        .filter(|a| match &a.version {
            Some(v) => v != dashboard_version,
            None => true, // unknown version treated as outdated
        })
        .collect();
    if !outdated.is_empty() {
        let names: Vec<_> = outdated
            .iter()
            .map(|a| {
                format!(
                    "{} ({})",
                    a.name,
                    a.version.as_deref().unwrap_or("unknown")
                )
            })
            .collect();
        tracing::warn!(
            dashboard_version,
            outdated_testers = ?names,
            "Benchmark launch blocked: tester version mismatch"
        );
        return Ok(Json(serde_json::json!({
            "error": "version_mismatch",
            "message": format!(
                "Cannot launch: testers {} are not on dashboard version {dashboard_version}. Update testers first.",
                names.join(", ")
            )
        })));
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
    pub testbeds: Vec<crate::db::benchmark_testbeds::BenchmarkTestbedRow>,
    pub results: Vec<crate::db::benchmarks::ConfigTestbedResult>,
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

    let testbeds = crate::db::benchmark_testbeds::list_for_config(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list benchmark testbeds");
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
        testbeds,
        results,
    }))
}

/// GET /projects/:pid/benchmark-configs/:id/regressions
async fn get_config_regressions(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<Vec<crate::regression::RegressionRow>>, StatusCode> {
    let _ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_config_regressions");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let regressions = crate::regression::list_for_config(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list config regressions");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(regressions))
}

/// GET /projects/:pid/benchmark-regressions
async fn list_project_regressions(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
    req: Request,
) -> Result<Json<Vec<crate::regression::RegressionWithConfig>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_project_regressions");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let regressions = crate::regression::list_for_project(
        &client,
        &ctx.project_id,
        q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        q.offset.unwrap_or(0).max(0),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to list project regressions");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(regressions))
}

/// GET /projects/:pid/benchmark-configs/:id/progress
async fn get_benchmark_progress(
    State(state): State<Arc<AppState>>,
    Path((_, config_id)): Path<(Uuid, Uuid)>,
    req: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_benchmark_progress");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let progress = crate::db::benchmark_progress::get_progress(&client, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark progress");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"progress": progress})))
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
        .route(
            "/benchmark-configs/:config_id/progress",
            get(get_benchmark_progress),
        )
        .route(
            "/benchmark-configs/:config_id/regressions",
            get(get_config_regressions),
        )
        .route("/benchmark-regressions", get(list_project_regressions))
        .with_state(state)
}
