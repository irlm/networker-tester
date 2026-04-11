//! Persistent tester REST endpoints (Tasks 14 + 15).
//!
//! Wiring into the main router happens in Task 18.
//!
//! Routes (project-scoped, nested under `/projects/{project_id}`):
//!
//!   GET    /testers                         — list (Viewer)
//!   GET    /testers/regions                 — region list (Viewer)
//!   GET    /testers/{tid}                   — inspect (Viewer)
//!   GET    /testers/{tid}/queue             — running + queued (Viewer)
//!   GET    /testers/{tid}/cost_estimate     — monthly $ estimate (Viewer)
//!   POST   /testers                         — create + provision (Operator)
//!   POST   /testers/{tid}/start             — deallocated → running (Operator)
//!   POST   /testers/{tid}/stop              — running → stopped (Operator)
//!   POST   /testers/{tid}/upgrade           — re-run installer (Admin)
//!   DELETE /testers/{tid}                   — destroy VM + row (Admin)
//!
//! Mutating endpoints return 202 Accepted with the current row and spawn
//! a background `tokio::task` that drives the Azure CLI + updates the
//! row's `power_state` / `allocation` / `status_message`.
//!
//! TODO (Task 17): replace the `tracing::info!` audit stubs with real
//! `audit_tester_action` calls once that helper lands.
#![allow(dead_code)]

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::db::project_testers::{CreateTesterInput, ProjectTesterRow};
use crate::AppState;
use networker_dashboard::services::{azure_vm, tester_install, tester_state};

/// Fallback region list when no Azure cloud account is registered for the
/// project, or when the account has no `region_default` set. The
/// `cloud_account` table does not currently store a `regions` array column,
/// so we fall back to this hard-coded list of common Azure regions.
// TODO: replace with a real per-account region catalog once the schema grows
// a `regions JSONB` column (or once we fetch from Azure Resource Manager).
const FALLBACK_AZURE_REGIONS: &[&str] = &[
    "eastus",
    "westus2",
    "japaneast",
    "uksouth",
    "westeurope",
    "southeastasia",
    "australiaeast",
];

// ── Pure helpers (unit-testable without DB) ───────────────────────────────

/// Hardcoded hourly USD cost lookup for supported VM sizes. Unknown sizes
/// fall back to the Standard_D2s_v3 rate (conservative low-end default).
fn hourly_usd(vm_size: &str) -> f64 {
    match vm_size {
        "Standard_D2s_v3" => 0.096,
        "Standard_D4s_v3" => 0.192,
        "Standard_D8s_v3" => 0.384,
        _ => 0.096,
    }
}

/// Estimate monthly cost in USD.
///
/// Returns `(always_on, with_schedule)`:
///   * `always_on` — 24h × 30d × hourly_usd
///   * `with_schedule` — 15h × 30d × hourly_usd if auto-shutdown is
///     enabled, otherwise equals `always_on`.
///
/// The 15-hour figure assumes a business-day schedule (roughly 8am–11pm
/// local); this is an MVP approximation, not an exact calendar computation.
fn cost_estimate(vm_size: &str, auto_shutdown_enabled: bool) -> (f64, f64) {
    let hourly = hourly_usd(vm_size);
    let always_on = 24.0 * 30.0 * hourly;
    let with_schedule = if auto_shutdown_enabled {
        15.0 * 30.0 * hourly
    } else {
        always_on
    };
    (always_on, with_schedule)
}

// ── Response shapes ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct RegionsResponse {
    regions: Vec<String>,
}

#[derive(Serialize)]
struct QueueEntry {
    config_id: Uuid,
    name: String,
    queued_at: Option<DateTime<Utc>>,
    position: i32,
    eta: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct RunningEntry {
    config_id: Uuid,
    name: String,
    started_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct QueueResponse {
    tester_id: Uuid,
    running: Option<RunningEntry>,
    queued: Vec<QueueEntry>,
}

#[derive(Serialize)]
struct CostEstimateResponse {
    vm_size: String,
    hourly_usd: f64,
    monthly_always_on_usd: f64,
    monthly_with_schedule_usd: f64,
    auto_shutdown_enabled: bool,
}

// ── Handlers ──────────────────────────────────────────────────────────────

fn db_error(stage: &'static str, err: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(error = %err, stage = stage, "DB error in testers handler");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Database error".to_string(),
    )
}

async fn list_testers(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<ProjectTesterRow>>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("list_testers pool", e))?;
    let rows = crate::db::project_testers::list_for_project(&client, &ctx.project_id)
        .await
        .map_err(|e| db_error("list_testers query", e))?;
    Ok(Json(rows))
}

