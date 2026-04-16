//! CRUD for the unified `test_config` table.
//!
//! Polymorphic fields (`endpoint_ref`, `workload`, `methodology`) are stored
//! as JSONB and deserialized into the strongly-typed
//! [`networker_common::TestConfig`] family at the DB boundary.

use chrono::{DateTime, Utc};
use networker_common::{EndpointRef, Methodology, TestConfig, Workload};
use tokio_postgres::Client;
use uuid::Uuid;

/// Arguments for inserting a fresh `test_config` row. The DB assigns
/// `id`, `created_at`, and `updated_at`.
#[derive(Debug, Clone)]
pub struct NewTestConfig<'a> {
    pub project_id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub endpoint: &'a EndpointRef,
    pub workload: &'a Workload,
    pub methodology: Option<&'a Methodology>,
    pub max_duration_secs: u32,
    pub created_by: Option<&'a Uuid>,
}

/// Insert a new `test_config` and return the full row populated by the DB.
pub async fn create(client: &Client, new: &NewTestConfig<'_>) -> anyhow::Result<TestConfig> {
    let endpoint_kind = match new.endpoint {
        EndpointRef::Network { .. } => "network",
        EndpointRef::Proxy { .. } => "proxy",
        EndpointRef::Runtime { .. } => "runtime",
        EndpointRef::Pending { .. } => "pending",
    };
    let endpoint_ref = serde_json::to_value(new.endpoint)?;
    let workload = serde_json::to_value(new.workload)?;
    let methodology: Option<serde_json::Value> =
        new.methodology.map(serde_json::to_value).transpose()?;
    let max_duration = new.max_duration_secs as i32;

    let row = client
        .query_one(
            "INSERT INTO test_config
                (project_id, name, description, endpoint_kind, endpoint_ref,
                 workload, methodology, max_duration_secs, created_by)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
             RETURNING id, project_id, name, description, endpoint_kind,
                       endpoint_ref, workload, methodology,
                       baseline_run_id, max_duration_secs,
                       created_by, created_at, updated_at",
            &[
                &new.project_id,
                &new.name,
                &new.description,
                &endpoint_kind,
                &endpoint_ref,
                &workload,
                &methodology,
                &max_duration,
                &new.created_by,
            ],
        )
        .await?;

    row_to_config(&row)
}

/// Fetch a single `test_config` by id. Returns `None` if not found.
pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<TestConfig>> {
    let row = client
        .query_opt(
            "SELECT id, project_id, name, description, endpoint_kind,
                    endpoint_ref, workload, methodology,
                    baseline_run_id, max_duration_secs,
                    created_by, created_at, updated_at
             FROM test_config WHERE id = $1",
            &[id],
        )
        .await?;

    row.as_ref().map(row_to_config).transpose()
}

/// List `test_config` rows for a project, newest first.
pub async fn list(
    client: &Client,
    project_id: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<TestConfig>> {
    let rows = client
        .query(
            "SELECT id, project_id, name, description, endpoint_kind,
                    endpoint_ref, workload, methodology,
                    baseline_run_id, max_duration_secs,
                    created_by, created_at, updated_at
             FROM test_config
             WHERE project_id = $1
             ORDER BY created_at DESC
             LIMIT $2 OFFSET $3",
            &[&project_id, &limit, &offset],
        )
        .await?;

    rows.iter().map(row_to_config).collect()
}

/// Partial update. `None` means "leave unchanged"; `Some(x)` means "set to x".
#[derive(Debug, Default, Clone)]
pub struct UpdateTestConfig<'a> {
    pub name: Option<&'a str>,
    pub description: Option<Option<&'a str>>,
    pub endpoint: Option<&'a EndpointRef>,
    pub workload: Option<&'a Workload>,
    pub methodology: Option<Option<&'a Methodology>>,
    pub baseline_run_id: Option<Option<Uuid>>,
    pub max_duration_secs: Option<u32>,
}

