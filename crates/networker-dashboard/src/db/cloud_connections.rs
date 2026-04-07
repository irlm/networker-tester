use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct CloudConnectionRow {
    pub connection_id: Uuid,
    pub name: String,
    pub provider: String,
    pub config: serde_json::Value,
    pub status: String,
    pub last_validated: Option<DateTime<Utc>>,
    pub validation_error: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn row_to_connection(r: &tokio_postgres::Row) -> CloudConnectionRow {
    CloudConnectionRow {
        connection_id: r.get("connection_id"),
        name: r.get("name"),
        provider: r.get("provider"),
        config: r.get("config"),
        status: r.get("status"),
        last_validated: r.get("last_validated"),
        validation_error: r.get("validation_error"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

pub async fn create(
    client: &Client,
    name: &str,
    provider: &str,
    config: &serde_json::Value,
    created_by: Option<&Uuid>,
    project_id: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO cloud_connection (connection_id, name, provider, config, created_by, project_id)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[&id, &name, &provider, config, &created_by, &project_id],
        )
        .await?;
    Ok(id)
}

pub async fn list(client: &Client, project_id: &str) -> anyhow::Result<Vec<CloudConnectionRow>> {
    let rows = client
        .query(
            "SELECT connection_id, name, provider, config, status, last_validated,
                    validation_error, created_by, created_at, updated_at
             FROM cloud_connection WHERE project_id = $1 ORDER BY created_at",
            &[&project_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_connection).collect())
}

pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<CloudConnectionRow>> {
    let row = client
        .query_opt(
            "SELECT connection_id, name, provider, config, status, last_validated,
                    validation_error, created_by, created_at, updated_at
             FROM cloud_connection WHERE connection_id = $1",
            &[id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_connection))
}

pub async fn update(
    client: &Client,
    id: &Uuid,
    name: &str,
    config: &serde_json::Value,
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE cloud_connection SET name = $1, config = $2, updated_at = now()
             WHERE connection_id = $3",
            &[&name, config, id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn delete(client: &Client, id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "DELETE FROM cloud_connection WHERE connection_id = $1",
            &[id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn set_status(
    client: &Client,
    id: &Uuid,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE cloud_connection SET status = $1, validation_error = $2,
             last_validated = now(), updated_at = now()
             WHERE connection_id = $3",
            &[&status, &error, id],
        )
        .await?;
    Ok(())
}
