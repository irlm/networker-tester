use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;

#[derive(Debug, Serialize)]
pub struct HealthCheck {
    pub check_name: String,
    pub status: String,
    pub value: Option<String>,
    pub message: Option<String>,
    pub details: Option<serde_json::Value>,
    pub checked_at: DateTime<Utc>,
}

/// Insert a health check result.
pub async fn insert(
    client: &Client,
    check_name: &str,
    status: &str,
    value: Option<&str>,
    message: Option<&str>,
    details: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    client
        .execute(
            "INSERT INTO system_health (check_name, status, value, message, details) \
             VALUES ($1, $2, $3, $4, $5)",
            &[&check_name, &status, &value, &message, &details],
        )
        .await?;
    Ok(())
}

/// Get the latest health check for each check_name.
pub async fn latest_all(client: &Client) -> anyhow::Result<Vec<HealthCheck>> {
    let rows = client
        .query(
            "SELECT DISTINCT ON (check_name) \
                check_name, status, value, message, details, checked_at \
             FROM system_health \
             ORDER BY check_name, checked_at DESC",
            &[],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| HealthCheck {
            check_name: r.get("check_name"),
            status: r.get("status"),
            value: r.get("value"),
            message: r.get("message"),
            details: r.get("details"),
            checked_at: r.get("checked_at"),
        })
        .collect())
}

/// Delete health records older than 7 days.
pub async fn cleanup(client: &Client) -> anyhow::Result<u64> {
    let result = client
        .execute(
            "DELETE FROM system_health WHERE checked_at < now() - interval '7 days'",
            &[],
        )
        .await?;
    Ok(result)
}
