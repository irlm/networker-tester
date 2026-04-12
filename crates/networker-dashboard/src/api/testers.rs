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
//! Task 17 wired `audit_tester_action` into each mutating endpoint — the
//! helper currently emits structured `tracing` events only (there is no
//! `service_log` table yet; see Task 11 retrospective). A follow-up task
//! can upgrade the sink to a real audit table without changing the call
//! sites.
#![allow(dead_code)]

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::db::project_testers::{CreateTesterInput, ProjectTesterRow};
use crate::AppState;
use networker_dashboard::services::{
    azure_regions, azure_vm, tester_install, tester_recovery, tester_state, version_refresh,
};

// ── Audit helper ──────────────────────────────────────────────────────────

/// Record a tester action for audit purposes.
///
/// Currently emits a structured `tracing` event at the `tester_action`
/// target — there is no `service_log` table in the dashboard schema yet
/// (confirmed in Task 11). When one lands, this helper is the single
/// sink all call sites already go through, so the upgrade is local.
///
/// `outcome` is typically one of `"requested"` (async task spawned),
/// `"success"` (synchronous completion), or `"failed"`.
#[allow(clippy::too_many_arguments)]
pub(super) fn audit_tester_action(
    _state: &AppState,
    project_id: &str,
    tester_id: Uuid,
    actor_user_id: Option<Uuid>,
    action: &str,
    outcome: &str,
    message: Option<&str>,
) {
    tracing::info!(
        target: "tester_action",
        %project_id,
        %tester_id,
        ?actor_user_id,
        action,
        outcome,
        message,
        "tester action audited"
    );
}

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
    ctx: ProjectContext,
) -> Result<Json<Vec<ProjectTesterRow>>, (StatusCode, String)> {
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
    ctx: ProjectContext,
) -> Result<Json<RegionsResponse>, (StatusCode, String)> {
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
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
) -> Result<Json<ProjectTesterRow>, (StatusCode, String)> {
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
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
) -> Result<Json<QueueResponse>, (StatusCode, String)> {
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
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
) -> Result<Json<CostEstimateResponse>, (StatusCode, String)> {
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
    ctx: ProjectContext,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
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

    tracing::info!(
        tester_id = %row.tester_id,
        project_id = %ctx.project_id,
        created_by = %user.email,
        region = %row.region,
        vm_size = %row.vm_size,
        "tester created (provisioning in background)"
    );
    audit_tester_action(
        &state,
        &ctx.project_id,
        row.tester_id,
        Some(user.user_id),
        "tester_created",
        "requested",
        Some(&format!("region={} vm_size={}", row.region, row.vm_size)),
    );

    // Drop the client before spawning; the background task acquires its own.
    drop(client);

    spawn_create_tester_task(state.clone(), ctx.project_id.clone(), row.tester_id, input);

    Ok((StatusCode::ACCEPTED, Json(row)))
}

async fn start_tester(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
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
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_start_requested",
        "requested",
        None,
    );

    spawn_start_tester_task(state.clone(), tester.clone());
    Ok((StatusCode::ACCEPTED, Json(tester)))
}

async fn stop_tester(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
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
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_stop_requested",
        "requested",
        None,
    );

    spawn_stop_tester_task(state.clone(), tester.clone());
    Ok((StatusCode::ACCEPTED, Json(tester)))
}

async fn upgrade_tester(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<ProjectTesterRow>), (StatusCode, String)> {
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
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_upgrade_requested",
        "requested",
        None,
    );

    spawn_upgrade_tester_task(state.clone(), tester.clone());
    Ok((StatusCode::ACCEPTED, Json(tester)))
}

async fn delete_tester(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
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
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_deleted",
        "success",
        None,
    );

    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ── Schedule + recovery handlers (Task 16) ────────────────────────────────

#[derive(Debug, Deserialize)]
struct ScheduleBody {
    #[serde(default)]
    auto_shutdown_enabled: Option<bool>,
    #[serde(default)]
    auto_shutdown_local_hour: Option<i16>,
}

