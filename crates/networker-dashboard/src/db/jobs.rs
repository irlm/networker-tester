use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct JobRow {
    pub job_id: Uuid,
    pub project_id: Option<String>,
    pub tls_profile_run_id: Option<Uuid>,
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
    project_id: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO job (job_id, agent_id, status, config, created_by, project_id)
             VALUES ($1, $2, 'pending', $3, $4, $5)",
            &[&id, &agent_id, config, &created_by, &project_id],
        )
        .await?;
    Ok(id)
}

pub async fn get(client: &Client, job_id: &Uuid) -> anyhow::Result<Option<JobRow>> {
    let row = client
        .query_opt(
            "SELECT job_id, project_id, tls_profile_run_id, definition_id, agent_id, status, config, created_by,
                    created_at, started_at, finished_at, run_id, error_message
             FROM job WHERE job_id = $1",
            &[job_id],
        )
        .await?;

    Ok(row.map(|r| JobRow {
        job_id: r.get("job_id"),
        project_id: r.get("project_id"),
        tls_profile_run_id: r.get("tls_profile_run_id"),
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
    project_id: &str,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<JobRow>> {
    list_filtered(
        client,
        project_id,
        status_filter,
        None::<&Uuid>,
        None::<&Uuid>,
        limit,
        offset,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn list_filtered(
    client: &Client,
    project_id: &str,
    status_filter: Option<&str>,
    agent_id_filter: Option<&Uuid>,
    created_by_filter: Option<&Uuid>,
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

    // Build query dynamically to support flexible filter combinations
    let base = "SELECT job_id, project_id, tls_profile_run_id, definition_id, agent_id, status, config, created_by,
                       created_at, started_at, finished_at, run_id, error_message
                FROM job WHERE project_id = $1";

    let mut clauses = Vec::new();
    let mut param_idx: usize = 2;

    // We build the SQL string dynamically but use parameterised queries for values
    if status_filter.is_some() {
        clauses.push(format!("status = ${param_idx}"));
        param_idx += 1;
    }
    if agent_id_filter.is_some() {
        clauses.push(format!("agent_id = ${param_idx}"));
        param_idx += 1;
    }
    if created_by_filter.is_some() {
        clauses.push(format!("created_by = ${param_idx}"));
        param_idx += 1;
    }
    if visible_vec.is_some() {
        clauses.push(format!("job_id = ANY(${param_idx})"));
        param_idx += 1;
    }

    let limit_clause = format!(
        "ORDER BY created_at DESC LIMIT ${param_idx} OFFSET ${}",
        param_idx + 1
    );
    let sql = if clauses.is_empty() {
        format!("{base} {limit_clause}")
    } else {
        format!("{base} AND {} {limit_clause}", clauses.join(" AND "))
    };

    // Build params vector dynamically
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    params.push(&project_id);
    if let Some(status) = &status_filter {
        params.push(status);
    }
    if let Some(aid) = &agent_id_filter {
        params.push(aid);
    }
    if let Some(uid) = &created_by_filter {
        params.push(uid);
    }
    if let Some(ref ids) = visible_vec {
        params.push(ids);
    }
    params.push(&limit);
    params.push(&offset);

    let rows = client.query(&sql, &params).await?;

    Ok(rows
        .iter()
        .map(|r| JobRow {
            job_id: r.get("job_id"),
            project_id: r.get("project_id"),
            tls_profile_run_id: r.get("tls_profile_run_id"),
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

pub async fn assign_to_agent(
    client: &Client,
    job_id: &Uuid,
    agent_id: &Uuid,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE job SET status = 'assigned', agent_id = $1 WHERE job_id = $2",
            &[agent_id, job_id],
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

pub async fn set_tls_profile_run_id(
    client: &Client,
    job_id: &Uuid,
    tls_profile_run_id: &Uuid,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE job SET tls_profile_run_id = $1 WHERE job_id = $2",
            &[tls_profile_run_id, job_id],
        )
        .await?;
    Ok(())
}

pub async fn recent_tls_profile_job_count(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
    minutes: i32,
) -> anyhow::Result<i64> {
    let row = client
        .query_one(
            "SELECT COUNT(*)
             FROM job
             WHERE project_id = $1
               AND created_by = $2
               AND created_at >= now() - ($3::text || ' minutes')::interval
               AND config ? 'tls_profile_url'",
            &[&project_id, user_id, &minutes.to_string()],
        )
        .await?;
    Ok(row.get(0))
}

/// Find jobs stuck in "assigned" status for longer than `stale_secs` seconds.
/// Returns (job_id, agent_id) pairs.
pub async fn find_stale_assigned(
    client: &Client,
    stale_secs: i64,
) -> anyhow::Result<Vec<(Uuid, Option<Uuid>)>> {
    let rows = client
        .query(
            "SELECT job_id, agent_id FROM job
             WHERE status = 'assigned'
               AND created_at < now() - ($1::text || ' seconds')::interval",
            &[&stale_secs.to_string()],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get("job_id"), r.get("agent_id")))
        .collect())
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
