use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct AgentRow {
    pub agent_id: Uuid,
    pub name: String,
    pub region: Option<String>,
    pub provider: Option<String>,
    pub status: String,
    pub version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub registered_at: DateTime<Utc>,
    pub tags: Option<serde_json::Value>,
}

pub async fn list(client: &Client) -> anyhow::Result<Vec<AgentRow>> {
    let rows = client
        .query(
            "SELECT agent_id, name, region, provider, status, version, os, arch,
                    last_heartbeat, registered_at, tags
             FROM agent ORDER BY name",
            &[],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| AgentRow {
            agent_id: r.get("agent_id"),
            name: r.get("name"),
            region: r.get("region"),
            provider: r.get("provider"),
            status: r.get("status"),
            version: r.get("version"),
            os: r.get("os"),
            arch: r.get("arch"),
            last_heartbeat: r.get("last_heartbeat"),
            registered_at: r.get("registered_at"),
            tags: r.get("tags"),
        })
        .collect())
}

pub async fn get_by_api_key(client: &Client, api_key: &str) -> anyhow::Result<Option<AgentRow>> {
    let row = client
        .query_opt(
            "SELECT agent_id, name, region, provider, status, version, os, arch,
                    last_heartbeat, registered_at, tags
             FROM agent WHERE api_key = $1",
            &[&api_key],
        )
        .await?;

    Ok(row.map(|r| AgentRow {
        agent_id: r.get("agent_id"),
        name: r.get("name"),
        region: r.get("region"),
        provider: r.get("provider"),
        status: r.get("status"),
        version: r.get("version"),
        os: r.get("os"),
        arch: r.get("arch"),
        last_heartbeat: r.get("last_heartbeat"),
        registered_at: r.get("registered_at"),
        tags: r.get("tags"),
    }))
}

pub async fn get_by_id(client: &Client, agent_id: &Uuid) -> anyhow::Result<Option<AgentRow>> {
    let row = client
        .query_opt(
            "SELECT agent_id, name, region, provider, status, version, os, arch,
                    last_heartbeat, registered_at, tags
             FROM agent WHERE agent_id = $1",
            &[agent_id],
        )
        .await?;

    Ok(row.map(|r| AgentRow {
        agent_id: r.get("agent_id"),
        name: r.get("name"),
        region: r.get("region"),
        provider: r.get("provider"),
        status: r.get("status"),
        version: r.get("version"),
        os: r.get("os"),
        arch: r.get("arch"),
        last_heartbeat: r.get("last_heartbeat"),
        registered_at: r.get("registered_at"),
        tags: r.get("tags"),
    }))
}

pub async fn update_status(client: &Client, agent_id: &Uuid, status: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE agent SET status = $1, last_heartbeat = now() WHERE agent_id = $2",
            &[&status, agent_id],
        )
        .await?;
    Ok(())
}

pub async fn update_heartbeat(client: &Client, agent_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE agent SET last_heartbeat = now() WHERE agent_id = $1",
            &[agent_id],
        )
        .await?;
    Ok(())
}

#[allow(dead_code)] // Used by admin API in Phase 2
pub async fn create(
    client: &Client,
    name: &str,
    api_key: &str,
    region: Option<&str>,
    provider: Option<&str>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO agent (agent_id, name, api_key, region, provider)
             VALUES ($1, $2, $3, $4, $5)",
            &[&id, &name, &api_key, &region, &provider],
        )
        .await?;
    Ok(id)
}
