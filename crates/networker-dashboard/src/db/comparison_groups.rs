//! CRUD for the `comparison_group` table.
//!
//! A comparison group bundles N cells that share a common `base_workload`
//! (and optional `methodology`). Each cell becomes a `test_config` +
//! `test_run` pair linked back via `test_run.comparison_group_id`.

use chrono::{DateTime, Utc};
use networker_common::{ComparisonCell, ComparisonGroup, Methodology, TestRun, Workload};
use tokio_postgres::Client;
use uuid::Uuid;

/// Insert a new comparison group. Returns the generated id.
pub async fn create(
    client: &Client,
    project_id: &str,
    name: &str,
    base_workload: &Workload,
    methodology: Option<&Methodology>,
    cells: &[ComparisonCell],
    created_by: Option<&Uuid>,
) -> anyhow::Result<Uuid> {
    let workload_json = serde_json::to_value(base_workload)?;
    let methodology_json: Option<serde_json::Value> =
        methodology.map(serde_json::to_value).transpose()?;
    let cells_json = serde_json::to_value(cells)?;

    let row = client
        .query_one(
            "INSERT INTO comparison_group
                (project_id, name, base_workload, methodology, cells, created_by)
             VALUES ($1,$2,$3,$4,$5,$6)
             RETURNING id",
            &[
                &project_id,
                &name,
                &workload_json,
                &methodology_json,
                &cells_json,
                &created_by,
            ],
        )
        .await?;

    Ok(row.get("id"))
}

/// Fetch a single comparison group by id.
pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<ComparisonGroup>> {
    let row = client
        .query_opt(
            "SELECT id, project_id, name, base_workload, methodology,
                    cells, status, created_by, created_at
             FROM comparison_group WHERE id = $1",
            &[id],
        )
        .await?;

    row.as_ref().map(row_to_group).transpose()
}

/// List comparison groups for a project, newest first.
pub async fn list(client: &Client, project_id: &str) -> anyhow::Result<Vec<ComparisonGroup>> {
    let rows = client
        .query(
            "SELECT id, project_id, name, base_workload, methodology,
                    cells, status, created_by, created_at
             FROM comparison_group
             WHERE project_id = $1
             ORDER BY created_at DESC",
            &[&project_id],
        )
        .await?;

    rows.iter().map(row_to_group).collect()
}

/// Update the status of a comparison group.
pub async fn update_status(client: &Client, id: &Uuid, status: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE comparison_group SET status = $2 WHERE id = $1",
            &[id, &status],
        )
        .await?;
    Ok(())
}

/// Get all test_runs linked to a comparison group.
pub async fn get_runs(client: &Client, group_id: &Uuid) -> anyhow::Result<Vec<TestRun>> {
    let rows = client
        .query(
            "SELECT id, test_config_id, project_id, status,
                    started_at, finished_at,
                    success_count, failure_count, error_message,
                    artifact_id, tester_id, worker_id,
                    last_heartbeat, created_at, comparison_group_id
             FROM test_run
             WHERE comparison_group_id = $1
             ORDER BY created_at",
            &[group_id],
        )
        .await?;

    rows.iter().map(row_to_run).collect()
}

// ── helpers ─────────────────────────────────────────────────────────────

fn row_to_group(r: &tokio_postgres::Row) -> anyhow::Result<ComparisonGroup> {
    let workload_json: serde_json::Value = r.get("base_workload");
    let methodology_json: Option<serde_json::Value> = r.get("methodology");
    let cells_json: serde_json::Value = r.get("cells");
    let created_at: DateTime<Utc> = r.get("created_at");

    Ok(ComparisonGroup {
        id: r.get("id"),
        project_id: r.get("project_id"),
        name: r.get("name"),
        base_workload: serde_json::from_value(workload_json)?,
        methodology: methodology_json.map(serde_json::from_value).transpose()?,
        cells: serde_json::from_value(cells_json)?,
        status: r.get("status"),
        created_by: r.get("created_by"),
        created_at,
    })
}

fn row_to_run(r: &tokio_postgres::Row) -> anyhow::Result<TestRun> {
    use networker_common::RunStatus;

    let status_str: String = r.get("status");
    let status = RunStatus::parse_str(&status_str)
        .ok_or_else(|| anyhow::anyhow!("invalid run status: {status_str}"))?;
    let success: i32 = r.get("success_count");
    let failure: i32 = r.get("failure_count");

    Ok(TestRun {
        id: r.get("id"),
        test_config_id: r.get("test_config_id"),
        project_id: r.get("project_id"),
        status,
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        success_count: success.max(0) as u32,
        failure_count: failure.max(0) as u32,
        error_message: r.get("error_message"),
        artifact_id: r.get("artifact_id"),
        tester_id: r.get("tester_id"),
        worker_id: r.get("worker_id"),
        last_heartbeat: r.get("last_heartbeat"),
        created_at: r.get("created_at"),
        comparison_group_id: r.get("comparison_group_id"),
    })
}
