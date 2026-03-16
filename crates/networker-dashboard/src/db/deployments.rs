use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct DeploymentRow {
    pub deployment_id: Uuid,
    pub name: String,
    pub status: String,
    pub config: serde_json::Value,
    pub provider_summary: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub endpoint_ips: Option<serde_json::Value>,
    pub agent_id: Option<Uuid>,
    pub error_message: Option<String>,
    pub log: Option<String>,
}

fn row_to_deployment(r: &tokio_postgres::Row) -> DeploymentRow {
    DeploymentRow {
        deployment_id: r.get("deployment_id"),
        name: r.get("name"),
        status: r.get("status"),
        config: r.get("config"),
        provider_summary: r.get("provider_summary"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        endpoint_ips: r.get("endpoint_ips"),
        agent_id: r.get("agent_id"),
        error_message: r.get("error_message"),
        log: r.get("log"),
    }
}

pub async fn create(
    client: &Client,
    name: &str,
    config: &serde_json::Value,
    provider_summary: Option<&str>,
    created_by: Option<&Uuid>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO deployment (deployment_id, name, status, config, provider_summary, created_by)
             VALUES ($1, $2, 'pending', $3, $4, $5)",
            &[&id, &name, config, &provider_summary, &created_by],
        )
        .await?;
    Ok(id)
}

pub async fn get(client: &Client, deployment_id: &Uuid) -> anyhow::Result<Option<DeploymentRow>> {
    let row = client
        .query_opt(
            "SELECT deployment_id, name, status, config, provider_summary, created_by,
                    created_at, started_at, finished_at, endpoint_ips, agent_id,
                    error_message, log
             FROM deployment WHERE deployment_id = $1",
            &[deployment_id],
        )
        .await?;

    Ok(row.as_ref().map(row_to_deployment))
}

pub async fn list(client: &Client, limit: i64, offset: i64) -> anyhow::Result<Vec<DeploymentRow>> {
    let rows = client
        .query(
            "SELECT deployment_id, name, status, config, provider_summary, created_by,
                    created_at, started_at, finished_at, endpoint_ips, agent_id,
                    error_message, log
             FROM deployment ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            &[&limit, &offset],
        )
        .await?;

    Ok(rows.iter().map(row_to_deployment).collect())
}

pub async fn update_status(
    client: &Client,
    deployment_id: &Uuid,
    status: &str,
) -> anyhow::Result<()> {
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
            "UPDATE deployment SET status = $1,
                started_at = COALESCE($2, started_at),
                finished_at = COALESCE($3, finished_at)
             WHERE deployment_id = $4",
            &[&status, &now, &finished, deployment_id],
        )
        .await?;
    Ok(())
}

pub async fn set_endpoint_ips(
    client: &Client,
    deployment_id: &Uuid,
    ips: &serde_json::Value,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE deployment SET endpoint_ips = $1 WHERE deployment_id = $2",
            &[ips, deployment_id],
        )
        .await?;
    Ok(())
}

pub async fn set_error(client: &Client, deployment_id: &Uuid, message: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE deployment SET status = 'failed', error_message = $1, finished_at = now()
             WHERE deployment_id = $2",
            &[&message, deployment_id],
        )
        .await?;
    Ok(())
}

pub async fn delete(client: &Client, deployment_id: &Uuid) -> anyhow::Result<bool> {
    let count = client
        .execute(
            "DELETE FROM deployment WHERE deployment_id = $1",
            &[deployment_id],
        )
        .await?;
    Ok(count > 0)
}