async fn list_regions(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<RegionsResponse>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("list_regions pool", e))?;

    // The `cloud_account` table does not store a regions array. If the
    // project has an Azure account with a `region_default`, surface that as
    // the first entry; otherwise return the fallback list unchanged.
    //
    // TODO: once cloud_account learns a `regions JSONB` column (or we
    // query Azure Resource Manager directly), replace this with a real fetch.
    let row = client
        .query_opt(
            "SELECT region_default FROM cloud_account \
             WHERE project_id = $1 AND provider = 'azure' AND region_default IS NOT NULL \
             ORDER BY created_at ASC LIMIT 1",
            &[&ctx.project_id],
        )
        .await
        .map_err(|e| db_error("list_regions query", e))?;

    let mut regions: Vec<String> = FALLBACK_AZURE_REGIONS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    if let Some(row) = row {
        let default: Option<String> = row.get("region_default");
        if let Some(d) = default {
            if !regions.iter().any(|r| r == &d) {
                regions.insert(0, d);
            }
        }
    }

    Ok(Json(RegionsResponse { regions }))
}

async fn get_tester(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<ProjectTesterRow>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("get_tester pool", e))?;
    let row = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("get_tester query", e))?;

    // 404 even for platform admins if the tester is not in this project —
    // the project-scoped middleware handles auth at the /projects/{pid}
    // level; this scoping ensures no cross-project leakage.
    match row {
        Some(tester) => Ok(Json(tester)),
        None => Err((StatusCode::NOT_FOUND, "Tester not found".to_string())),
    }
}

async fn get_queue(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<QueueResponse>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("get_queue pool", e))?;

    // Confirm the tester belongs to this project (404 otherwise).
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("get_queue tester lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".to_string()))?;

    // Pull running + queued benchmark_config rows bound to this tester.
    let rows = client
        .query(
            "SELECT config_id, name, status, queued_at, started_at \
             FROM benchmark_config \
             WHERE tester_id = $1 AND status IN ('running', 'queued') \
             ORDER BY \
               CASE status WHEN 'running' THEN 0 ELSE 1 END, \
               queued_at ASC NULLS LAST, \
               created_at ASC",
            &[&tester_id],
        )
        .await
        .map_err(|e| db_error("get_queue select", e))?;

    let mut running: Option<RunningEntry> = None;
    let mut queued: Vec<QueueEntry> = Vec::new();

    for row in rows.iter() {
        let status: String = row.get("status");
        let config_id: Uuid = row.get("config_id");
        let name: String = row.get("name");
        if status == "running" && running.is_none() {
            let started_at: Option<DateTime<Utc>> = row.get("started_at");
            running = Some(RunningEntry {
                config_id,
                name,
                started_at,
            });
        } else if status == "queued" {
            let queued_at: Option<DateTime<Utc>> = row.get("queued_at");
            queued.push(QueueEntry {
                config_id,
                name,
                queued_at,
                position: 0, // filled in below
                eta: None,   // filled in below
            });
        }
    }

    // Assign positions + ETAs using the tester's rolling average duration.
    let avg_secs = tester.avg_benchmark_duration_seconds;
    let now = Utc::now();
    for (idx, entry) in queued.iter_mut().enumerate() {
        let position = (idx as i32) + 1;
        entry.position = position;
        entry.eta = avg_secs.map(|avg| {
            let wait_secs = i64::from(position - 1) * i64::from(avg);
            now + Duration::seconds(wait_secs)
        });
    }

    Ok(Json(QueueResponse {
        tester_id,
        running,
        queued,
    }))
}

