use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct BenchmarkLogRow {
    pub id: i64,
    pub config_id: Uuid,
    pub testbed_id: Option<Uuid>,
    pub line: String,
    pub logged_at: DateTime<Utc>,
}

/// Insert a batch of log lines.
pub async fn insert_batch(
    client: &Client,
    config_id: &Uuid,
    testbed_id: Option<&Uuid>,
    lines: &[String],
) -> anyhow::Result<u64> {
    if lines.is_empty() {
        return Ok(0);
    }
    let stmt = client
        .prepare("INSERT INTO benchmark_log (config_id, testbed_id, line) VALUES ($1, $2, $3)")
        .await?;
    let mut count = 0u64;
    for line in lines {
        client
            .execute(&stmt, &[config_id, &testbed_id, &line])
            .await?;
        count += 1;
    }
    Ok(count)
}

/// Fetch all log lines for a config, ordered by time.
pub async fn get_for_config(
    client: &Client,
    config_id: &Uuid,
) -> anyhow::Result<Vec<BenchmarkLogRow>> {
    let rows = client
        .query(
            "SELECT id, config_id, testbed_id, line, logged_at \
             FROM benchmark_log WHERE config_id = $1 ORDER BY logged_at, id",
            &[config_id],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| BenchmarkLogRow {
            id: r.get("id"),
            config_id: r.get("config_id"),
            testbed_id: r.get("testbed_id"),
            line: r.get("line"),
            logged_at: r.get("logged_at"),
        })
        .collect())
}
