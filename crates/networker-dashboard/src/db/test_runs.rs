//! CRUD for the unified `test_run` table.
//!
//! One row per execution of a `test_config`. When the config carried a
//! `Methodology`, a `benchmark_artifact` row is attached via `artifact_id`.

use chrono::{DateTime, Utc};
use networker_common::{RunStatus, TestRun};
use tokio_postgres::Client;
use uuid::Uuid;

/// Arguments for queueing a new `test_run`.
#[derive(Debug, Clone)]
pub struct NewTestRun<'a> {
    pub test_config_id: &'a Uuid,
    pub project_id: &'a str,
    pub tester_id: Option<&'a Uuid>,
    pub worker_id: Option<&'a str>,
    pub comparison_group_id: Option<&'a Uuid>,
}

/// Queue a new run (status = queued). Returns the full row.
pub async fn create(client: &Client, new: &NewTestRun<'_>) -> anyhow::Result<TestRun> {
    let row = client
        .query_one(
            "INSERT INTO test_run
                (test_config_id, project_id, status, tester_id, worker_id, comparison_group_id)
             VALUES ($1,$2,'queued',$3,$4,$5)
             RETURNING id, test_config_id, project_id, status,
                       started_at, finished_at,
                       success_count, failure_count, error_message,
                       artifact_id, tester_id, worker_id,
                       last_heartbeat, created_at, comparison_group_id",
            &[
                &new.test_config_id,
                &new.project_id,
                &new.tester_id,
                &new.worker_id,
                &new.comparison_group_id,
            ],
        )
        .await?;

    row_to_run(&row)
}

pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<TestRun>> {
    let row = client
        .query_opt(
            "SELECT id, test_config_id, project_id, status,
                    started_at, finished_at,
                    success_count, failure_count, error_message,
                    artifact_id, tester_id, worker_id,
                    last_heartbeat, created_at, comparison_group_id
             FROM test_run WHERE id = $1",
            &[id],
        )
        .await?;

    row.as_ref().map(row_to_run).transpose()
}

/// List runs for a project, newest first, with optional filters.
pub async fn list(
    client: &Client,
    project_id: &str,
    status_filter: Option<RunStatus>,
    has_artifact: Option<bool>,
    comparison_group_id: Option<&Uuid>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<TestRun>> {
    // Build the WHERE clause around the fixed params; keep the static part
    // parameterised, append the dynamic predicates.
    let mut sql = String::from(
        "SELECT id, test_config_id, project_id, status,
                started_at, finished_at,
                success_count, failure_count, error_message,
                artifact_id, tester_id, worker_id,
                last_heartbeat, created_at, comparison_group_id
         FROM test_run WHERE project_id = $1",
    );
    let status_str = status_filter.map(|s| s.as_str().to_string());
    let mut idx: usize = 2;
    if status_str.is_some() {
        sql.push_str(&format!(" AND status = ${idx}"));
        idx += 1;
    }
    if let Some(flag) = has_artifact {
        if flag {
            sql.push_str(" AND artifact_id IS NOT NULL");
        } else {
            sql.push_str(" AND artifact_id IS NULL");
        }
    }
    if comparison_group_id.is_some() {
        sql.push_str(&format!(" AND comparison_group_id = ${idx}"));
        idx += 1;
    }
    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
        idx,
        idx + 1
    ));

    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    params.push(&project_id);
    if let Some(s) = &status_str {
        params.push(s);
    }
    if let Some(g) = comparison_group_id {
        params.push(g);
    }
    params.push(&limit);
    params.push(&offset);

    let rows = client.query(&sql, &params).await?;
    rows.iter().map(row_to_run).collect()
}

/// Transition status; stamps started_at on running and finished_at on terminal.
pub async fn update_status(
    client: &Client,
    id: &Uuid,
    status: RunStatus,
) -> anyhow::Result<Option<TestRun>> {
    let status_str = status.as_str();
    let now = Utc::now();
    let started: Option<DateTime<Utc>> = if status == RunStatus::Running {
        Some(now)
    } else {
        None
    };
    let finished: Option<DateTime<Utc>> = if status.is_terminal() {
        Some(now)
    } else {
        None
    };

    let row = client
        .query_opt(
            "UPDATE test_run
             SET status      = $2,
                 started_at  = COALESCE($3, started_at),
                 finished_at = COALESCE($4, finished_at)
             WHERE id = $1
             RETURNING id, test_config_id, project_id, status,
                       started_at, finished_at,
                       success_count, failure_count, error_message,
                       artifact_id, tester_id, worker_id,
                       last_heartbeat, created_at, comparison_group_id",
            &[id, &status_str, &started, &finished],
        )
        .await?;
    row.as_ref().map(row_to_run).transpose()
}

/// Bump the aggregate success/failure counters as attempts stream in.
pub async fn update_counts(
    client: &Client,
    id: &Uuid,
    success: u32,
    failure: u32,
) -> anyhow::Result<()> {
    let s = success as i32;
    let f = failure as i32;
    client
        .execute(
            "UPDATE test_run
             SET success_count = $2,
                 failure_count = $3,
                 last_heartbeat = now()
             WHERE id = $1",
            &[id, &s, &f],
        )
        .await?;
    Ok(())
}

/// Record a terminal error. Sets status=failed and finished_at=now.
pub async fn set_error(client: &Client, id: &Uuid, message: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE test_run
             SET status = 'failed',
                 error_message = $2,
                 finished_at = now()
             WHERE id = $1",
            &[id, &message],
        )
        .await?;
    Ok(())
}

/// Transition a freshly-created run into `provisioning` and link it to the
/// deployment whose completion unblocks it. Called by the provisioning
/// orchestrator at dispatch time for `EndpointRef::Pending` configs.
pub async fn set_provisioning(
    client: &Client,
    run_id: &Uuid,
    deployment_id: &Uuid,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE test_run
             SET status = 'provisioning',
                 provisioning_deployment_id = $2
             WHERE id = $1",
            &[run_id, deployment_id],
        )
        .await?;
    Ok(())
}

/// Every run that's currently waiting on a deployment to come up. Returns
/// `(run, deployment_id)` pairs so the orchestrator can check deployment
/// status in one pass.
pub async fn list_provisioning(client: &Client) -> anyhow::Result<Vec<(TestRun, Uuid)>> {
    let rows = client
        .query(
            "SELECT id, test_config_id, project_id, status,
                    started_at, finished_at,
                    success_count, failure_count, error_message,
                    artifact_id, tester_id, worker_id,
                    last_heartbeat, created_at, comparison_group_id,
                    provisioning_deployment_id
             FROM test_run
             WHERE status = 'provisioning'
               AND provisioning_deployment_id IS NOT NULL",
            &[],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let run = row_to_run(row)?;
        let dep_id: Uuid = row.get("provisioning_deployment_id");
        out.push((run, dep_id));
    }
    Ok(out)
}

/// Attach an artifact id to a completed run (methodology mode only).
#[allow(dead_code)]
pub async fn attach_artifact(client: &Client, id: &Uuid, artifact_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE test_run SET artifact_id = $2 WHERE id = $1",
            &[id, artifact_id],
        )
        .await?;
    Ok(())
}

#[allow(dead_code)]
pub async fn delete(client: &Client, id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute("DELETE FROM test_run WHERE id = $1", &[id])
        .await?;
    Ok(n > 0)
}

// ── helpers ─────────────────────────────────────────────────────────────
fn row_to_run(r: &tokio_postgres::Row) -> anyhow::Result<TestRun> {
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