async fn get_cost_estimate(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<CostEstimateResponse>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("get_cost_estimate pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("get_cost_estimate query", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".to_string()))?;

    let (always_on, with_schedule) = cost_estimate(&tester.vm_size, tester.auto_shutdown_enabled);
    Ok(Json(CostEstimateResponse {
        vm_size: tester.vm_size.clone(),
        hourly_usd: hourly_usd(&tester.vm_size),
        monthly_always_on_usd: always_on,
        monthly_with_schedule_usd: with_schedule,
        auto_shutdown_enabled: tester.auto_shutdown_enabled,
    }))
}

// ── Rate-limit helper (Task 15) ───────────────────────────────────────────

/// Total-tester cap per project.
const MAX_TESTERS_PER_PROJECT: i64 = 10;

/// Hourly create-burst cap per project.
const MAX_TESTERS_PER_HOUR: i64 = 5;

/// Decision helper for rate-limit gating — pure so it's unit-testable.
/// Returns `Err(message)` if either cap is violated.
fn check_rate_limit(total: i64, last_hour: i64) -> Result<(), String> {
    if total >= MAX_TESTERS_PER_PROJECT {
        return Err(format!(
            "project already has {total} testers (max {MAX_TESTERS_PER_PROJECT})"
        ));
    }
    if last_hour >= MAX_TESTERS_PER_HOUR {
        return Err(format!(
            "project created {last_hour} testers in the last hour (max {MAX_TESTERS_PER_HOUR}/h)"
        ));
    }
    Ok(())
}

// ── Lifecycle handlers (Task 15) ──────────────────────────────────────────

/// Strongly-typed request body for `POST /testers`. This mirrors
/// `CreateTesterInput` plus the bool for `auto_probe_enabled`, which the
/// DB layer also exposes optionally.
#[derive(Debug, Deserialize)]
struct CreateTesterBody {
    name: String,
    cloud: String,
    region: String,
    #[serde(default)]
    vm_size: Option<String>,
    #[serde(default)]
    auto_shutdown_local_hour: Option<i16>,
    #[serde(default)]
    auto_probe_enabled: Option<bool>,
}

impl From<CreateTesterBody> for CreateTesterInput {
    fn from(b: CreateTesterBody) -> Self {
        CreateTesterInput {
            name: b.name,
            cloud: b.cloud,
            region: b.region,
            vm_size: b.vm_size,
            auto_shutdown_local_hour: b.auto_shutdown_local_hour,
            auto_probe_enabled: b.auto_probe_enabled,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpgradeBody {
    #[serde(default)]
    confirm: bool,
}

async fn create_tester(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)
        .map_err(|s| (s, "Operator role required".to_string()))?;

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let body: CreateTesterBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    if body.name.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name must not be empty".into()));
    }
    if body.cloud.trim().is_empty() || body.region.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "cloud and region are required".into(),
        ));
    }

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("create_tester pool", e))?;

    // Rate-limit: total testers in project + bursts in the last hour.
    let totals = client
        .query_one(
            "SELECT \
                COUNT(*)::bigint AS total, \
                COUNT(*) FILTER (WHERE created_at > NOW() - INTERVAL '1 hour')::bigint AS last_hour \
             FROM project_tester WHERE project_id = $1",
            &[&ctx.project_id],
        )
        .await
        .map_err(|e| db_error("create_tester rate-limit query", e))?;
    let total: i64 = totals.get("total");
    let last_hour: i64 = totals.get("last_hour");

    if let Err(msg) = check_rate_limit(total, last_hour) {
        return Err((StatusCode::TOO_MANY_REQUESTS, msg));
    }

    let input: CreateTesterInput = body.into();
    let row = crate::db::project_testers::insert(&client, &ctx.project_id, &input, &user.user_id)
        .await
        .map_err(|e| db_error("create_tester insert", e))?;

    // TODO(Task 17): audit_tester_action(state, ctx, user, "create", row.tester_id, ...)
    tracing::info!(
        tester_id = %row.tester_id,
        project_id = %ctx.project_id,
        created_by = %user.email,
        region = %row.region,
        vm_size = %row.vm_size,
        "tester created (provisioning in background)"
    );

    // Drop the client before spawning; the background task acquires its own.
    drop(client);

    spawn_create_tester_task(state.clone(), ctx.project_id.clone(), row.tester_id, input);

    Ok((StatusCode::ACCEPTED, Json(row)))
}