/// Body for `POST /testers/{tid}/postpone`. The three variants are
/// mutually exclusive — exactly one shape must be supplied.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PostponeBody {
    Until { until: DateTime<Utc> },
    AddHours { add_hours: i64 },
    SkipTonight { skip_tonight: bool },
}

#[derive(Debug, Deserialize)]
struct ForceStopBody {
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    reason: String,
}

async fn update_schedule(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<ProjectTesterRow>, (StatusCode, String)> {
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 4)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let body: ScheduleBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    if let Some(h) = body.auto_shutdown_local_hour {
        if !(0..=23).contains(&h) {
            return Err((
                StatusCode::BAD_REQUEST,
                "auto_shutdown_local_hour must be 0..=23".into(),
            ));
        }
    }
    if body.auto_shutdown_enabled.is_none() && body.auto_shutdown_local_hour.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one of auto_shutdown_enabled or auto_shutdown_local_hour required".into(),
        ));
    }

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("update_schedule pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("update_schedule lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    // Merge proposed values with existing row to decide the new schedule.
    let new_enabled = body
        .auto_shutdown_enabled
        .unwrap_or(tester.auto_shutdown_enabled);
    let new_hour = body
        .auto_shutdown_local_hour
        .unwrap_or(tester.auto_shutdown_local_hour);

    // Recompute next_shutdown_at. If disabled, clear it; otherwise compute
    // the next UTC instant for the region + hour pair.
    let next_shutdown: Option<DateTime<Utc>> = if new_enabled {
        Some(azure_regions::next_shutdown_at(
            &tester.region,
            new_hour,
            Utc::now(),
        ))
    } else {
        None
    };

    client
        .execute(
            "UPDATE project_tester \
             SET auto_shutdown_enabled = $2, \
                 auto_shutdown_local_hour = $3, \
                 next_shutdown_at = $4, \
                 updated_at = NOW() \
             WHERE tester_id = $1",
            &[&tester_id, &new_enabled, &new_hour, &next_shutdown],
        )
        .await
        .map_err(|e| db_error("update_schedule update", e))?;

    let updated = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("update_schedule reload", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        actor = %user.email,
        auto_shutdown_enabled = new_enabled,
        auto_shutdown_local_hour = new_hour,
        "tester schedule updated"
    );
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_schedule_changed",
        "success",
        Some(&format!(
            "auto_shutdown_enabled={new_enabled} auto_shutdown_local_hour={new_hour}"
        )),
    );

    Ok(Json(updated))
}

async fn postpone_shutdown(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<ProjectTesterRow>, (StatusCode, String)> {
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 4)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let body: PostponeBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("postpone pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("postpone lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    let now = Utc::now();
    let new_next =
        compute_postpone(&body, &tester, now).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    client
        .execute(
            "UPDATE project_tester \
             SET next_shutdown_at = $2, \
                 shutdown_deferral_count = shutdown_deferral_count + 1, \
                 updated_at = NOW() \
             WHERE tester_id = $1",
            &[&tester_id, &new_next],
        )
        .await
        .map_err(|e| db_error("postpone update", e))?;

    let updated = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("postpone reload", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        actor = %user.email,
        next_shutdown_at = %new_next,
        "tester shutdown postponed"
    );
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_postponed",
        "success",
        Some(&format!("next_shutdown_at={new_next}")),
    );

    Ok(Json(updated))
}

