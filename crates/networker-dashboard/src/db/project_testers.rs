//! CRUD for the `project_tester` table (V027).
//!
//! This module defines the row struct plus basic list/get/insert/delete
//! helpers. Lifecycle transitions (locking, power state, etc.) live in
//! `tester_state` — keep this module focused on raw persistence.
//!
//! REST handlers that consume these helpers land in a later task, so
//! suppress dead-code warnings until the wiring is in place.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::{Client, Row};
use uuid::Uuid;

/// Canonical column list for `SELECT`/`RETURNING` clauses. Keep the order
/// in sync with [`ProjectTesterRow::from_row`].
pub const SELECT_COLUMNS: &str = "tester_id, project_id, name, cloud, region, vm_size, \
    vm_name, vm_resource_id, public_ip, ssh_user, \
    power_state, allocation, status_message, locked_by_config_id, \
    installer_version, last_installed_at, \
    auto_shutdown_enabled, auto_shutdown_local_hour, next_shutdown_at, shutdown_deferral_count, \
    auto_probe_enabled, \
    last_used_at, avg_benchmark_duration_seconds, benchmark_run_count, \
    created_by, created_at, updated_at, \
    cloud_connection_id";

/// A single row from the `project_tester` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectTesterRow {
    pub tester_id: Uuid,
    pub project_id: String,
    pub name: String,
    pub cloud: String,
    pub region: String,
    pub vm_size: String,
    pub vm_name: Option<String>,
    pub vm_resource_id: Option<String>,
    /// Stored as INET in Postgres; surfaced as a string for API parity.
    pub public_ip: Option<String>,
    pub ssh_user: String,
    pub power_state: String,
    pub allocation: String,
    pub status_message: Option<String>,
    pub locked_by_config_id: Option<Uuid>,
    pub installer_version: Option<String>,
    pub last_installed_at: Option<DateTime<Utc>>,
    pub auto_shutdown_enabled: bool,
    pub auto_shutdown_local_hour: i16,
    pub next_shutdown_at: Option<DateTime<Utc>>,
    pub shutdown_deferral_count: i16,
    pub auto_probe_enabled: bool,
    pub last_used_at: Option<DateTime<Utc>>,
    pub avg_benchmark_duration_seconds: Option<i32>,
    pub benchmark_run_count: i32,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub cloud_connection_id: Option<Uuid>,
}

impl ProjectTesterRow {
    /// Decode a row produced by a `SELECT` that used [`SELECT_COLUMNS`].
    pub fn from_row(row: &Row) -> Self {
        let public_ip = row
            .get::<_, Option<std::net::IpAddr>>("public_ip")
            .map(|ip| ip.to_string());

        Self {
            tester_id: row.get("tester_id"),
            project_id: row.get("project_id"),
            name: row.get("name"),
            cloud: row.get("cloud"),
            region: row.get("region"),
            vm_size: row.get("vm_size"),
            vm_name: row.get("vm_name"),
            vm_resource_id: row.get("vm_resource_id"),
            public_ip,
            ssh_user: row.get("ssh_user"),
            power_state: row.get("power_state"),
            allocation: row.get("allocation"),
            status_message: row.get("status_message"),
            locked_by_config_id: row.get("locked_by_config_id"),
            installer_version: row.get("installer_version"),
            last_installed_at: row.get("last_installed_at"),
            auto_shutdown_enabled: row.get("auto_shutdown_enabled"),
            auto_shutdown_local_hour: row.get("auto_shutdown_local_hour"),
            next_shutdown_at: row.get("next_shutdown_at"),
            shutdown_deferral_count: row.get("shutdown_deferral_count"),
            auto_probe_enabled: row.get("auto_probe_enabled"),
            last_used_at: row.get("last_used_at"),
            avg_benchmark_duration_seconds: row.get("avg_benchmark_duration_seconds"),
            benchmark_run_count: row.get("benchmark_run_count"),
            created_by: row.get("created_by"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            cloud_connection_id: row.get("cloud_connection_id"),
        }
    }
}

/// Fields accepted from clients when creating a tester. Everything else
/// (power_state, allocation, timestamps, defaults) comes from the DB.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTesterInput {
    pub name: String,
    pub cloud: String,
    pub region: String,
    #[serde(default)]
    pub vm_size: Option<String>,
    #[serde(default)]
    pub auto_shutdown_local_hour: Option<i16>,
    #[serde(default)]
    pub auto_probe_enabled: Option<bool>,
    #[serde(default)]
    pub cloud_connection_id: Option<Uuid>,
}

/// List all testers belonging to a project, newest first.
pub async fn list_for_project(
    client: &Client,
    project_id: &str,
) -> anyhow::Result<Vec<ProjectTesterRow>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM project_tester \
         WHERE project_id = $1 ORDER BY created_at DESC"
    );
    let rows = client.query(&sql, &[&project_id]).await?;
    Ok(rows.iter().map(ProjectTesterRow::from_row).collect())
}

/// Fetch a single tester, scoped to its project (cross-project lookups
/// return `None` so callers can 404 cleanly).
pub async fn get(
    client: &Client,
    project_id: &str,
    tester_id: &Uuid,
) -> anyhow::Result<Option<ProjectTesterRow>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM project_tester \
         WHERE project_id = $1 AND tester_id = $2"
    );
    let row = client.query_opt(&sql, &[&project_id, tester_id]).await?;
    Ok(row.as_ref().map(ProjectTesterRow::from_row))
}

/// Insert a new tester row, relying on column defaults for state and
/// audit fields. Returns the freshly-created row.
pub async fn insert(
    client: &Client,
    project_id: &str,
    input: &CreateTesterInput,
    created_by: &Uuid,
) -> anyhow::Result<ProjectTesterRow> {
    // Build the insert so that NULL vm_size etc. fall back to the column
    // default defined in V027. We use COALESCE on the parameter for the
    // fields that have non-trivial defaults.
    let sql = format!(
        "INSERT INTO project_tester ( \
             project_id, name, cloud, region, \
             vm_size, \
             auto_shutdown_local_hour, \
             auto_probe_enabled, \
             created_by, \
             cloud_connection_id \
         ) VALUES ( \
             $1, $2, $3, $4, \
             COALESCE($5, 'Standard_D2s_v3'), \
             COALESCE($6, 23::smallint), \
             COALESCE($7, FALSE), \
             $8, \
             $9 \
         ) \
         RETURNING {SELECT_COLUMNS}"
    );

    let row = client
        .query_one(
            &sql,
            &[
                &project_id,
                &input.name,
                &input.cloud,
                &input.region,
                &input.vm_size,
                &input.auto_shutdown_local_hour,
                &input.auto_probe_enabled,
                created_by,
                &input.cloud_connection_id,
            ],
        )
        .await?;

    Ok(ProjectTesterRow::from_row(&row))
}

/// Delete a tester. Returns `true` if a row was actually removed.
pub async fn delete(client: &Client, project_id: &str, tester_id: &Uuid) -> anyhow::Result<bool> {
    let row = client
        .query_opt(
            "DELETE FROM project_tester \
             WHERE project_id = $1 AND tester_id = $2 \
             RETURNING tester_id",
            &[&project_id, tester_id],
        )
        .await?;
    Ok(row.is_some())
}