/// Apply a partial update and return the refreshed row.
///
/// Uses a single UPDATE with COALESCE-like semantics expressed as `$n IS NULL`
/// guards over separate scalar params. Keeps the SQL readable and avoids
/// dynamic string building.
pub async fn update(
    client: &Client,
    id: &Uuid,
    patch: &UpdateTestConfig<'_>,
) -> anyhow::Result<Option<TestConfig>> {
    // Serialize polymorphic fields ahead of the query so we can bind them.
    let endpoint_kind = patch.endpoint.map(|e| match e {
        EndpointRef::Network { .. } => "network",
        EndpointRef::Proxy { .. } => "proxy",
        EndpointRef::Runtime { .. } => "runtime",
        EndpointRef::Pending { .. } => "pending",
    });
    let endpoint_ref: Option<serde_json::Value> =
        patch.endpoint.map(serde_json::to_value).transpose()?;
    let workload: Option<serde_json::Value> =
        patch.workload.map(serde_json::to_value).transpose()?;

    // methodology: Option<Option<&Methodology>>
    //   None          → leave unchanged (sentinel: methodology_set = false)
    //   Some(None)    → clear to NULL
    //   Some(Some(m)) → set to JSONB of m
    let methodology_set = patch.methodology.is_some();
    let methodology_val: Option<serde_json::Value> = match patch.methodology {
        Some(Some(m)) => Some(serde_json::to_value(m)?),
        _ => None,
    };

    let baseline_set = patch.baseline_run_id.is_some();
    let baseline_val: Option<Uuid> = patch.baseline_run_id.flatten();

    let description_set = patch.description.is_some();
    let description_val: Option<&str> = patch.description.flatten();

    let max_dur: Option<i32> = patch.max_duration_secs.map(|v| v as i32);

    let row = client
        .query_opt(
            "UPDATE test_config
             SET name              = COALESCE($2, name),
                 description       = CASE WHEN $3 THEN $4 ELSE description END,
                 endpoint_kind     = COALESCE($5, endpoint_kind),
                 endpoint_ref      = COALESCE($6, endpoint_ref),
                 workload          = COALESCE($7, workload),
                 methodology       = CASE WHEN $8 THEN $9 ELSE methodology END,
                 baseline_run_id   = CASE WHEN $10 THEN $11 ELSE baseline_run_id END,
                 max_duration_secs = COALESCE($12, max_duration_secs),
                 updated_at        = now()
             WHERE id = $1
             RETURNING id, project_id, name, description, endpoint_kind,
                       endpoint_ref, workload, methodology,
                       baseline_run_id, max_duration_secs,
                       created_by, created_at, updated_at",
            &[
                id,
                &patch.name,
                &description_set,
                &description_val,
                &endpoint_kind,
                &endpoint_ref,
                &workload,
                &methodology_set,
                &methodology_val,
                &baseline_set,
                &baseline_val,
                &max_dur,
            ],
        )
        .await?;

    row.as_ref().map(row_to_config).transpose()
}

/// Rewrite only the endpoint on an existing config. Used by the provisioning
/// orchestrator when it resolves a `Pending` endpoint to a concrete
/// `Network { host, port }` after the deployment lands.
pub async fn update_endpoint(
    client: &Client,
    id: &Uuid,
    endpoint: &EndpointRef,
) -> anyhow::Result<()> {
    let kind = match endpoint {
        EndpointRef::Network { .. } => "network",
        EndpointRef::Proxy { .. } => "proxy",
        EndpointRef::Runtime { .. } => "runtime",
        EndpointRef::Pending { .. } => "pending",
    };
    let value = serde_json::to_value(endpoint)?;
    client
        .execute(
            "UPDATE test_config
             SET endpoint_kind = $2,
                 endpoint_ref = $3,
                 updated_at = now()
             WHERE id = $1",
            &[id, &kind, &value],
        )
        .await?;
    Ok(())
}

/// Delete a `test_config`. Returns `true` if a row was deleted.
pub async fn delete(client: &Client, id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute("DELETE FROM test_config WHERE id = $1", &[id])
        .await?;
    Ok(n > 0)
}

// ── helpers ─────────────────────────────────────────────────────────────
fn row_to_config(r: &tokio_postgres::Row) -> anyhow::Result<TestConfig> {
    let endpoint_ref: serde_json::Value = r.get("endpoint_ref");
    let workload: serde_json::Value = r.get("workload");
    let methodology: Option<serde_json::Value> = r.get("methodology");
    let max_duration: i32 = r.get("max_duration_secs");
    let created_at: DateTime<Utc> = r.get("created_at");
    let updated_at: DateTime<Utc> = r.get("updated_at");

    Ok(TestConfig {
        id: r.get("id"),
        project_id: r.get("project_id"),
        name: r.get("name"),
        description: r.get("description"),
        endpoint: serde_json::from_value(endpoint_ref)?,
        workload: serde_json::from_value(workload)?,
        methodology: methodology.map(serde_json::from_value).transpose()?,
        baseline_run_id: r.get("baseline_run_id"),
        max_duration_secs: max_duration.max(0) as u32,
        created_by: r.get("created_by"),
        created_at,
        updated_at,
    })
}
