//! CRUD for `benchmark_artifact` — the methodology-mode rich result row.
//!
//! One artifact is created per `test_run` whose `test_config.methodology`
//! was non-null. The heavy fields are kept as JSONB so the statistical
//! shape can evolve without a schema migration on every change.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

/// A benchmark artifact row. The JSONB fields are kept as
/// `serde_json::Value` at this layer because their shapes are internal to
/// the reporting / regression pipeline and not part of the cross-crate
/// `networker-common` surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkArtifact {
    pub id: Uuid,
    pub test_run_id: Uuid,
    pub environment: serde_json::Value,
    pub methodology: serde_json::Value,
    pub launches: serde_json::Value,
    pub cases: serde_json::Value,
    #[serde(default)]
    pub samples: Option<serde_json::Value>,
    pub summaries: serde_json::Value,
    pub data_quality: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewBenchmarkArtifact<'a> {
    pub test_run_id: &'a Uuid,
    pub environment: &'a serde_json::Value,
    pub methodology: &'a serde_json::Value,
    pub launches: &'a serde_json::Value,
    pub cases: &'a serde_json::Value,
    pub samples: Option<&'a serde_json::Value>,
    pub summaries: &'a serde_json::Value,
    pub data_quality: &'a serde_json::Value,
}

/// Insert a new artifact. Also sets `test_run.artifact_id` inside the same
/// transaction so the run and its artifact can never drift apart.
pub async fn create(
    client: &Client,
    new: &NewBenchmarkArtifact<'_>,
) -> anyhow::Result<BenchmarkArtifact> {
    let row = client
        .query_one(
            "INSERT INTO benchmark_artifact
                (test_run_id, environment, methodology, launches, cases,
                 samples, summaries, data_quality)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             RETURNING id, test_run_id, environment, methodology, launches,
                       cases, samples, summaries, data_quality, created_at",
            &[
                &new.test_run_id,
                new.environment,
                new.methodology,
                new.launches,
                new.cases,
                &new.samples,
                new.summaries,
                new.data_quality,
            ],
        )
        .await?;

    let artifact = row_to_artifact(&row);

    // Stitch the artifact id back onto test_run — callers can also do this
    // themselves via `test_runs::attach_artifact`, but doing it here
    // guarantees the invariant.
    client
        .execute(
            "UPDATE test_run SET artifact_id = $2 WHERE id = $1",
            &[&artifact.test_run_id, &artifact.id],
        )
        .await?;

    Ok(artifact)
}

pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<BenchmarkArtifact>> {
    let row = client
        .query_opt(
            "SELECT id, test_run_id, environment, methodology, launches,
                    cases, samples, summaries, data_quality, created_at
             FROM benchmark_artifact WHERE id = $1",
            &[id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_artifact))
}

/// Fetch the artifact attached to a specific run, if any.
pub async fn get_for_run(
    client: &Client,
    test_run_id: &Uuid,
) -> anyhow::Result<Option<BenchmarkArtifact>> {
    let row = client
        .query_opt(
            "SELECT id, test_run_id, environment, methodology, launches,
                    cases, samples, summaries, data_quality, created_at
             FROM benchmark_artifact WHERE test_run_id = $1
             ORDER BY created_at DESC
             LIMIT 1",
            &[test_run_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_artifact))
}

pub async fn list(
    client: &Client,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<BenchmarkArtifact>> {
    let rows = client
        .query(
            "SELECT id, test_run_id, environment, methodology, launches,
                    cases, samples, summaries, data_quality, created_at
             FROM benchmark_artifact
             ORDER BY created_at DESC
             LIMIT $1 OFFSET $2",
            &[&limit, &offset],
        )
        .await?;
    Ok(rows.iter().map(row_to_artifact).collect())
}

pub async fn delete(client: &Client, id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute("DELETE FROM benchmark_artifact WHERE id = $1", &[id])
        .await?;
    Ok(n > 0)
}

// ── helpers ─────────────────────────────────────────────────────────────
fn row_to_artifact(r: &tokio_postgres::Row) -> BenchmarkArtifact {
    BenchmarkArtifact {
        id: r.get("id"),
        test_run_id: r.get("test_run_id"),
        environment: r.get("environment"),
        methodology: r.get("methodology"),
        launches: r.get("launches"),
        cases: r.get("cases"),
        samples: r.get("samples"),
        summaries: r.get("summaries"),
        data_quality: r.get("data_quality"),
        created_at: r.get("created_at"),
    }
}
