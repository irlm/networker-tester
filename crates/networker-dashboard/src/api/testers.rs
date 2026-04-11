//! Persistent tester listing + inspection REST endpoints (Task 14).
//!
//! Wiring into the main router happens in Task 18.
//!
//! Routes (all require `ProjectRole::Viewer` — the lowest project role,
//! closest to the "Member" level referenced in the plan):
//!
//!   GET /projects/{pid}/testers
//!   GET /projects/{pid}/testers/regions
//!   GET /projects/{pid}/testers/{tid}
//!   GET /projects/{pid}/testers/{tid}/queue
//!   GET /projects/{pid}/testers/{tid}/cost_estimate
//!
//! TODO: integration tests need an api_testers.rs harness (full axum + DB).
//!       For now we cover the pure helpers (`hourly_usd`, `cost_estimate`)
//!       with unit tests and land the handlers so Task 18 can wire them.
#![allow(dead_code)]

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{ProjectContext, ProjectRole};
use crate::db::project_testers::ProjectTesterRow;
use crate::AppState;

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

// ── Router ────────────────────────────────────────────────────────────────

/// Build the tester REST router. Designed to be merged into the project-
/// scoped router (which nests `/projects/{project_id}`) in Task 18.
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/testers", get(list_testers))
        .route("/testers/regions", get(list_regions))
        .route("/testers/{tester_id}", get(get_tester))
        .route("/testers/{tester_id}/queue", get(get_queue))
        .route("/testers/{tester_id}/cost_estimate", get(get_cost_estimate))
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
    fn fallback_regions_non_empty() {
        assert!(!FALLBACK_AZURE_REGIONS.is_empty());
        assert!(FALLBACK_AZURE_REGIONS.contains(&"eastus"));
    }
}
