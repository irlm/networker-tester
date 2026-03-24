use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct JobRow {
    pub job_id: Uuid,
    pub definition_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub status: String,
    pub config: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub run_id: Option<Uuid>,
    pub error_message: Option<String>,
}

pub async fn create(
    client: &Client,
    config: &serde_json::Value,
    agent_id: Option<&Uuid>,
    created_by: Option<&Uuid>,
    project_id: &Uuid,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO job (job_id, agent_id, status, config, created_by, project_id)
             VALUES ($1, $2, 'pending', $3, $4, $5)",
            &[&id, &agent_id, config, &created_by, project_id],
        )
        .await?;
    Ok(id)
}

pub async fn get(client: &Client, job_id: &Uuid) -> anyhow::Result<Option<JobRow>> {
    let row = client
        .query_opt(
            "SELECT job_id, definition_id, agent_id, status, config, created_by,
                    created_at, started_at, finished_at, run_id, error_message
             FROM job WHERE job_id = $1",
            &[job_id],
        )
        .await?;

    Ok(row.map(|r| JobRow {
        job_id: r.get("job_id"),
        definition_id: r.get("definition_id"),
        agent_id: r.get("agent_id"),
        status: r.get("status"),
        config: r.get("config"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        run_id: r.get("run_id"),
        error_message: r.get("error_message"),
    }))
}

pub async fn list(
    client: &Client,
    project_id: &Uuid,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<JobRow>> {
    list_filtered(client, project_id, status_filter, limit, offset, None).await
}

pub async fn list_filtered(
    client: &Client,
    project_id: &Uuid,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
    visible_ids: Option<&std::collections::HashSet<uuid::Uuid>>,
) -> anyhow::Result<Vec<JobRow>> {
    // If visibility filtering is active but the set is empty, return nothing
    if let Some(ids) = visible_ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
    }

    let visible_vec: Option<Vec<Uuid>> = visible_ids.map(|ids| ids.iter().copied().collect());

    let rows = match (status_filter, &visible_vec) {
        (Some(status), Some(ids)) => {
            client
                .query(
                    "SELECT job_id, definition_id, agent_id, status, config, created_by,
                            created_at, started_at, finished_at, run_id, error_message
                     FROM job WHERE project_id = $1 AND status = $2 AND job_id = ANY($3)
                     ORDER BY created_at DESC LIMIT $4 OFFSET $5",
                    &[project_id, &status, ids, &limit, &offset],
                )
                .await?
        }
        (Some(status), None) => {
            client
                .query(
                    "SELECT job_id, definition_id, agent_id, status, config, created_by,
                            created_at, started_at, finished_at, run_id, error_message
                     FROM job WHERE project_id = $1 AND status = $2
                     ORDER BY created_at DESC LIMIT $3 OFFSET $4",
                    &[project_id, &status, &limit, &offset],
                )
                .await?
        }
        (None, Some(ids)) => {
            client
                .query(
                    "SELECT job_id, definition_id, agent_id, status, config, created_by,
                            created_at, started_at, finished_at, run_id, error_message
                     FROM job WHERE project_id = $1 AND job_id = ANY($2)
                     ORDER BY created_at DESC LIMIT $3 OFFSET $4",
                    &[project_id, ids, &limit, &offset],
                )
                .await?
        }
        (None, None) => {
            client
                .query(
                    "SELECT job_id, definition_id, agent_id, status, config, created_by,
                            created_at, started_at, finished_at, run_id, error_message
                     FROM job WHERE project_id = $1
                     ORDER BY created_at DESC LIMIT $2 OFFSET $3",
                    &[project_id, &limit, &offset],
                )
                .await?
        }
    };

    Ok(rows
        .iter()
        .map(|r| JobRow {
            job_id: r.get("job_id"),
            definition_id: r.get("definition_id"),
            agent_id: r.get("agent_id"),
            status: r.get("status"),
            config: r.get("config"),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
            started_at: r.get("started_at"),
            finished_at: r.get("finished_at"),
            run_id: r.get("run_id"),
            error_message: r.get("error_message"),
        })
        .collect())
}

pub async fn update_status(client: &Client, job_id: &Uuid, status: &str) -> anyhow::Result<()> {
    let now: Option<DateTime<Utc>> = match status {
        "running" => Some(Utc::now()),
        _ => None,
    };
    let finished: Option<DateTime<Utc>> = match status {
        "completed" | "failed" | "cancelled" => Some(Utc::now()),
        _ => None,
    };
    client
        .execute(
            "UPDATE job SET status = $1,
                started_at = COALESCE($2, started_at),
                finished_at = COALESCE($3, finished_at)
             WHERE job_id = $4",
            &[&status, &now, &finished, job_id],
        )
        .await?;
    Ok(())
}

pub async fn set_run_id(client: &Client, job_id: &Uuid, run_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE job SET run_id = $1 WHERE job_id = $2",
            &[run_id, job_id],
        )
        .await?;
    Ok(())
}

pub async fn set_error(client: &Client, job_id: &Uuid, message: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE job SET status = 'failed', error_message = $1, finished_at = now() WHERE job_id = $2",
            &[&message, job_id],
        )
        .await?;
    Ok(())
}
