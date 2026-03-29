use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

// ── Row types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct BenchmarkRunRow {
    pub run_id: Uuid,
    pub name: String,
    pub config: serde_json::Value,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub tier: Option<String>,
    pub created_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<BenchmarkResultRow>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkResultRow {
    pub result_id: Uuid,
    pub run_id: Uuid,
    pub language: String,
    pub runtime: String,
    pub server_os: Option<String>,
    pub client_os: Option<String>,
    pub cloud: Option<String>,
    pub phase: Option<String>,
    pub concurrency: Option<i32>,
    pub metrics: serde_json::Value,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct LeaderboardEntry {
    pub language: String,
    pub runtime: String,
    pub metrics: serde_json::Value,
    pub server_os: Option<String>,
    pub client_os: Option<String>,
    pub cloud: Option<String>,
    pub phase: Option<String>,
    pub concurrency: Option<i32>,
}

// ── Input types ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NewResult {
    pub language: String,
    pub runtime: String,
    #[serde(default)]
    pub server_os: Option<String>,
    #[serde(default)]
    pub client_os: Option<String>,
    #[serde(default)]
    pub cloud: Option<String>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub concurrency: Option<i32>,
    #[serde(default = "default_empty_object")]
    pub metrics: serde_json::Value,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
}

fn default_empty_object() -> serde_json::Value {
    serde_json::json!({})
}

// ── Queries ──────────────────────────────────────────────────────────────

pub async fn list_runs(client: &Client) -> anyhow::Result<Vec<BenchmarkRunRow>> {
    let rows = client
        .query(
            "SELECT run_id, name, config, status, started_at, finished_at, tier, created_by
             FROM benchmark_run
             ORDER BY started_at DESC
             LIMIT 50",
            &[],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| BenchmarkRunRow {
            run_id: r.get("run_id"),
            name: r.get("name"),
            config: r.get("config"),
            status: r.get("status"),
            started_at: r.get("started_at"),
            finished_at: r.get("finished_at"),
            tier: r.get("tier"),
            created_by: r.get("created_by"),
            results: None,
        })
        .collect())
}

pub async fn get_run(client: &Client, run_id: &Uuid) -> anyhow::Result<Option<BenchmarkRunRow>> {
    let row = client
        .query_opt(
            "SELECT run_id, name, config, status, started_at, finished_at, tier, created_by
             FROM benchmark_run WHERE run_id = $1",
            &[run_id],
        )
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let result_rows = client
        .query(
            "SELECT result_id, run_id, language, runtime, server_os, client_os,
                    cloud, phase, concurrency, metrics, started_at, finished_at
             FROM benchmark_result WHERE run_id = $1
             ORDER BY started_at",
            &[run_id],
        )
        .await?;

    let results: Vec<BenchmarkResultRow> = result_rows
        .iter()
        .map(|r| BenchmarkResultRow {
            result_id: r.get("result_id"),
            run_id: r.get("run_id"),
            language: r.get("language"),
            runtime: r.get("runtime"),
            server_os: r.get("server_os"),
            client_os: r.get("client_os"),
            cloud: r.get("cloud"),
            phase: r.get("phase"),
            concurrency: r.get("concurrency"),
            metrics: r.get("metrics"),
            started_at: r.get("started_at"),
            finished_at: r.get("finished_at"),
        })
        .collect();

    Ok(Some(BenchmarkRunRow {
        run_id: row.get("run_id"),
        name: row.get("name"),
        config: row.get("config"),
        status: row.get("status"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        tier: row.get("tier"),
        created_by: row.get("created_by"),
        results: Some(results),
    }))
}

pub async fn create_run(
    client: &Client,
    name: &str,
    config: &serde_json::Value,
) -> anyhow::Result<Uuid> {
    let row = client
        .query_one(
            "INSERT INTO benchmark_run (run_id, name, config)
             VALUES (gen_random_uuid(), $1, $2)
             RETURNING run_id",
            &[&name, config],
        )
        .await?;
    Ok(row.get("run_id"))
}

pub async fn finish_run(client: &Client, run_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_run SET status = 'completed', finished_at = now() WHERE run_id = $1",
            &[run_id],
        )
        .await?;
    Ok(())
}

pub async fn add_result(
    client: &Client,
    run_id: &Uuid,
    result: &NewResult,
) -> anyhow::Result<Uuid> {
    let row = client
        .query_one(
            "INSERT INTO benchmark_result
                (result_id, run_id, language, runtime, server_os, client_os,
                 cloud, phase, concurrency, metrics, started_at, finished_at)
             VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING result_id",
            &[
                run_id,
                &result.language,
                &result.runtime,
                &result.server_os.as_deref().unwrap_or("ubuntu-24.04"),
                &result.client_os.as_deref().unwrap_or("ubuntu-24.04"),
                &result.cloud.as_deref().unwrap_or("azure"),
                &result.phase.as_deref().unwrap_or("warm"),
                &result.concurrency.unwrap_or(1),
                &result.metrics,
                &result.started_at,
                &result.finished_at,
            ],
        )
        .await?;
    Ok(row.get("result_id"))
}

pub async fn get_latest_leaderboard(client: &Client) -> anyhow::Result<Vec<LeaderboardEntry>> {
    // Get results from the most recent completed run
    let rows = client
        .query(
            "SELECT br.language, br.runtime, br.metrics, br.server_os, br.client_os,
                    br.cloud, br.phase, br.concurrency
             FROM benchmark_result br
             JOIN benchmark_run brun ON brun.run_id = br.run_id
             WHERE brun.run_id = (
                 SELECT run_id FROM benchmark_run
                 WHERE status = 'completed'
                 ORDER BY started_at DESC LIMIT 1
             )
             ORDER BY (br.metrics->>'mean')::float ASC NULLS LAST",
            &[],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| LeaderboardEntry {
            language: r.get("language"),
            runtime: r.get("runtime"),
            metrics: r.get("metrics"),
            server_os: r.get("server_os"),
            client_os: r.get("client_os"),
            cloud: r.get("cloud"),
            phase: r.get("phase"),
            concurrency: r.get("concurrency"),
        })
        .collect())
}
