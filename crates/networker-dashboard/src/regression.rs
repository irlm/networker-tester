//! Regression detection for benchmark configs.
//!
//! After a benchmark completes, compare each language's p50 latency and success
//! rate against the baseline config. Flag regressions exceeding the configured
//! threshold and persist them to `benchmark_regression`.

use anyhow::{Context, Result};
use tokio_postgres::Client;
use uuid::Uuid;

/// Default regression threshold: 10% worse is flagged.
const DEFAULT_LATENCY_THRESHOLD_PERCENT: f64 = 10.0;

/// Default minimum success rate (below this is flagged).
const DEFAULT_MIN_SUCCESS_RATE: f64 = 99.0;

/// A single detected regression.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Regression {
    pub language: String,
    pub metric: String,
    pub baseline_value: f64,
    pub current_value: f64,
    pub delta_percent: f64,
    pub severity: String,
}

/// Run regression detection for a completed benchmark config.
///
/// Returns the list of regressions found (empty if no baseline or no regressions).
pub async fn detect(
    client: &Client,
    config_id: &Uuid,
    latency_threshold: Option<f64>,
    min_success_rate: Option<f64>,
) -> Result<Vec<Regression>> {
    let threshold = latency_threshold.unwrap_or(DEFAULT_LATENCY_THRESHOLD_PERCENT);
    let min_sr = min_success_rate.unwrap_or(DEFAULT_MIN_SUCCESS_RATE);

    // 1. Load the current config to find baseline_run_id
    let config = crate::db::benchmark_configs::get(client, config_id)
        .await
        .context("Failed to load benchmark config for regression detection")?
        .ok_or_else(|| anyhow::anyhow!("Benchmark config not found: {config_id}"))?;

    let baseline_config_id = match config.baseline_run_id {
        Some(id) => id,
        None => {
            tracing::debug!(
                config_id = %config_id,
                "No baseline_run_id set — skipping regression detection"
            );
            return Ok(Vec::new());
        }
    };

    // 2. Load current results (pipeline summaries per language)
    let current_results = load_language_metrics(client, config_id).await?;
    if current_results.is_empty() {
        tracing::debug!(config_id = %config_id, "No results found for current config");
        return Ok(Vec::new());
    }

    // 3. Load baseline results
    let baseline_results = load_language_metrics(client, &baseline_config_id).await?;
    if baseline_results.is_empty() {
        tracing::debug!(
            config_id = %config_id,
            baseline_config_id = %baseline_config_id,
            "No results found for baseline config"
        );
        return Ok(Vec::new());
    }

    // 4. Compare each language
    let mut regressions = Vec::new();

    for (language, current) in &current_results {
        if let Some(baseline) = baseline_results.get(language) {
            // p50 latency comparison (higher is worse for latency)
            if baseline.p50_ms > 0.0 && current.p50_ms > 0.0 {
                let delta = ((current.p50_ms - baseline.p50_ms) / baseline.p50_ms) * 100.0;
                if delta > threshold {
                    let severity = if delta > threshold * 2.0 {
                        "critical"
                    } else {
                        "warning"
                    };
                    regressions.push(Regression {
                        language: language.clone(),
                        metric: "p50_latency_ms".to_string(),
                        baseline_value: baseline.p50_ms,
                        current_value: current.p50_ms,
                        delta_percent: delta,
                        severity: severity.to_string(),
                    });
                }
            }

            // Success rate comparison
            if current.success_rate < min_sr && baseline.success_rate >= min_sr {
                let delta = baseline.success_rate - current.success_rate;
                let severity = if delta > 5.0 { "critical" } else { "warning" };
                regressions.push(Regression {
                    language: language.clone(),
                    metric: "success_rate".to_string(),
                    baseline_value: baseline.success_rate,
                    current_value: current.success_rate,
                    delta_percent: -delta,
                    severity: severity.to_string(),
                });
            }
        }
    }

    // 5. Persist regressions
    for reg in &regressions {
        save_regression(client, config_id, &baseline_config_id, reg).await?;
    }

    if !regressions.is_empty() {
        tracing::warn!(
            config_id = %config_id,
            baseline_config_id = %baseline_config_id,
            count = regressions.len(),
            "Benchmark regressions detected"
        );
    }

    Ok(regressions)
}

/// Per-language aggregated metrics for comparison.
#[derive(Debug, Default)]
struct LanguageMetrics {
    p50_ms: f64,
    success_rate: f64,
}

