//! Persistence helpers for the `vm_lifecycle` and `cost_rate` tables (V034).
//!
//! `vm_lifecycle` is append-only — no update, no delete. Rows outlive the
//! `cloud_connection` they originated from thanks to snapshot columns
//! (`cloud`, `region`, `vm_size`, `cloud_account_name_at_event`), so history
//! survives rename, soft-delete, or even hard-delete of the source account.
//!
//! Design doc: `docs/superpowers/specs/2026-04-15-vm-usage-history-design.md`.
//!
//! Some helpers here will be consumed by REST API handlers landing in the
//! next PR; silence dead-code warnings until then.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

/// Stable enum of lifecycle event kinds. Kept in sync with the SQL
/// `vm_lifecycle_event_type_valid` CHECK constraint in migration V034 —
/// the DB will reject anything not in this list, so if you add a variant
/// here you MUST also extend the CHECK constraint in a new migration.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Created,
    Started,
    Stopped,
    Deleted,
    AutoShutdown,
    Error,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::Created => "created",
            EventType::Started => "started",
            EventType::Stopped => "stopped",
            EventType::Deleted => "deleted",
            EventType::AutoShutdown => "auto_shutdown",
            EventType::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    Tester,
    Endpoint,
    Benchmark,
}

impl ResourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceType::Tester => "tester",
            ResourceType::Endpoint => "endpoint",
            ResourceType::Benchmark => "benchmark",
        }
    }
}

/// Single row of the append-only `vm_lifecycle` table.
#[derive(Debug, Clone, Serialize)]
pub struct VmLifecycleRow {
    pub event_id: Uuid,
    pub project_id: String,

    pub resource_type: String,
    pub resource_id: Uuid,
    pub resource_name: Option<String>,

    pub cloud: String,
    pub region: Option<String>,
    pub vm_size: Option<String>,
    pub vm_name: Option<String>,
    pub vm_resource_id: Option<String>,

    pub cloud_connection_id: Option<Uuid>,
    pub cloud_account_name_at_event: Option<String>,
    pub provider_account_id: Option<String>,

    pub event_type: String,
    pub event_time: DateTime<Utc>,
    pub triggered_by: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,

    pub created_at: DateTime<Utc>,
}

/// Bundled inputs for `record_event`. Snapshot strings are caller-supplied
/// so the DB doesn't have to join to `cloud_connection` on the write path —
/// joins happen later in read queries where they won't block a state change.
#[derive(Debug, Clone)]
pub struct NewEvent<'a> {
    pub project_id: &'a str,
    pub resource_type: ResourceType,
    pub resource_id: Uuid,
    pub resource_name: Option<&'a str>,

    pub cloud: &'a str,
    pub region: Option<&'a str>,
    pub vm_size: Option<&'a str>,
    pub vm_name: Option<&'a str>,
    pub vm_resource_id: Option<&'a str>,

    pub cloud_connection_id: Option<Uuid>,
    pub cloud_account_name_at_event: Option<&'a str>,
    pub provider_account_id: Option<&'a str>,

    pub event_type: EventType,
    pub event_time: DateTime<Utc>,
    pub triggered_by: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

fn row_to_record(r: &tokio_postgres::Row) -> VmLifecycleRow {
    VmLifecycleRow {
        event_id: r.get("event_id"),
        project_id: r.get("project_id"),
        resource_type: r.get("resource_type"),
        resource_id: r.get("resource_id"),
        resource_name: r.get("resource_name"),
        cloud: r.get("cloud"),
        region: r.get("region"),
        vm_size: r.get("vm_size"),
        vm_name: r.get("vm_name"),
        vm_resource_id: r.get("vm_resource_id"),
        cloud_connection_id: r.get("cloud_connection_id"),
        cloud_account_name_at_event: r.get("cloud_account_name_at_event"),
        provider_account_id: r.get("provider_account_id"),
        event_type: r.get("event_type"),
        event_time: r.get("event_time"),
        triggered_by: r.get("triggered_by"),
        metadata: r.get("metadata"),
        created_at: r.get("created_at"),
    }
}

