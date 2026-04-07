use tokio_postgres::Client;

pub async fn has_warning(
    client: &Client,
    project_id: &str,
    warning_type: &str,
) -> anyhow::Result<bool> {
    let row = client
        .query_opt(
            "SELECT 1 FROM workspace_warning WHERE project_id = $1 AND warning_type = $2",
            &[&project_id, &warning_type],
        )
        .await?;
    Ok(row.is_some())
}

pub async fn record_warning(
    client: &Client,
    project_id: &str,
    warning_type: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            "INSERT INTO workspace_warning (project_id, warning_type) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            &[&project_id, &warning_type],
        )
        .await?;
    Ok(())
}

#[allow(dead_code)]
pub async fn clear_warnings(client: &Client, project_id: &str) -> anyhow::Result<()> {
    client
        .execute(
            "DELETE FROM workspace_warning WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    Ok(())
}

/// Find warning records sent more than N days ago for a specific type.
pub async fn warnings_older_than(
    client: &Client,
    warning_type: &str,
    days: i64,
) -> anyhow::Result<Vec<String>> {
    let rows = client
        .query(
            "SELECT project_id FROM workspace_warning \
             WHERE warning_type = $1 AND sent_at < now() - ($2::text || ' days')::interval",
            &[&warning_type, &days.to_string()],
        )
        .await?;
    Ok(rows.iter().map(|r| r.get("project_id")).collect())
}