/// Load per-language p50 and success rate from pipeline tables for a config.
async fn load_language_metrics(
    client: &Client,
    config_id: &Uuid,
) -> Result<std::collections::HashMap<String, LanguageMetrics>> {
    // Join benchmark_run (pipeline) with BenchmarkSummary to get per-language metrics.
    // The benchmark_run.config_id links to the config, and the run stores per-language data.
    let rows = client
        .query(
            "SELECT
                br.name AS run_name,
                bs.p50,
                bs.sample_count,
                bs.included_sample_count
             FROM benchmark_run br
             JOIN BenchmarkSummary bs ON bs.BenchmarkRunId = br.run_id::text::uuid
             WHERE br.config_id = $1
               AND br.status = 'completed'
               AND bs.MetricName = 'latency'",
            &[config_id],
        )
        .await
        .context("Failed to load pipeline summaries for regression detection")?;

    let mut results = std::collections::HashMap::new();

    for row in &rows {
        let run_name: String = row.get("run_name");
        // Extract language from run name (format: "Config Name - language")
        let language = run_name
            .rsplit(" - ")
            .next()
            .unwrap_or(&run_name)
            .to_string();

        let p50: f64 = row.get("p50");
        let sample_count: i32 = row.get("sample_count");
        let included_count: i32 = row.get("included_sample_count");

        let success_rate = if sample_count > 0 {
            (included_count as f64 / sample_count as f64) * 100.0
        } else {
            100.0
        };

        results.insert(
            language,
            LanguageMetrics {
                p50_ms: p50,
                success_rate,
            },
        );
    }

    Ok(results)
}

/// Persist a single regression to the database.
async fn save_regression(
    client: &Client,
    config_id: &Uuid,
    baseline_config_id: &Uuid,
    reg: &Regression,
) -> Result<()> {
    client
        .execute(
            "INSERT INTO benchmark_regression
                (config_id, baseline_config_id, language, metric,
                 baseline_value, current_value, delta_percent, severity)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                config_id,
                baseline_config_id,
                &reg.language,
                &reg.metric,
                &reg.baseline_value,
                &reg.current_value,
                &reg.delta_percent,
                &reg.severity,
            ],
        )
        .await
        .context("Failed to insert benchmark regression")?;
    Ok(())
}

/// List regressions for a specific config.
pub async fn list_for_config(client: &Client, config_id: &Uuid) -> Result<Vec<RegressionRow>> {
    let rows = client
        .query(
            "SELECT regression_id, config_id, baseline_config_id, language, metric,
                    baseline_value, current_value, delta_percent, severity, detected_at
             FROM benchmark_regression
             WHERE config_id = $1
             ORDER BY detected_at DESC",
            &[config_id],
        )
        .await
        .context("Failed to list regressions for config")?;

    Ok(rows.iter().map(row_to_regression).collect())
}