/// Append an event row. Returns the generated `event_id`.
///
/// Accepts a `&Client` rather than a `&Transaction` because most callers
/// emit events from paths that aren't already inside a transaction — the
/// state change is a provider-side side-effect (az vm create, etc.) and
/// the event row just records that it happened. Callers that ARE inside
/// a transaction (future: DB-only state flips) should use
/// `insert_within_tx` instead once that helper lands.
pub async fn insert(client: &Client, event: &NewEvent<'_>) -> anyhow::Result<Uuid> {
    let row = client
        .query_one(
            "INSERT INTO vm_lifecycle (
                project_id, resource_type, resource_id, resource_name,
                cloud, region, vm_size, vm_name, vm_resource_id,
                cloud_connection_id, cloud_account_name_at_event, provider_account_id,
                event_type, event_time, triggered_by, metadata
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7, $8, $9,
                $10, $11, $12,
                $13, $14, $15, $16
            )
            RETURNING event_id",
            &[
                &event.project_id,
                &event.resource_type.as_str(),
                &event.resource_id,
                &event.resource_name,
                &event.cloud,
                &event.region,
                &event.vm_size,
                &event.vm_name,
                &event.vm_resource_id,
                &event.cloud_connection_id,
                &event.cloud_account_name_at_event,
                &event.provider_account_id,
                &event.event_type.as_str(),
                &event.event_time,
                &event.triggered_by,
                &event.metadata,
            ],
        )
        .await?;
    Ok(row.get("event_id"))
}

/// List events for a project, newest first. `limit` is clamped by the
/// caller; this helper applies no bounds of its own.
pub async fn list_by_project(
    client: &Client,
    project_id: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<VmLifecycleRow>> {
    let rows = client
        .query(
            "SELECT * FROM vm_lifecycle
             WHERE project_id = $1
             ORDER BY event_time DESC, event_id DESC
             LIMIT $2 OFFSET $3",
            &[&project_id, &limit, &offset],
        )
        .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

/// List events for a single resource, oldest first — matches the natural
/// display order for a timeline view.
pub async fn list_by_resource(
    client: &Client,
    project_id: &str,
    resource_type: ResourceType,
    resource_id: Uuid,
) -> anyhow::Result<Vec<VmLifecycleRow>> {
    let rows = client
        .query(
            "SELECT * FROM vm_lifecycle
             WHERE project_id = $1 AND resource_type = $2 AND resource_id = $3
             ORDER BY event_time ASC, event_id ASC",
            &[&project_id, &resource_type.as_str(), &resource_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

/// Lookup the effective cost rate for a given (cloud, vm_size) at a
/// specific instant. Prefers a region-specific row when one exists,
/// otherwise falls back to the region-NULL flat rate. Returns `None` when
/// no matching row exists — callers should treat that as an unpriceable
/// event and skip the cost column rather than fail the write.
pub async fn lookup_rate(
    client: &Client,
    cloud: &str,
    vm_size: &str,
    region: Option<&str>,
    at: DateTime<Utc>,
) -> anyhow::Result<Option<f64>> {
    // Cast to DOUBLE PRECISION so tokio-postgres can decode as f64 without
    // pulling in rust_decimal. Precision loss on a 6-decimal rate is well
    // below the billing noise floor (sub-cent per hour), and the UI only
    // displays two decimal places anyway.
    let row = client
        .query_opt(
            "SELECT rate_per_hour_usd::double precision AS rate FROM cost_rate
             WHERE cloud = $1
               AND vm_size = $2
               AND (region = $3 OR region IS NULL)
               AND effective_from <= $4
               AND (effective_to IS NULL OR effective_to > $4)
             ORDER BY
                CASE WHEN region IS NOT NULL AND region = $3 THEN 0 ELSE 1 END,
                effective_from DESC
             LIMIT 1",
            &[&cloud, &vm_size, &region, &at],
        )
        .await?;
    Ok(row.map(|r| r.get::<_, f64>("rate")))
}