async fn start_tester(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)
        .map_err(|s| (s, "Operator role required".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("start_tester pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("start_tester lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    if tester.power_state != "stopped" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot start tester in power_state={}; expected 'stopped'",
                tester.power_state
            ),
        ));
    }
    drop(client);

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        triggered_by = %user.email,
        "tester start requested"
    );
    // TODO(Task 17): audit hook.

    spawn_start_tester_task(state.clone(), tester.clone());
    Ok((StatusCode::ACCEPTED, Json(tester)))
}

async fn stop_tester(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)
        .map_err(|s| (s, "Operator role required".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("stop_tester pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("stop_tester lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    if tester.allocation != "idle" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot stop tester with allocation={}; must be idle",
                tester.allocation
            ),
        ));
    }
    if tester.power_state != "running" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot stop tester in power_state={}; expected 'running'",
                tester.power_state
            ),
        ));
    }

    let queue_count: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM benchmark_config \
             WHERE tester_id = $1 AND status IN ('queued','pending','running')",
            &[&tester_id],
        )
        .await
        .map_err(|e| db_error("stop_tester queue check", e))?
        .get(0);
    if queue_count > 0 {
        return Err((
            StatusCode::CONFLICT,
            format!("cannot stop tester with {queue_count} benchmark(s) in flight"),
        ));
    }
    drop(client);

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        triggered_by = %user.email,
        "tester stop requested"
    );
    // TODO(Task 17): audit hook.

    spawn_stop_tester_task(state.clone(), tester.clone());
    Ok((StatusCode::ACCEPTED, Json(tester)))
}

async fn upgrade_tester(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 4)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let body: UpgradeBody = if body_bytes.is_empty() {
        UpgradeBody { confirm: false }
    } else {
        serde_json::from_slice(&body_bytes).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    };
    if !body.confirm {
        return Err((
            StatusCode::BAD_REQUEST,
            "upgrade requires {\"confirm\": true}".into(),
        ));
    }

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("upgrade_tester pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("upgrade_tester lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    if tester.allocation != "idle" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot upgrade tester with allocation={}; must be idle",
                tester.allocation
            ),
        ));
    }
    if tester.power_state != "running" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot upgrade tester in power_state={}; expected 'running'",
                tester.power_state
            ),
        ));
    }

    let queue_count: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM benchmark_config \
             WHERE tester_id = $1 AND status IN ('queued','pending','running')",
            &[&tester_id],
        )
        .await
        .map_err(|e| db_error("upgrade_tester queue check", e))?
        .get(0);
    if queue_count > 0 {
        return Err((
            StatusCode::CONFLICT,
            format!("cannot upgrade tester with {queue_count} benchmark(s) in flight"),
        ));
    }
    drop(client);

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        triggered_by = %user.email,
        "tester upgrade requested"
    );
    // TODO(Task 17): audit hook.

    spawn_upgrade_tester_task(state.clone(), tester.clone());
    Ok((StatusCode::ACCEPTED, Json(tester)))
}

async fn delete_tester(
    State(state): State<Arc<AppState>>,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("delete_tester pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("delete_tester lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    let transient = matches!(
        tester.power_state.as_str(),
        "provisioning" | "starting" | "stopping" | "upgrading"
    );
    if transient {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot delete tester in transient power_state={}",
                tester.power_state
            ),
        ));
    }
    if tester.allocation != "idle" {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot delete tester with allocation={}; must be idle",
                tester.allocation
            ),
        ));
    }

    let queue_count: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM benchmark_config \
             WHERE tester_id = $1 AND status IN ('queued','pending','running')",
            &[&tester_id],
        )
        .await
        .map_err(|e| db_error("delete_tester queue check", e))?
        .get(0);
    if queue_count > 0 {
        return Err((
            StatusCode::CONFLICT,
            format!("cannot delete tester with {queue_count} benchmark(s) in flight"),
        ));
    }

    // Destroy the VM synchronously where possible, then the row. If the VM
    // delete fails we refuse to delete the row so the user can retry
    // (otherwise we'd leak Azure resources).
    if let Some(resource_id) = tester.vm_resource_id.as_deref() {
        if let Err(e) = azure_vm::az_vm_delete(resource_id).await {
            tracing::error!(
                %tester_id,
                error = %e,
                "az vm delete failed; leaving row in place"
            );
            let _ = tester_state::set_status_message(
                &client,
                &tester_id,
                &format!("delete failed: {e}"),
            )
            .await;
            return Err((StatusCode::BAD_GATEWAY, format!("az vm delete failed: {e}")));
        }
    } else {
        tracing::warn!(
            %tester_id,
            "tester has no vm_resource_id; deleting row without Azure call"
        );
    }

    let deleted = crate::db::project_testers::delete(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("delete_tester delete", e))?;
    if !deleted {
        return Err((StatusCode::NOT_FOUND, "Tester not found".into()));
    }

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        deleted_by = %user.email,
        "tester deleted"
    );
    // TODO(Task 17): audit hook.

    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ── Background task helpers ───────────────────────────────────────────────

