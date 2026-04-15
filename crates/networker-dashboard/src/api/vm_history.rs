//! REST endpoint for the VM usage history feed (Phase 1c).
//!
//! Project-scoped: `GET /api/projects/{pid}/vm-history`. Returns events from
//! the `vm_lifecycle` table with optional filters. Role gate is
//! `ProjectRole::Viewer` — everybody with read access to the project can see
//! the history; per-user operator-vs-admin scoping lands in v0.27.22 when
//! we wire `provider_account_id` and "only resources I own" filtering.
//!
//! Keeps the query shape thin: pagination via `limit` + `offset`, date
//! window via `from` / `to`, resource-kind filter via `resource_type`, and
//! per-resource drill-down via `resource_id`. Nothing fancier yet — the UI
//! in v0.27.20 will decide what gets promoted to first-class.
//!
//! Design: `docs/superpowers/specs/2026-04-15-vm-usage-history-design.md`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_role, ProjectContext, ProjectRole};
use crate::db::vm_lifecycle::{self, VmLifecycleRow};
use crate::AppState;

/// Hard ceiling so a buggy client can't pull the entire table.
const MAX_LIMIT: i64 = 500;
/// Default page size when the caller omits `limit`.
const DEFAULT_LIMIT: i64 = 100;

#[derive(Debug, Deserialize, Default)]
pub struct VmHistoryQuery {
    /// `tester` | `endpoint` | `benchmark`. When omitted, returns all kinds.
    pub resource_type: Option<String>,
    /// Restrict to a single resource (typically used from the UI detail
    /// drawer). When set, events are ordered oldest-first to show a
    /// natural timeline; otherwise the project-wide list orders newest-first.
    pub resource_id: Option<Uuid>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct VmHistoryResponse {
    pub events: Vec<VmLifecycleRow>,
    /// Whether there are probably more events past the returned window.
    /// Approximation: set when the returned row count equals the requested
    /// limit. The UI uses this to show a "load more" control without
    /// paying for a COUNT(*) on every page.
    pub has_more: bool,
}

async fn list_vm_history(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path(_project_id): Path<String>,
    Query(q): Query<VmHistoryQuery>,
) -> Result<Json<VmHistoryResponse>, (StatusCode, String)> {
    require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Viewer role required".to_string()))?;

    let client = state
        .db
        .get()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db pool: {e}")))?;

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);

    // Resource-scoped drill-down gets its own oldest-first query path so
    // the UI renders a natural create → start → stop timeline without
    // post-processing. `list_by_resource` already handles ordering.
    if let Some(resource_id) = q.resource_id {
        let rt = q
            .resource_type
            .as_deref()
            .and_then(parse_resource_type)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "resource_id requires a valid resource_type".into(),
                )
            })?;
        let events = vm_lifecycle::list_by_resource(&client, &ctx.project_id, rt, resource_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;
        let has_more = false; // resource-scoped views are always complete
        return Ok(Json(VmHistoryResponse { events, has_more }));
    }

    // Project-wide list. Currently uses the unfiltered helper and applies
    // `resource_type` / `from` / `to` in Rust for simplicity — the WHERE
    // clause can move into SQL when the list gets hot enough to matter.
    let events = vm_lifecycle::list_by_project(&client, &ctx.project_id, limit, offset)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")))?;

    let has_more = (events.len() as i64) == limit;

    let filtered: Vec<VmLifecycleRow> = events
        .into_iter()
        .filter(|e| match &q.resource_type {
            Some(rt) => e.resource_type == *rt,
            None => true,
        })
        .filter(|e| match q.from {
            Some(f) => e.event_time >= f,
            None => true,
        })
        .filter(|e| match q.to {
            Some(t) => e.event_time <= t,
            None => true,
        })
        .collect();

    Ok(Json(VmHistoryResponse {
        events: filtered,
        has_more,
    }))
}

fn parse_resource_type(s: &str) -> Option<vm_lifecycle::ResourceType> {
    match s {
        "tester" => Some(vm_lifecycle::ResourceType::Tester),
        "endpoint" => Some(vm_lifecycle::ResourceType::Endpoint),
        "benchmark" => Some(vm_lifecycle::ResourceType::Benchmark),
        _ => None,
    }
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/vm-history", get(list_vm_history))
        .with_state(state)
}
