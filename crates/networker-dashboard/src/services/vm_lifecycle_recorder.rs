//! Lib-side recorder for `vm_lifecycle` events.
//!
//! Lives in `services/` rather than `db/` because auto-shutdown and error
//! paths run from inside service modules (`tester_scheduler`,
//! `tester_state`), which are in the lib crate and can't see the bin-side
//! `db::vm_lifecycle` module due to the dashboard's lib/bin split.
//!
//! Uses raw SQL so it carries no struct dependencies on bin-side types
//! (`ProjectTesterRow` et al). Callers pass primitives directly — all
//! snapshot columns are string slices so we don't have to mirror row types
//! here either.
//!
//! `insert_event` mirrors the public contract of the bin-side
//! `db::vm_lifecycle::insert` helper: failures log at WARN and are
//! swallowed, since a transient audit-table issue must not surface as a
//! user-visible failure on the state change that triggered it.
//!
//! Event-type + resource-type strings here MUST stay in sync with the DB
//! CHECK constraints (`vm_lifecycle_event_type_valid`,
//! `vm_lifecycle_resource_type_valid`) added in migration V034, and with
//! the bin-side `db::vm_lifecycle::EventType` / `ResourceType` enums.

use chrono::{DateTime, Utc};
use tokio_postgres::Client;
use uuid::Uuid;

/// Minimal snapshot inputs for a lifecycle insert.
///
/// Intentionally primitive-typed so lib-side callers need no bin types to
/// use it. Matches the columns written by `db::vm_lifecycle::insert` so
/// rows produced by either path are indistinguishable on read.
#[derive(Debug, Clone)]
#[allow(clippy::too_many_arguments)]
pub struct TesterEventInput<'a> {
    pub project_id: &'a str,
    pub tester_id: Uuid,
    pub tester_name: &'a str,
    pub cloud: &'a str,
    pub region: &'a str,
    pub vm_size: &'a str,
    pub vm_name: Option<&'a str>,
    pub vm_resource_id: Option<&'a str>,
    pub cloud_connection_id: Option<Uuid>,
    pub event_type: &'a str,
    pub event_time: DateTime<Utc>,
    pub triggered_by: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

/// Append a `vm_lifecycle` row. `event_type` must be one of
/// `created | started | stopped | deleted | auto_shutdown | error` — the
/// DB CHECK constraint will reject anything else.
///
/// Never returns an error from the caller's perspective; logs at WARN on
/// failure so a transient DB issue can't crash the scheduler loop.
pub async fn insert_tester_event<'a>(client: &Client, event: TesterEventInput<'a>) {
    let result = client
        .execute(
            "INSERT INTO vm_lifecycle (
                project_id, resource_type, resource_id, resource_name,
                cloud, region, vm_size, vm_name, vm_resource_id,
                cloud_connection_id, event_type, event_time, triggered_by, metadata
            ) VALUES (
                $1, 'tester', $2, $3,
                $4, $5, $6, $7, $8,
                $9, $10, $11, $12, $13
            )",
            &[
                &event.project_id,
                &event.tester_id,
                &event.tester_name,
                &event.cloud,
                &event.region,
                &event.vm_size,
                &event.vm_name,
                &event.vm_resource_id,
                &event.cloud_connection_id,
                &event.event_type,
                &event.event_time,
                &event.triggered_by,
                &event.metadata,
            ],
        )
        .await;
    if let Err(e) = result {
        tracing::warn!(
            tester_id = %event.tester_id,
            event_type = %event.event_type,
            error = %e,
            "failed to append vm_lifecycle event from lib-side path (history incomplete, user-facing op unaffected)"
        );
    }
}