fn spawn_create_tester_task(
    state: Arc<AppState>,
    project_id: String,
    tester_id: Uuid,
    input: CreateTesterInput,
) {
    tokio::spawn(async move {
        if let Err(e) = run_create_tester(state.clone(), project_id, tester_id, input).await {
            tracing::error!(%tester_id, error = ?e, "tester create background task failed");
            if let Ok(client) = state.db.get().await {
                let _ = client
                    .execute(
                        "UPDATE project_tester SET power_state='error', \
                         status_message=$2, updated_at=NOW() WHERE tester_id=$1",
                        &[&tester_id, &format!("create failed: {e}")],
                    )
                    .await;
            }
        }
    });
}

async fn run_create_tester(
    state: Arc<AppState>,
    _project_id: String,
    tester_id: Uuid,
    _input: CreateTesterInput,
) -> anyhow::Result<()> {
    // Step 1: load the row so we have region + vm_size.
    let client = state.db.get().await?;
    let row = client
        .query_one(
            "SELECT region, vm_size FROM project_tester WHERE tester_id = $1",
            &[&tester_id],
        )
        .await?;
    let region: String = row.get("region");
    let vm_size: String = row.get("vm_size");

    // Step 2: provision the VM via `az vm create`.
    tester_state::set_status_message(&client, &tester_id, "creating Azure VM").await?;
    let vm_name = azure_vm::generate_vm_name(&region);
    let created = azure_vm::az_vm_create(&vm_name, &region, &vm_size).await?;

    // Step 3: persist identity fields so the next stages can find the host.
    client
        .execute(
            "UPDATE project_tester \
             SET vm_name = $2, vm_resource_id = $3, public_ip = $4::inet, \
                 ssh_user = $5, updated_at = NOW() \
             WHERE tester_id = $1",
            &[
                &tester_id,
                &created.vm_name,
                &created.resource_id,
                &created.public_ip,
                &created.admin_username,
            ],
        )
        .await?;

    // Step 4: run the installer. Progress closure captures an Arc<Pool> so
    // each step writes back to `status_message`.
    let state_for_progress = state.clone();
    let progress = move |msg: &str| {
        let msg = msg.to_string();
        tracing::info!(%tester_id, msg = %msg, "install progress");
        let state = state_for_progress.clone();
        // Fire-and-forget — we don't want install slowdowns if the DB
        // momentarily hiccups.
        tokio::spawn(async move {
            if let Ok(c) = state.db.get().await {
                let _ = tester_state::set_status_message(&c, &tester_id, &msg).await;
            }
        });
    };
    let target = tester_install::TesterTarget {
        tester_id,
        public_ip: Some(created.public_ip.clone()),
        ssh_user: created.admin_username.clone(),
    };
    tester_install::install_tester(&target, progress).await?;

    // Step 5: provisioning → running + stamp installer_version + next_shutdown_at.
    let installer_version = env!("CARGO_PKG_VERSION");
    let moved =
        tester_state::try_power_transition(&client, &tester_id, "provisioning", "running").await?;
    if !moved {
        tracing::warn!(%tester_id, "power_state was not 'provisioning' at end of install");
    }
    client
        .execute(
            "UPDATE project_tester \
             SET installer_version = $2, last_installed_at = NOW(), \
                 status_message = NULL, updated_at = NOW() \
             WHERE tester_id = $1",
            &[&tester_id, &installer_version],
        )
        .await?;

    // next_shutdown_at: compute in the tester's local shutdown hour. For
    // MVP we just set it to NOW()+15h; the auto-shutdown loop (Task 11)
    // recomputes precise windows from the hour column.
    client
        .execute(
            "UPDATE project_tester \
             SET next_shutdown_at = NOW() + INTERVAL '15 hours' \
             WHERE tester_id = $1 AND auto_shutdown_enabled = TRUE",
            &[&tester_id],
        )
        .await?;

    tracing::info!(%tester_id, "tester provisioning complete");
    Ok(())
}