/// List all regressions for a project (across all configs).
pub async fn list_for_project(
    client: &Client,
    project_id: &Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<RegressionWithConfig>> {
    let rows = client
        .query(
            "SELECT r.regression_id, r.config_id, r.baseline_config_id, r.language, r.metric,
                    r.baseline_value, r.current_value, r.delta_percent, r.severity, r.detected_at,
                    c.name AS config_name
             FROM benchmark_regression r
             JOIN benchmark_config c ON c.config_id = r.config_id
             WHERE c.project_id = $1
             ORDER BY r.detected_at DESC
             LIMIT $2 OFFSET $3",
            &[project_id, &limit, &offset],
        )
        .await
        .context("Failed to list regressions for project")?;

    Ok(rows
        .iter()
        .map(|row| RegressionWithConfig {
            regression: row_to_regression(row),
            config_name: row.get("config_name"),
        })
        .collect())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegressionRow {
    pub regression_id: Uuid,
    pub config_id: Uuid,
    pub baseline_config_id: Option<Uuid>,
    pub language: String,
    pub metric: String,
    pub baseline_value: f64,
    pub current_value: f64,
    pub delta_percent: f64,
    pub severity: String,
    pub detected_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegressionWithConfig {
    #[serde(flatten)]
    pub regression: RegressionRow,
    pub config_name: String,
}

fn row_to_regression(row: &tokio_postgres::Row) -> RegressionRow {
    RegressionRow {
        regression_id: row.get("regression_id"),
        config_id: row.get("config_id"),
        baseline_config_id: row.get("baseline_config_id"),
        language: row.get("language"),
        metric: row.get("metric"),
        baseline_value: row.get("baseline_value"),
        current_value: row.get("current_value"),
        delta_percent: row.get("delta_percent"),
        severity: row.get("severity"),
        detected_at: row.get("detected_at"),
    }
}

/// Send notification emails about detected regressions.
pub async fn notify_regressions(
    config_id: &Uuid,
    config_name: &str,
    regressions: &[Regression],
    project_members_emails: &[String],
) {
    if regressions.is_empty() || project_members_emails.is_empty() {
        return;
    }

    let mut body = format!(
        "Benchmark regression detected in \"{}\".\n\n\
         Config ID: {}\n\
         Regressions found: {}\n\n",
        config_name,
        config_id,
        regressions.len()
    );

    for reg in regressions {
        body.push_str(&format!(
            "  - {} / {}: {:.2} -> {:.2} ({:+.1}%) [{}]\n",
            reg.language,
            reg.metric,
            reg.baseline_value,
            reg.current_value,
            reg.delta_percent,
            reg.severity
        ));
    }

    body.push_str(
        "\nReview regressions in the AletheDash benchmark results page.\n\n-- AletheDash",
    );

    let subject = format!(
        "AletheDash -- Benchmark regression in \"{}\" ({} issue{})",
        config_name,
        regressions.len(),
        if regressions.len() == 1 { "" } else { "s" }
    );

    for email in project_members_emails {
        if let Err(e) = crate::email::send_email(email, &subject, &body).await {
            tracing::warn!(
                error = %e,
                email = %email,
                "Failed to send regression notification email"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_fields_populated() {
        let reg = Regression {
            language: "rust".to_string(),
            metric: "p50_latency_ms".to_string(),
            baseline_value: 1.5,
            current_value: 2.0,
            delta_percent: 33.3,
            severity: "warning".to_string(),
        };
        assert_eq!(reg.language, "rust");
        assert_eq!(reg.metric, "p50_latency_ms");
        assert!(reg.delta_percent > 0.0);
    }

    #[test]
    fn regression_serializes_to_json() {
        let reg = Regression {
            language: "go".to_string(),
            metric: "success_rate".to_string(),
            baseline_value: 99.5,
            current_value: 97.0,
            delta_percent: -2.5,
            severity: "critical".to_string(),
        };
        let json = serde_json::to_value(&reg).expect("must serialize");
        assert_eq!(json["language"], "go");
        assert_eq!(json["severity"], "critical");
    }

    #[test]
    fn default_thresholds() {
        assert!((DEFAULT_LATENCY_THRESHOLD_PERCENT - 10.0).abs() < f64::EPSILON);
        assert!((DEFAULT_MIN_SUCCESS_RATE - 99.0).abs() < f64::EPSILON);
    }

    // ─── language extraction from run name ───────────────────────────────

    /// Mirrors the inline parsing in `load_language_metrics()`.
    fn extract_language(run_name: &str) -> String {
        run_name
            .rsplit(" - ")
            .next()
            .unwrap_or(run_name)
            .to_string()
    }

    #[test]
    fn extract_language_standard_format() {
        assert_eq!(extract_language("My Config - rust"), "rust");
        assert_eq!(extract_language("Benchmark - go"), "go");
    }

    #[test]
    fn extract_language_no_separator() {
        assert_eq!(extract_language("rust"), "rust");
    }

    #[test]
    fn extract_language_multiple_separators() {
        assert_eq!(extract_language("Multi - Part - Config - python"), "python");
    }

    #[test]
    fn extract_language_empty() {
        assert_eq!(extract_language(""), "");
    }

    // ─── severity classification ─────────────────────────────────────────

    #[test]
    fn latency_severity_warning_at_threshold() {
        let threshold = DEFAULT_LATENCY_THRESHOLD_PERCENT;
        let delta = threshold + 1.0; // just above threshold
        let severity = if delta > threshold * 2.0 {
            "critical"
        } else {
            "warning"
        };
        assert_eq!(severity, "warning");
    }

    #[test]
    fn latency_severity_critical_at_double_threshold() {
        let threshold = DEFAULT_LATENCY_THRESHOLD_PERCENT;
        let delta = threshold * 2.0 + 1.0; // above 2x threshold
        let severity = if delta > threshold * 2.0 {
            "critical"
        } else {
            "warning"
        };
        assert_eq!(severity, "critical");
    }

    #[test]
    fn success_rate_severity_warning_for_small_drop() {
        let delta = 3.0; // 3% drop
        let severity = if delta > 5.0 { "critical" } else { "warning" };
        assert_eq!(severity, "warning");
    }

    #[test]
    fn success_rate_severity_critical_for_large_drop() {
        let delta = 6.0; // 6% drop
        let severity = if delta > 5.0 { "critical" } else { "warning" };
        assert_eq!(severity, "critical");
    }

    // ─── delta calculation ───────────────────────────────────────────────

    #[test]
    fn latency_delta_percent_positive_regression() {
        let baseline: f64 = 100.0;
        let current: f64 = 115.0;
        let delta = ((current - baseline) / baseline) * 100.0;
        assert!((delta - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn latency_delta_percent_improvement() {
        let baseline: f64 = 100.0;
        let current: f64 = 90.0;
        let delta = ((current - baseline) / baseline) * 100.0;
        assert!((delta - (-10.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn success_rate_delta_is_drop() {
        let baseline_sr: f64 = 99.5;
        let current_sr: f64 = 97.0;
        let delta = baseline_sr - current_sr;
        assert!((delta - 2.5).abs() < f64::EPSILON);
    }
}
