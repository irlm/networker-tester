use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct ModeProgress {
    pub mode: String,
    pub completed: i64,
    pub total: i32,
    pub p50_ms: Option<f64>,
    pub mean_ms: Option<f64>,
    pub success_count: i64,
    pub fail_count: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct LanguageProgress {
    pub language: String,
    pub testbed_id: Option<Uuid>,
    pub modes: Vec<ModeProgress>,
}

/// Insert a single request-progress row.
pub async fn insert_single(
    client: &Client,
    config_id: &Uuid,
    testbed_id: Option<&Uuid>,
    language: &str,
    mode: &str,
    request_index: i32,
    total_requests: i32,
    latency_ms: f64,
    success: bool,
) -> anyhow::Result<()> {
    client
        .execute(
            "INSERT INTO benchmark_request_progress \
             (config_id, testbed_id, language, mode, request_index, total_requests, latency_ms, success) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                config_id,
                &testbed_id,
                &language,
                &mode,
                &request_index,
                &total_requests,
                &latency_ms,
                &success,
            ],
        )
        .await?;
    Ok(())
}

/// Return per-language, per-mode progress with running p50/mean.
pub async fn get_progress(
    client: &Client,
    config_id: &Uuid,
) -> anyhow::Result<Vec<LanguageProgress>> {
    let rows = client
        .query(
            "SELECT language, testbed_id, mode, \
                 COUNT(*) as completed, \
                 MAX(total_requests) as total, \
                 PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY latency_ms) as p50, \
                 AVG(latency_ms) as mean, \
                 COUNT(*) FILTER (WHERE success) as success_count, \
                 COUNT(*) FILTER (WHERE NOT success) as fail_count \
             FROM benchmark_request_progress \
             WHERE config_id = $1 \
             GROUP BY language, testbed_id, mode \
             ORDER BY language, mode",
            &[config_id],
        )
        .await?;

    // Group rows by (language, testbed_id) into LanguageProgress structs.
    let mut result: Vec<LanguageProgress> = Vec::new();

    for row in &rows {
        let language: String = row.get("language");
        let testbed_id: Option<Uuid> = row.get("testbed_id");
        let mode_progress = ModeProgress {
            mode: row.get("mode"),
            completed: row.get("completed"),
            total: row.get::<_, i32>("total"),
            p50_ms: row.get("p50"),
            mean_ms: row.get("mean"),
            success_count: row.get("success_count"),
            fail_count: row.get("fail_count"),
        };

        // Find existing LanguageProgress or create a new one
        if let Some(lp) = result
            .iter_mut()
            .find(|lp| lp.language == language && lp.testbed_id == testbed_id)
        {
            lp.modes.push(mode_progress);
        } else {
            result.push(LanguageProgress {
                language,
                testbed_id,
                modes: vec![mode_progress],
            });
        }
    }

    Ok(result)
}

/// Delete all progress rows for a given config (cleanup).
#[allow(dead_code)]
pub async fn delete_for_config(client: &Client, config_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "DELETE FROM benchmark_request_progress WHERE config_id = $1",
            &[config_id],
        )
        .await?;
    Ok(())
}