fn spawn_start_tester_task(state: Arc<AppState>, tester: ProjectTesterRow) {
    let tester_id = tester.tester_id;
    tokio::spawn(async move {
        if let Err(e) = run_start_tester(state.clone(), tester).await {
            tracing::error!(%tester_id, error = ?e, "tester start background task failed");
            if let Ok(client) = state.db.get().await {
                let _ = client
                    .execute(
                        "UPDATE project_tester SET power_state='error', \
                         status_message=$2, updated_at=NOW() WHERE tester_id=$1",
                        &[&tester_id, &format!("start failed: {e}")],
                    )
                    .await;
            }
        }
    });
}

async fn run_start_tester(state: Arc<AppState>, tester: ProjectTesterRow) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let moved =
        tester_state::try_power_transition(&client, &tester.tester_id, "stopped", "starting")
            .await?;
    if !moved {
        anyhow::bail!("tester no longer in 'stopped' state");
    }
    tester_state::set_status_message(&client, &tester.tester_id, "starting Azure VM").await?;

    let resource_id = tester
        .vm_resource_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("tester has no vm_resource_id"))?;
    azure_vm::az_vm_start(resource_id).await?;

    // Wait for SSH to come back up.
    if let Some(ip) = tester.public_ip.as_deref() {
        let target = tester_install::TesterTarget {
            tester_id: tester.tester_id,
            public_ip: Some(ip.to_string()),
            ssh_user: tester.ssh_user.clone(),
        };
        // `install_tester` is too heavy; use a minimal SSH readiness poll by
        // invoking `install_tester` with a short-circuit? No — just poll
        // `ssh true` via the wait_for_ssh flow. For MVP we delegate to
        // install_tester only on create/upgrade; here we simply call a
        // lightweight ssh probe by doing up to 30x `ssh true`.
        wait_for_ssh_ready(&target).await?;
    }

    let moved =
        tester_state::try_power_transition(&client, &tester.tester_id, "starting", "running")
            .await?;
    if !moved {
        tracing::warn!(tester_id = %tester.tester_id, "power_state changed mid-start");
    }
    tester_state::set_status_message(&client, &tester.tester_id, "").await?;
    Ok(())
}

/// Minimal SSH readiness wait used by the start path (install_tester does
/// its own, but we don't re-run the whole installer on a warm start).
async fn wait_for_ssh_ready(target: &tester_install::TesterTarget) -> anyhow::Result<()> {
    let ip = target
        .public_ip
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("no public_ip"))?;
    let user = target.ssh_user.as_str();
    for _ in 0..30u32 {
        let ok = tokio::process::Command::new("ssh")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg(format!("{user}@{ip}"))
            .arg("true")
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
    anyhow::bail!("SSH did not become ready within 5 minutes")
}

fn spawn_stop_tester_task(state: Arc<AppState>, tester: ProjectTesterRow) {
    let tester_id = tester.tester_id;
    tokio::spawn(async move {
        if let Err(e) = run_stop_tester(state.clone(), tester).await {
            tracing::error!(%tester_id, error = ?e, "tester stop background task failed");
            if let Ok(client) = state.db.get().await {
                let _ = client
                    .execute(
                        "UPDATE project_tester SET power_state='error', \
                         status_message=$2, updated_at=NOW() WHERE tester_id=$1",
                        &[&tester_id, &format!("stop failed: {e}")],
                    )
                    .await;
            }
        }
    });
}