/// Pure postpone computation, extracted so it can be unit-tested without a
/// DB. Returns `Err(msg)` for illegal shapes (past `until`, zero hours, etc).
fn compute_postpone(
    body: &PostponeBody,
    tester: &ProjectTesterRow,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, String> {
    match body {
        PostponeBody::Until { until } => {
            if *until <= now {
                return Err("until must be in the future".into());
            }
            Ok(*until)
        }
        PostponeBody::AddHours { add_hours } => {
            if *add_hours <= 0 {
                return Err("add_hours must be positive".into());
            }
            let base = tester.next_shutdown_at.unwrap_or(now);
            Ok(base + Duration::hours(*add_hours))
        }
        PostponeBody::SkipTonight { skip_tonight } => {
            if !*skip_tonight {
                return Err("skip_tonight must be true".into());
            }
            // Recompute tomorrow's slot by asking azure_regions for the next
            // slot starting from (now + 24h) — this rolls forward one day.
            Ok(azure_regions::next_shutdown_at(
                &tester.region,
                tester.auto_shutdown_local_hour,
                now + Duration::hours(24),
            ))
        }
    }
}

async fn probe_tester(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<ProjectTesterRow>, (StatusCode, String)> {
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("probe_tester pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("probe_tester lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    if matches!(tester.allocation.as_str(), "locked" | "upgrading") {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "cannot probe tester with allocation={}; retry once idle",
                tester.allocation
            ),
        ));
    }

    let azure_state = tester_recovery::probe_azure_state(&tester.vm_resource_id, &tester.vm_name)
        .await
        .map_err(|e| {
            tracing::warn!(%tester_id, error = ?e, "probe_azure_state failed");
            (
                StatusCode::BAD_GATEWAY,
                format!("az vm get-instance-view failed: {e}"),
            )
        })?;
    let new_power = tester_recovery::azure_power_to_row(&azure_state);
    let status = format!("Manual probe: Azure reported {azure_state}");

    client
        .execute(
            "UPDATE project_tester \
             SET power_state = $2, status_message = $3, updated_at = NOW() \
             WHERE tester_id = $1",
            &[&tester_id, &new_power, &status],
        )
        .await
        .map_err(|e| db_error("probe_tester update", e))?;

    let updated = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("probe_tester reload", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    tracing::info!(
        %tester_id,
        project_id = %ctx.project_id,
        actor = %user.email,
        azure_state = %azure_state,
        resolved = %new_power,
        "tester probed"
    );
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_probed",
        "success",
        Some(&format!("azure_state={azure_state} resolved={new_power}")),
    );

    Ok(Json(updated))
}

