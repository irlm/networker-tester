use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct BenchmarkConfigRow {
    pub config_id: Uuid,
    pub project_id: String,
    pub name: String,
    pub template: Option<String>,
    pub status: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub config_json: serde_json::Value,
    pub error_message: Option<String>,
    pub max_duration_secs: i32,
    pub baseline_run_id: Option<Uuid>,
    pub worker_id: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub benchmark_type: String,
}

fn row_to_config(r: &tokio_postgres::Row) -> BenchmarkConfigRow {
    BenchmarkConfigRow {
        config_id: r.get("config_id"),
        project_id: r.get("project_id"),
        name: r.get("name"),
        template: r.get("template"),
        status: r.get("status"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        config_json: r.get("config_json"),
        error_message: r.get("error_message"),
        max_duration_secs: r.get("max_duration_secs"),
        baseline_run_id: r.get("baseline_run_id"),
        worker_id: r.get("worker_id"),
        last_heartbeat: r.get("last_heartbeat"),
        benchmark_type: r.get("benchmark_type"),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    project_id: &str,
    name: &str,
    template: Option<&str>,
    config_json: &serde_json::Value,
    created_by: Option<&Uuid>,
    max_duration_secs: i32,
    baseline_run_id: Option<&Uuid>,
    benchmark_type: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO benchmark_config
                (config_id, project_id, name, template, config_json, created_by,
                 max_duration_secs, baseline_run_id, benchmark_type)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &id,
                &project_id,
                &name,
                &template,
                config_json,
                &created_by,
                &max_duration_secs,
                &baseline_run_id,
                &benchmark_type,
            ],
        )
        .await?;
    Ok(id)
}

pub async fn get(client: &Client, config_id: &Uuid) -> anyhow::Result<Option<BenchmarkConfigRow>> {
    let row = client
        .query_opt(
            "SELECT config_id, project_id, name, template, status, created_by,
                    created_at, started_at, finished_at, config_json, error_message,
                    max_duration_secs, baseline_run_id, worker_id, last_heartbeat, benchmark_type
             FROM benchmark_config WHERE config_id = $1",
            &[config_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_config))
}

pub async fn list(
    client: &Client,
    project_id: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<BenchmarkConfigRow>> {
    let rows = client
        .query(
            "SELECT config_id, project_id, name, template, status, created_by,
                    created_at, started_at, finished_at, config_json, error_message,
                    max_duration_secs, baseline_run_id, worker_id, last_heartbeat, benchmark_type
             FROM benchmark_config WHERE project_id = $1
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            &[&project_id, &limit, &offset],
        )
        .await?;
    Ok(rows.iter().map(row_to_config).collect())
}

pub async fn update_status(
    client: &Client,
    config_id: &Uuid,
    status: &str,
    error_message: Option<&str>,
) -> anyhow::Result<()> {
    let started: Option<DateTime<Utc>> = match status {
        "running" | "provisioning" | "deploying" => Some(Utc::now()),
        _ => None,
    };
    let finished: Option<DateTime<Utc>> = match status {
        "completed" | "failed" | "cancelled" => Some(Utc::now()),
        _ => None,
    };
    client
        .execute(
            "UPDATE benchmark_config SET status = $1,
                error_message = COALESCE($2, error_message),
                started_at = COALESCE($3, started_at),
                finished_at = COALESCE($4, finished_at)
             WHERE config_id = $5",
            &[&status, &error_message, &started, &finished, config_id],
        )
        .await?;
    Ok(())
}

/// Atomically claim a queued config for a worker. Returns the config if claimed.
pub async fn claim_queued(
    client: &Client,
    worker_id: &str,
) -> anyhow::Result<Option<BenchmarkConfigRow>> {
    let row = client
        .query_opt(
            "UPDATE benchmark_config
             SET status = 'running', worker_id = $1, started_at = now(), last_heartbeat = now()
             WHERE config_id = (
                 SELECT config_id FROM benchmark_config
                 WHERE status = 'queued'
                 ORDER BY created_at ASC
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING config_id, project_id, name, template, status, created_by,
                       created_at, started_at, finished_at, config_json, error_message,
                       max_duration_secs, baseline_run_id, worker_id, last_heartbeat, benchmark_type",
            &[&worker_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_config))
}

pub async fn update_heartbeat(client: &Client, config_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_config SET last_heartbeat = now() WHERE config_id = $1",
            &[config_id],
        )
        .await?;
    Ok(())
}

/// Find configs that are running but have not received a heartbeat in the given minutes.
pub async fn find_stalled(
    client: &Client,
    stale_minutes: i32,
) -> anyhow::Result<Vec<BenchmarkConfigRow>> {
    let rows = client
        .query(
            "SELECT config_id, project_id, name, template, status, created_by,
                    created_at, started_at, finished_at, config_json, error_message,
                    max_duration_secs, baseline_run_id, worker_id, last_heartbeat, benchmark_type
             FROM benchmark_config
             WHERE status = 'running'
               AND last_heartbeat < now() - ($1::text || ' minutes')::interval",
            &[&stale_minutes.to_string()],
        )
        .await?;
    Ok(rows.iter().map(row_to_config).collect())
}