async fn run_stop_tester(state: Arc<AppState>, tester: ProjectTesterRow) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let moved =
        tester_state::try_power_transition(&client, &tester.tester_id, "running", "stopping")
            .await?;
    if !moved {
        anyhow::bail!("tester no longer in 'running' state");
    }
    tester_state::set_status_message(&client, &tester.tester_id, "deallocating Azure VM").await?;

    let resource_id = tester
        .vm_resource_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("tester has no vm_resource_id"))?;
    azure_vm::az_vm_deallocate(resource_id).await?;

    let moved =
        tester_state::try_power_transition(&client, &tester.tester_id, "stopping", "stopped")
            .await?;
    if !moved {
        tracing::warn!(tester_id = %tester.tester_id, "power_state changed mid-stop");
    }
    tester_state::set_status_message(&client, &tester.tester_id, "").await?;
    Ok(())
}

fn spawn_upgrade_tester_task(state: Arc<AppState>, tester: ProjectTesterRow) {
    let tester_id = tester.tester_id;
    tokio::spawn(async move {
        if let Err(e) = run_upgrade_tester(state.clone(), tester).await {
            tracing::error!(%tester_id, error = ?e, "tester upgrade background task failed");
            if let Ok(client) = state.db.get().await {
                // Best-effort recovery: release the upgrading lock via tester_state.
                let _ = tester_state::force_release(&client, &tester_id).await;
                let _ = client
                    .execute(
                        "UPDATE project_tester SET status_message=$2, updated_at=NOW() \
                         WHERE tester_id=$1",
                        &[&tester_id, &format!("upgrade failed: {e}")],
                    )
                    .await;
            }
        }
    });
}

async fn run_upgrade_tester(state: Arc<AppState>, tester: ProjectTesterRow) -> anyhow::Result<()> {
    let client = state.db.get().await?;

    // Flip allocation idle → upgrading atomically.
    let rows = client
        .execute(
            "UPDATE project_tester \
             SET allocation = 'upgrading', updated_at = NOW() \
             WHERE tester_id = $1 AND allocation = 'idle'",
            &[&tester.tester_id],
        )
        .await?;
    if rows != 1 {
        anyhow::bail!("tester allocation was no longer 'idle'");
    }
    tester_state::set_status_message(&client, &tester.tester_id, "upgrading tester").await?;

    let state_for_progress = state.clone();
    let tester_id = tester.tester_id;
    let progress = move |msg: &str| {
        let msg = msg.to_string();
        tracing::info!(%tester_id, msg = %msg, "upgrade progress");
        let state = state_for_progress.clone();
        tokio::spawn(async move {
            if let Ok(c) = state.db.get().await {
                let _ = tester_state::set_status_message(&c, &tester_id, &msg).await;
            }
        });
    };

    let target = tester_install::TesterTarget {
        tester_id,
        public_ip: tester.public_ip.clone(),
        ssh_user: tester.ssh_user.clone(),
    };
    tester_install::install_tester(&target, progress).await?;

    // Success — release the upgrading lock, stamp installer_version.
    let installer_version = env!("CARGO_PKG_VERSION");
    tester_state::force_release(&client, &tester.tester_id).await?;
    client
        .execute(
            "UPDATE project_tester \
             SET installer_version = $2, last_installed_at = NOW(), \
                 status_message = NULL, updated_at = NOW() \
             WHERE tester_id = $1",
            &[&tester.tester_id, &installer_version],
        )
        .await?;
    tracing::info!(%tester_id, "tester upgrade complete");
    Ok(())
}

// ── Router ────────────────────────────────────────────────────────────────