async fn force_stop_tester(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, tester_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<ProjectTesterRow>, (StatusCode, String)> {
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 4)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let body: ForceStopBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if !body.confirm {
        return Err((
            StatusCode::BAD_REQUEST,
            "force-stop requires {\"confirm\": true, \"reason\": \"...\"}".into(),
        ));
    }
    if body.reason.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "reason must not be empty".into()));
    }

    let client = state
        .db
        .get()
        .await
        .map_err(|e| db_error("force_stop pool", e))?;
    let tester = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("force_stop lookup", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    // Refuse 409 if the tester is actively running a benchmark — the user
    // should cancel the benchmark first (which unlocks via the dispatcher).
    if tester.power_state == "running" && tester.allocation == "locked" {
        return Err((
            StatusCode::CONFLICT,
            "cannot force-stop tester while a benchmark is actively running; cancel the benchmark first".into(),
        ));
    }

    // Two-step to keep the grep-guard invariant clean: route the allocation
    // clear through `tester_state::force_release` (the only sanctioned
    // writer of `allocation='idle'`), then issue a follow-up UPDATE that
    // ONLY touches `power_state` + `status_message`.
    tester_state::force_release(&client, &tester_id)
        .await
        .map_err(|e| db_error("force_stop release", e))?;

    let status = format!("Force-stopped: {}", body.reason);
    client
        .execute(
            "UPDATE project_tester \
             SET power_state = 'stopped', status_message = $2, updated_at = NOW() \
             WHERE tester_id = $1",
            &[&tester_id, &status],
        )
        .await
        .map_err(|e| db_error("force_stop update", e))?;

    let updated = crate::db::project_testers::get(&client, &ctx.project_id, &tester_id)
        .await
        .map_err(|e| db_error("force_stop reload", e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Tester not found".into()))?;

    // Force-stop is a high-impact admin action; audit record is mandatory.
    tracing::warn!(
        target: "tester_force_stop",
        %tester_id,
        project_id = %ctx.project_id,
        actor = %user.email,
        reason = %body.reason,
        "tester force-stopped (admin override)"
    );
    audit_tester_action(
        &state,
        &ctx.project_id,
        tester_id,
        Some(user.user_id),
        "tester_force_stopped",
        "success",
        Some(&format!("reason={}", body.reason)),
    );

    Ok(Json(updated))
}

// ── Refresh latest version handler ────────────────────────────────────────

#[derive(Serialize)]
struct RefreshLatestVersionResponse {
    latest_version: String,
}

/// Admin-only manual trigger for the GitHub releases latest-version fetch.
///
/// Uses the shared `latest_version_cache` on `AppState` which the background
/// loop `services::version_refresh::refresh_latest_version_loop` also updates,
/// so a manual refresh immediately benefits future reads.
async fn refresh_latest_version(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    req: axum::extract::Request,
) -> Result<Json<RefreshLatestVersionResponse>, (StatusCode, String)> {
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".to_string()))?;

    let resolved = version_refresh::refresh_now(state.latest_version_cache.clone())
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, "manual version refresh failed");
            (
                StatusCode::BAD_GATEWAY,
                format!("latest-version refresh failed: {e}"),
            )
        })?;

    audit_tester_action(
        &state,
        &ctx.project_id,
        Uuid::nil(),
        Some(user.user_id),
        "tester_latest_version_refreshed",
        "success",
        Some(&format!("latest_version={resolved}")),
    );

    Ok(Json(RefreshLatestVersionResponse {
        latest_version: resolved,
    }))
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
            "/testers/refresh-latest-version",
            post(refresh_latest_version),
        )
        .route(
            "/testers/{tester_id}",
            get(get_tester).delete(delete_tester),
        )
        .route("/testers/{tester_id}/queue", get(get_queue))
        .route("/testers/{tester_id}/cost_estimate", get(get_cost_estimate))
        .route("/testers/{tester_id}/start", post(start_tester))
        .route("/testers/{tester_id}/stop", post(stop_tester))
        .route("/testers/{tester_id}/upgrade", post(upgrade_tester))
        .route("/testers/{tester_id}/schedule", patch(update_schedule))
        .route("/testers/{tester_id}/postpone", post(postpone_shutdown))
        .route("/testers/{tester_id}/probe", post(probe_tester))
        .route("/testers/{tester_id}/force-stop", post(force_stop_tester))
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

    // ── Task 16: postpone + schedule deserializers ────────────────────

    fn fixture_row() -> ProjectTesterRow {
        let now = Utc::now();
        ProjectTesterRow {
            tester_id: Uuid::nil(),
            project_id: "proj".into(),
            name: "t".into(),
            cloud: "azure".into(),
            region: "eastus".into(),
            vm_size: "Standard_D2s_v3".into(),
            vm_name: Some("vm-x".into()),
            vm_resource_id: Some("/subscriptions/x/vm-x".into()),
            public_ip: Some("1.2.3.4".into()),
            ssh_user: "azureuser".into(),
            power_state: "running".into(),
            allocation: "idle".into(),
            status_message: None,
            locked_by_config_id: None,
            installer_version: None,
            last_installed_at: None,
            auto_shutdown_enabled: true,
            auto_shutdown_local_hour: 23,
            next_shutdown_at: Some(now + Duration::hours(5)),
            shutdown_deferral_count: 0,
            auto_probe_enabled: true,
            last_used_at: None,
            avg_benchmark_duration_seconds: None,
            benchmark_run_count: 0,
            created_by: Uuid::nil(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn postpone_body_until_deserializes() {
        let json = r#"{"until":"2030-01-01T00:00:00Z"}"#;
        let body: PostponeBody = serde_json::from_str(json).unwrap();
        match body {
            PostponeBody::Until { until } => {
                assert_eq!(until.to_rfc3339(), "2030-01-01T00:00:00+00:00");
            }
            _ => panic!("expected Until"),
        }
    }

    #[test]
    fn postpone_body_add_hours_deserializes() {
        let body: PostponeBody = serde_json::from_str(r#"{"add_hours":4}"#).unwrap();
        match body {
            PostponeBody::AddHours { add_hours } => assert_eq!(add_hours, 4),
            _ => panic!("expected AddHours"),
        }
    }

    #[test]
    fn postpone_body_skip_tonight_deserializes() {
        let body: PostponeBody = serde_json::from_str(r#"{"skip_tonight":true}"#).unwrap();
        match body {
            PostponeBody::SkipTonight { skip_tonight } => assert!(skip_tonight),
            _ => panic!("expected SkipTonight"),
        }
    }

    #[test]
    fn compute_postpone_until_future_ok() {
        let row = fixture_row();
        let now = Utc::now();
        let target = now + Duration::hours(10);
        let body = PostponeBody::Until { until: target };
        assert_eq!(compute_postpone(&body, &row, now).unwrap(), target);
    }

    #[test]
    fn compute_postpone_until_past_rejected() {
        let row = fixture_row();
        let now = Utc::now();
        let body = PostponeBody::Until {
            until: now - Duration::hours(1),
        };
        assert!(compute_postpone(&body, &row, now).is_err());
    }

    #[test]
    fn compute_postpone_add_hours_ok() {
        let row = fixture_row();
        let now = Utc::now();
        let base = row.next_shutdown_at.unwrap();
        let body = PostponeBody::AddHours { add_hours: 3 };
        let out = compute_postpone(&body, &row, now).unwrap();
        assert_eq!(out, base + Duration::hours(3));
    }

    #[test]
    fn compute_postpone_add_hours_without_schedule_uses_now() {
        let mut row = fixture_row();
        row.next_shutdown_at = None;
        let now = Utc::now();
        let body = PostponeBody::AddHours { add_hours: 2 };
        let out = compute_postpone(&body, &row, now).unwrap();
        assert_eq!(out, now + Duration::hours(2));
    }

    #[test]
    fn compute_postpone_add_hours_zero_rejected() {
        let row = fixture_row();
        let body = PostponeBody::AddHours { add_hours: 0 };
        assert!(compute_postpone(&body, &row, Utc::now()).is_err());
    }

    #[test]
    fn compute_postpone_add_hours_negative_rejected() {
        let row = fixture_row();
        let body = PostponeBody::AddHours { add_hours: -5 };
        assert!(compute_postpone(&body, &row, Utc::now()).is_err());
    }

    #[test]
    fn compute_postpone_skip_tonight_rolls_forward() {
        let row = fixture_row();
        let now = Utc::now();
        let body = PostponeBody::SkipTonight { skip_tonight: true };
        let out = compute_postpone(&body, &row, now).unwrap();
        // Skipping tonight must move next_shutdown_at strictly more than
        // the current `now`; typically well into the next day.
        assert!(out > now + Duration::hours(20));
    }

    #[test]
    fn compute_postpone_skip_tonight_false_rejected() {
        let row = fixture_row();
        let body = PostponeBody::SkipTonight {
            skip_tonight: false,
        };
        assert!(compute_postpone(&body, &row, Utc::now()).is_err());
    }

    #[test]
    fn schedule_body_partial_fields() {
        let b: ScheduleBody = serde_json::from_str(r#"{"auto_shutdown_enabled":false}"#).unwrap();
        assert_eq!(b.auto_shutdown_enabled, Some(false));
        assert!(b.auto_shutdown_local_hour.is_none());

        let b: ScheduleBody = serde_json::from_str(r#"{"auto_shutdown_local_hour":22}"#).unwrap();
        assert_eq!(b.auto_shutdown_local_hour, Some(22));
        assert!(b.auto_shutdown_enabled.is_none());
    }

    /// Compile-level guard: if any argument type or ordering drifts, this
    /// test fails to build, catching accidental signature changes to the
    /// audit helper before they break every call site.
    #[test]
    #[allow(clippy::type_complexity)]
    fn audit_tester_action_signature_stable() {
        type AuditFn = fn(&AppState, &str, Uuid, Option<Uuid>, &str, &str, Option<&str>);
        let _: AuditFn = audit_tester_action;
    }

    #[test]
    fn force_stop_body_requires_confirm_and_reason() {
        let b: ForceStopBody =
            serde_json::from_str(r#"{"confirm":true,"reason":"wedged"}"#).unwrap();
        assert!(b.confirm);
        assert_eq!(b.reason, "wedged");

        let b: ForceStopBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!b.confirm);
        assert!(b.reason.is_empty());
    }
}