/// Build the tester REST router. Designed to be merged into the project-
/// scoped router (which nests `/projects/{project_id}`) in Task 18.
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/testers", get(list_testers).post(create_tester))
        .route("/testers/regions", get(list_regions))
        .route(
            "/testers/{tester_id}",
            get(get_tester).delete(delete_tester),
        )
        .route("/testers/{tester_id}/queue", get(get_queue))
        .route("/testers/{tester_id}/cost_estimate", get(get_cost_estimate))
        .route("/testers/{tester_id}/start", post(start_tester))
        .route("/testers/{tester_id}/stop", post(stop_tester))
        .route("/testers/{tester_id}/upgrade", post(upgrade_tester))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hourly_usd_known_sizes() {
        assert!((hourly_usd("Standard_D2s_v3") - 0.096).abs() < f64::EPSILON);
        assert!((hourly_usd("Standard_D4s_v3") - 0.192).abs() < f64::EPSILON);
        assert!((hourly_usd("Standard_D8s_v3") - 0.384).abs() < f64::EPSILON);
    }

    #[test]
    fn hourly_usd_unknown_falls_back_to_d2s_v3() {
        assert!((hourly_usd("Standard_Unknown") - 0.096).abs() < f64::EPSILON);
        assert!((hourly_usd("") - 0.096).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_estimate_always_on_when_shutdown_disabled() {
        let (always_on, with_schedule) = cost_estimate("Standard_D2s_v3", false);
        let expected = 24.0 * 30.0 * 0.096;
        assert!((always_on - expected).abs() < 1e-9);
        assert!((with_schedule - expected).abs() < 1e-9);
    }

    #[test]
    fn cost_estimate_reduced_when_shutdown_enabled() {
        let (always_on, with_schedule) = cost_estimate("Standard_D2s_v3", true);
        let expected_always = 24.0 * 30.0 * 0.096;
        let expected_sched = 15.0 * 30.0 * 0.096;
        assert!((always_on - expected_always).abs() < 1e-9);
        assert!((with_schedule - expected_sched).abs() < 1e-9);
        assert!(with_schedule < always_on);
    }

    #[test]
    fn cost_estimate_scales_with_size() {
        let (d2_always, _) = cost_estimate("Standard_D2s_v3", false);
        let (d4_always, _) = cost_estimate("Standard_D4s_v3", false);
        let (d8_always, _) = cost_estimate("Standard_D8s_v3", false);
        assert!((d4_always - 2.0 * d2_always).abs() < 1e-9);
        assert!((d8_always - 4.0 * d2_always).abs() < 1e-9);
    }

    #[test]
    fn rate_limit_allows_fresh_project() {
        assert!(check_rate_limit(0, 0).is_ok());
        assert!(check_rate_limit(3, 2).is_ok());
    }

    #[test]
    fn rate_limit_blocks_on_total_cap() {
        let err = check_rate_limit(MAX_TESTERS_PER_PROJECT, 0).unwrap_err();
        assert!(err.contains("max"));
        let err = check_rate_limit(MAX_TESTERS_PER_PROJECT + 5, 0).unwrap_err();
        assert!(err.contains("max"));
    }

    #[test]
    fn rate_limit_blocks_on_hourly_cap() {
        let err = check_rate_limit(1, MAX_TESTERS_PER_HOUR).unwrap_err();
        assert!(err.contains("hour"));
    }

    #[test]
    fn rate_limit_boundary_exact() {
        // Right at the cap-1 is still OK.
        assert!(check_rate_limit(MAX_TESTERS_PER_PROJECT - 1, MAX_TESTERS_PER_HOUR - 1).is_ok());
    }

    #[test]
    fn create_tester_body_deserializes_minimum() {
        let json = r#"{"name":"t1","cloud":"azure","region":"eastus"}"#;
        let body: CreateTesterBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.name, "t1");
        assert_eq!(body.cloud, "azure");
        assert_eq!(body.region, "eastus");
        assert!(body.vm_size.is_none());
        assert!(body.auto_probe_enabled.is_none());
    }

    #[test]
    fn create_tester_body_deserializes_full() {
        let json = r#"{
            "name":"t1","cloud":"azure","region":"eastus",
            "vm_size":"Standard_D4s_v3",
            "auto_shutdown_local_hour":22,
            "auto_probe_enabled":true
        }"#;
        let body: CreateTesterBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.vm_size.as_deref(), Some("Standard_D4s_v3"));
        assert_eq!(body.auto_shutdown_local_hour, Some(22));
        assert_eq!(body.auto_probe_enabled, Some(true));
    }

    #[test]
    fn upgrade_body_requires_confirm() {
        let b: UpgradeBody = serde_json::from_str(r#"{"confirm":true}"#).unwrap();
        assert!(b.confirm);
        let b: UpgradeBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!b.confirm);
    }

    #[test]
    fn fallback_regions_non_empty() {
        assert!(!FALLBACK_AZURE_REGIONS.is_empty());
        assert!(FALLBACK_AZURE_REGIONS.contains(&"eastus"));
    }
}
