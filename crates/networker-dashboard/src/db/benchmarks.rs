use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use tokio_postgres::{error::SqlState, Client};
use uuid::Uuid;

use networker_tester::output::json::{
    BenchmarkArtifact, BenchmarkCase, BenchmarkDataQuality, BenchmarkDiagnostics,
    BenchmarkEnvironment, BenchmarkLaunch, BenchmarkMetadata, BenchmarkMethodology,
    BenchmarkSample, BenchmarkSummary,
};

const REPORT_CONFIDENCE_LEVEL: f64 = 0.95;
const BOOTSTRAP_RESAMPLES: usize = 2_048;
const MAX_COMPARABLE_RTT_RATIO: f64 = 1.5;
const DEFAULT_OUTLIER_POLICY: &str =
    "Tukey 1.5xIQR fences flag outliers for audit while raw measured samples remain preserved in the artifact.";
const DEFAULT_UNCERTAINTY_METHOD: &str =
    "Percentile bootstrap confidence intervals around the median with deterministic resampling.";

#[derive(Debug, Clone)]
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn from_values(values: &[f64]) -> Self {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ values.len() as u64;
        for value in values {
            state ^= value.to_bits().wrapping_mul(0xbf58_476d_1ce4_e5b9);
            state = state.rotate_left(13);
        }
        if state == 0 {
            state = 0x94d0_49bb_1331_11eb;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_index(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }
}

#[derive(Debug, Serialize)]
pub struct BenchmarkRunSummary {
    pub run_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub target_url: String,
    pub target_host: String,
    pub modes: Vec<String>,
    pub concurrency: u32,
    pub total_runs: u32,
    pub contract_version: String,
    pub scenario: String,
    pub primary_phase: String,
    pub phase_model: String,
    pub execution_plan_source: Option<String>,
    pub server_region: Option<String>,
    pub network_type: Option<String>,
    pub baseline_rtt_p50_ms: Option<f64>,
    pub total_cases: u64,
    pub total_samples: u64,
    pub publication_ready: bool,
    pub noise_level: String,
    pub sufficiency: String,
    pub publication_blocker_count: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkDistributionStats {
    pub sample_count: u64,
    pub min: f64,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub stddev: f64,
    pub cv: f64,
    pub standard_error: f64,
    pub ci95_lower: f64,
    pub ci95_upper: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkEnvironmentFingerprintView {
    pub client_os: Option<String>,
    pub client_arch: Option<String>,
    pub client_cpu_cores: Option<u32>,
    pub client_region: Option<String>,
    pub server_os: Option<String>,
    pub server_arch: Option<String>,
    pub server_cpu_cores: Option<u32>,
    pub server_region: Option<String>,
    pub network_type: Option<String>,
    pub baseline_rtt_p50_ms: Option<f64>,
    pub baseline_rtt_p95_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComparedBenchmarkRun {
    pub run_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub target_host: String,
    pub scenario: String,
    pub primary_phase: String,
    pub phase_model: String,
    pub publication_ready: bool,
    pub noise_level: String,
    pub sufficiency: String,
    pub warning_count: usize,
    pub environment: BenchmarkEnvironmentFingerprintView,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkCaseRunView {
    pub run_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub target_host: String,
    pub scenario: String,
    pub primary_phase: String,
    pub phase_model: String,
    pub publication_ready: bool,
    pub noise_level: String,
    pub sufficiency: String,
    pub warning_count: usize,
    pub included_sample_count: u64,
    pub failure_count: u64,
    pub error_count: u64,
    pub rps: f64,
    pub p95: f64,
    pub p99: f64,
    pub environment: BenchmarkEnvironmentFingerprintView,
    pub distribution: BenchmarkDistributionStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkCaseCandidateComparison {
    pub run: BenchmarkCaseRunView,
    pub comparable: bool,
    pub comparability_notes: Vec<String>,
    pub absolute_delta: Option<f64>,
    pub percent_delta: Option<f64>,
    pub ratio: Option<f64>,
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkCaseComparison {
    pub case_id: String,
    pub protocol: String,
    pub payload_bytes: Option<usize>,
    pub http_stack: Option<String>,
    pub metric_name: String,
    pub metric_unit: String,
    pub higher_is_better: bool,
    pub baseline: BenchmarkCaseRunView,
    pub candidates: Vec<BenchmarkCaseCandidateComparison>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkComparisonReport {
    pub baseline_run_id: Uuid,
    pub comparability_policy: String,
    pub gated_candidate_count: usize,
    pub runs: Vec<ComparedBenchmarkRun>,
    pub cases: Vec<BenchmarkCaseComparison>,
}

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

/// Persist a full BenchmarkArtifact into the pipeline tables (BenchmarkRun, BenchmarkLaunch,
/// BenchmarkEnvironment, BenchmarkDataQuality, BenchmarkCase, BenchmarkSample, BenchmarkSummary).
/// Also creates a job row to link the pipeline run to the project, and updates the
/// lightweight `benchmark_run` (lowercase) row with cell_id/config_id for grouping.
#[allow(clippy::too_many_arguments)]
pub async fn save_artifact(
    client: &Client,
    project_id: &Uuid,
    config_id: &Uuid,
    cell_id: Option<&Uuid>,
    language: &str,
    artifact: &BenchmarkArtifact,
) -> anyhow::Result<Uuid> {
    let run_id = artifact.metadata.run_id;

    // 1. Insert into pipeline BenchmarkRun table (Pascal-case)
    let modes = artifact.metadata.modes.join(",");
    let methodology_json = serde_json::to_value(&artifact.methodology)?;
    let diagnostics_json = serde_json::to_value(&artifact.diagnostics)?;
    let aggregate_summary_json = serde_json::to_value(&artifact.summary)?;

    client
        .execute(
            "INSERT INTO BenchmarkRun (
                BenchmarkRunId, ContractVersion, GeneratedAt, Source, TargetUrl, TargetHost,
                Modes, TotalRuns, Concurrency, TimeoutMs, ClientOs, ClientVersion,
                MethodologyJson, DiagnosticsJson, AggregateSummaryJson
             ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15
             )
             ON CONFLICT (BenchmarkRunId) DO NOTHING",
            &[
                &run_id,
                &artifact.metadata.contract_version,
                &artifact.metadata.generated_at,
                &artifact.metadata.source,
                &artifact.metadata.target_url,
                &artifact.metadata.target_host,
                &modes,
                &(artifact.metadata.total_runs as i32),
                &(artifact.metadata.concurrency as i32),
                &(artifact.metadata.timeout_ms as i64),
                &artifact.metadata.client_os,
                &artifact.metadata.client_version,
                &methodology_json,
                &diagnostics_json,
                &aggregate_summary_json,
            ],
        )
        .await
        .context("INSERT BenchmarkRun (pipeline)")?;

    // 2. BenchmarkEnvironment
    let environment_json = serde_json::to_value(&artifact.environment)?;
    let client_info_json = artifact
        .environment
        .client_info
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let server_info_json = artifact
        .environment
        .server_info
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let network_baseline_json = artifact
        .environment
        .network_baseline
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;

    client
        .execute(
            "INSERT INTO BenchmarkEnvironment (
                BenchmarkRunId, ClientInfoJson, ServerInfoJson, NetworkBaselineJson,
                PacketCaptureEnabled, EnvironmentJson
             ) VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT DO NOTHING",
            &[
                &run_id,
                &client_info_json,
                &server_info_json,
                &network_baseline_json,
                &artifact.environment.packet_capture_enabled,
                &environment_json,
            ],
        )
        .await
        .context("INSERT BenchmarkEnvironment")?;

    // 3. BenchmarkDataQuality
    let warnings_json = serde_json::to_value(&artifact.data_quality.warnings)?;
    let quality_json = serde_json::to_value(&artifact.data_quality)?;
    client
        .execute(
            "INSERT INTO BenchmarkDataQuality (
                BenchmarkRunId, NoiseLevel, SampleStabilityCv, Sufficiency,
                PublicationReady, WarningsJson, QualityJson
             ) VALUES ($1,$2,$3,$4,$5,$6,$7)
             ON CONFLICT DO NOTHING",
            &[
                &run_id,
                &artifact.data_quality.noise_level,
                &artifact.data_quality.sample_stability_cv,
                &artifact.data_quality.sufficiency,
                &artifact.data_quality.publication_ready,
                &warnings_json,
                &quality_json,
            ],
        )
        .await
        .context("INSERT BenchmarkDataQuality")?;

    // 4. BenchmarkLaunch rows
    for launch in &artifact.launches {
        let phases_json = serde_json::to_value(&launch.phases_present)?;
        client
            .execute(
                "INSERT INTO BenchmarkLaunch (
                    BenchmarkRunId, LaunchIndex, Scenario, PrimaryPhase, StartedAt, FinishedAt,
                    SampleCount, PrimarySampleCount, WarmupSampleCount, SuccessCount, FailureCount,
                    PhasesJson
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
                &[
                    &run_id,
                    &(launch.launch_index as i32),
                    &launch.scenario,
                    &launch.primary_phase,
                    &launch.started_at,
                    &launch.finished_at,
                    &(launch.sample_count as i64),
                    &(launch.primary_sample_count as i64),
                    &(launch.warmup_sample_count as i64),
                    &(launch.success_count as i64),
                    &(launch.failure_count as i64),
                    &phases_json,
                ],
            )
            .await
            .context("INSERT BenchmarkLaunch")?;
    }

    // 5. BenchmarkCase rows
    for case in &artifact.cases {
        let case_json = serde_json::to_value(case)?;
        client
            .execute(
                "INSERT INTO BenchmarkCase (
                    BenchmarkRunId, CaseId, Protocol, PayloadBytes, HttpStack,
                    MetricName, MetricUnit, HigherIsBetter, CaseJson
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
                &[
                    &run_id,
                    &case.id,
                    &case.protocol,
                    &case.payload_bytes.map(|v| v as i64),
                    &case.http_stack,
                    &case.metric_name,
                    &case.metric_unit,
                    &case.higher_is_better,
                    &case_json,
                ],
            )
            .await
            .context("INSERT BenchmarkCase")?;
    }

    // 6. BenchmarkSample rows
    for sample in &artifact.samples {
        let sample_json = serde_json::to_value(sample)?;
        client
            .execute(
                "INSERT INTO BenchmarkSample (
                    AttemptId, BenchmarkRunId, CaseId, LaunchIndex, Phase, IterationIndex,
                    Success, RetryCount, InclusionStatus, MetricValue, MetricUnit, StartedAt,
                    FinishedAt, TotalDurationMs, TtfbMs, SampleJson
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
                &[
                    &sample.attempt_id,
                    &run_id,
                    &sample.case_id,
                    &(sample.launch_index as i32),
                    &sample.phase,
                    &(sample.iteration_index as i32),
                    &sample.success,
                    &(sample.retry_count as i32),
                    &sample.inclusion_status,
                    &sample.metric_value,
                    &sample.metric_unit,
                    &sample.started_at,
                    &sample.finished_at,
                    &sample.total_duration_ms,
                    &sample.ttfb_ms,
                    &sample_json,
                ],
            )
            .await
            .context("INSERT BenchmarkSample")?;
    }

    // 7. BenchmarkSummary rows
    for summary in &artifact.summaries {
        let summary_json = serde_json::to_value(summary)?;
        client
            .execute(
                "INSERT INTO BenchmarkSummary (
                    BenchmarkRunId, CaseId, Protocol, PayloadBytes, HttpStack, MetricName,
                    MetricUnit, HigherIsBetter, SampleCount, IncludedSampleCount,
                    ExcludedSampleCount, SuccessCount, FailureCount, TotalRequests, ErrorCount,
                    BytesTransferred, WallTimeMs, Rps, Min, Mean, P5, P25, P50, P75, P95, P99,
                    P999, Max, Stddev, LatencyMeanMs, LatencyP50Ms, LatencyP99Ms,
                    LatencyP999Ms, LatencyMaxMs, SummaryJson
                 ) VALUES (
                    $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                    $21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35
                 )",
                &[
                    &run_id,
                    &summary.case_id,
                    &summary.protocol,
                    &summary.payload_bytes.map(|v| v as i64),
                    &summary.http_stack,
                    &summary.metric_name,
                    &summary.metric_unit,
                    &summary.higher_is_better,
                    &(summary.sample_count as i64),
                    &(summary.included_sample_count as i64),
                    &(summary.excluded_sample_count as i64),
                    &(summary.success_count as i64),
                    &(summary.failure_count as i64),
                    &(summary.total_requests as i64),
                    &(summary.error_count as i64),
                    &(summary.bytes_transferred as i64),
                    &summary.wall_time_ms,
                    &summary.rps,
                    &summary.min,
                    &summary.mean,
                    &summary.p5,
                    &summary.p25,
                    &summary.p50,
                    &summary.p75,
                    &summary.p95,
                    &summary.p99,
                    &summary.p999,
                    &summary.max,
                    &summary.stddev,
                    &summary.latency_mean_ms,
                    &summary.latency_p50_ms,
                    &summary.latency_p99_ms,
                    &summary.latency_p999_ms,
                    &summary.latency_max_ms,
                    &summary_json,
                ],
            )
            .await
            .context("INSERT BenchmarkSummary")?;
    }

    // 8. Create a job row to link the pipeline run to the project
    let job_config = serde_json::json!({
        "type": "benchmark_artifact",
        "config_id": config_id,
        "cell_id": cell_id,
        "language": language,
    });
    let job_id = crate::db::jobs::create(client, &job_config, None, None, project_id).await?;
    crate::db::jobs::set_run_id(client, &job_id, &run_id).await?;

    // 9. Update the lightweight benchmark_run row with cell_id/config_id
    client
        .execute(
            "UPDATE benchmark_run SET cell_id = $1, config_id = $2 WHERE run_id = $3",
            &[&cell_id, config_id, &run_id],
        )
        .await
        .context("UPDATE benchmark_run cell_id/config_id")?;

    Ok(run_id)
}

/// Retrieve benchmark results for a config, grouped by cell.
pub async fn get_config_results(
    client: &Client,
    config_id: &Uuid,
) -> anyhow::Result<Vec<ConfigCellResult>> {
    // Get all benchmark_run rows linked to this config
    let rows = client
        .query(
            "SELECT br.run_id, br.name, br.cell_id, br.config_id,
                    br.status, br.started_at, br.finished_at
             FROM benchmark_run br
             WHERE br.config_id = $1
             ORDER BY br.started_at",
            &[config_id],
        )
        .await?;

    let mut results = Vec::new();
    for row in &rows {
        let run_id: Uuid = row.get("run_id");
        let cell_id: Option<Uuid> = row.get("cell_id");
        let name: String = row.get("name");

        // Get summaries for this run from the pipeline tables
        let summary_rows = client
            .query(
                "SELECT SummaryJson FROM BenchmarkSummary WHERE BenchmarkRunId = $1
                 ORDER BY Protocol, PayloadBytes NULLS FIRST",
                &[&run_id],
            )
            .await
            .unwrap_or_default();

        let summaries: Vec<BenchmarkSummary> = summary_rows
            .iter()
            .filter_map(|r| {
                serde_json::from_value(r.get("summaryjson")).ok()
            })
            .collect();

        // Extract language from the run name (format: "Config Name - language")
        let language = name
            .rsplit_once(" - ")
            .map(|(_, lang)| lang.to_string())
            .unwrap_or_else(|| name.clone());

        results.push(ConfigCellResult {
            run_id,
            cell_id,
            language,
            status: row.get("status"),
            started_at: row.get("started_at"),
            finished_at: row.get("finished_at"),
            summaries,
        });
    }

    Ok(results)
}

#[derive(Debug, Serialize)]
pub struct ConfigCellResult {
    pub run_id: Uuid,
    pub cell_id: Option<Uuid>,
    pub language: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub summaries: Vec<BenchmarkSummary>,
}

pub async fn list(
    client: &Client,
    project_id: &Uuid,
    target_host: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<BenchmarkRunSummary>> {
    let rows = match client
        .query(
            "SELECT br.BenchmarkRunId, br.GeneratedAt, br.TargetUrl, br.TargetHost,
                    br.Modes, br.Concurrency, br.TotalRuns, br.ContractVersion,
                    COALESCE(launch.Scenario, 'unknown') AS Scenario,
                    COALESCE(launch.PrimaryPhase, 'measured') AS PrimaryPhase,
                    COALESCE(br.MethodologyJson->>'phase_model', '') AS PhaseModel,
                    NULLIF(br.MethodologyJson->'execution_plan'->>'source', '') AS ExecutionPlanSource,
                    NULLIF(env.EnvironmentJson->'server_info'->>'region', '') AS ServerRegion,
                    COALESCE(
                        NULLIF(env.EnvironmentJson->'network_baseline'->>'network_type', ''),
                        NULLIF(env.EnvironmentJson->'environment_check'->>'network_type', ''),
                        NULLIF(env.EnvironmentJson->'stability_check'->>'network_type', '')
                    ) AS NetworkType,
                    COALESCE(
                        (env.EnvironmentJson->'network_baseline'->>'rtt_p50_ms')::DOUBLE PRECISION,
                        (env.EnvironmentJson->'environment_check'->>'rtt_p50_ms')::DOUBLE PRECISION,
                        (env.EnvironmentJson->'stability_check'->>'rtt_p50_ms')::DOUBLE PRECISION
                    ) AS BaselineRttP50Ms,
                    COALESCE(case_counts.TotalCases, 0) AS TotalCases,
                    COALESCE(sample_counts.TotalSamples, 0) AS TotalSamples,
                    COALESCE(quality.PublicationReady, FALSE) AS PublicationReady,
                    COALESCE(quality.NoiseLevel, 'unknown') AS NoiseLevel,
                    COALESCE(quality.Sufficiency, 'unknown') AS Sufficiency,
                    COALESCE(jsonb_array_length(COALESCE(quality.QualityJson->'publication_blockers', '[]'::jsonb)), 0) AS PublicationBlockerCount,
                    COALESCE(quality.WarningsJson, '[]'::jsonb) AS WarningsJson
             FROM BenchmarkRun br
             LEFT JOIN LATERAL (
                 SELECT Scenario, PrimaryPhase
                 FROM BenchmarkLaunch bl
                 WHERE bl.BenchmarkRunId = br.BenchmarkRunId
                 ORDER BY bl.LaunchIndex
                 LIMIT 1
             ) launch ON TRUE
             LEFT JOIN (
                 SELECT BenchmarkRunId, COUNT(*)::BIGINT AS TotalCases
                 FROM BenchmarkCase
                 GROUP BY BenchmarkRunId
             ) case_counts ON case_counts.BenchmarkRunId = br.BenchmarkRunId
             LEFT JOIN (
                 SELECT BenchmarkRunId, COUNT(*)::BIGINT AS TotalSamples
                 FROM BenchmarkSample
                 GROUP BY BenchmarkRunId
             ) sample_counts ON sample_counts.BenchmarkRunId = br.BenchmarkRunId
             LEFT JOIN BenchmarkEnvironment env
               ON env.BenchmarkRunId = br.BenchmarkRunId
             LEFT JOIN BenchmarkDataQuality quality
               ON quality.BenchmarkRunId = br.BenchmarkRunId
             WHERE EXISTS (
                 SELECT 1
                 FROM job j
                 WHERE j.project_id = $1
                   AND j.run_id = br.BenchmarkRunId
             )
               AND ($2::VARCHAR IS NULL OR br.TargetHost = $2)
             ORDER BY br.GeneratedAt DESC
             LIMIT $3 OFFSET $4",
            &[project_id, &target_host, &limit, &offset],
        )
        .await
    {
        Ok(rows) => rows,
        Err(err) if is_missing_benchmark_table_error(&err) => return Ok(vec![]),
        Err(err) => return Err(err.into()),
    };

    rows.into_iter()
        .map(|row| {
            Ok(BenchmarkRunSummary {
                run_id: row.get("benchmarkrunid"),
                generated_at: row.get("generatedat"),
                target_url: row.get("targeturl"),
                target_host: row.get("targethost"),
                modes: split_csv_modes(&row.get::<_, String>("modes")),
                concurrency: to_u32(row.get::<_, i32>("concurrency")),
                total_runs: to_u32(row.get::<_, i32>("totalruns")),
                contract_version: row.get("contractversion"),
                scenario: row.get("scenario"),
                primary_phase: row.get("primaryphase"),
                phase_model: row.get("phasemodel"),
                execution_plan_source: row.get("executionplansource"),
                server_region: row.get("serverregion"),
                network_type: row.get("networktype"),
                baseline_rtt_p50_ms: row.get("baselinerttp50ms"),
                total_cases: to_u64(row.get::<_, i64>("totalcases")),
                total_samples: to_u64(row.get::<_, i64>("totalsamples")),
                publication_ready: row.get("publicationready"),
                noise_level: row.get("noiselevel"),
                sufficiency: row.get("sufficiency"),
                publication_blocker_count: row.get::<_, i32>("publicationblockercount") as u64,
                warnings: serde_json::from_value(row.get("warningsjson"))
                    .context("deserialize BenchmarkDataQuality.warnings")?,
            })
        })
        .collect()
}

pub async fn get_artifact(
    client: &Client,
    project_id: &Uuid,
    run_id: &Uuid,
) -> anyhow::Result<Option<BenchmarkArtifact>> {
    let run_row = match client
        .query_opt(
            "SELECT br.BenchmarkRunId, br.ContractVersion, br.GeneratedAt, br.Source,
                    br.TargetUrl, br.TargetHost, br.Modes, br.TotalRuns, br.Concurrency,
                    br.TimeoutMs, br.ClientOs, br.ClientVersion, br.MethodologyJson,
                    br.DiagnosticsJson, br.AggregateSummaryJson
             FROM BenchmarkRun br
             WHERE br.BenchmarkRunId = $1
               AND EXISTS (
                   SELECT 1
                   FROM job j
                   WHERE j.project_id = $2
                     AND j.run_id = br.BenchmarkRunId
               )",
            &[run_id, project_id],
        )
        .await
    {
        Ok(row) => row,
        Err(err) if is_missing_benchmark_table_error(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    let Some(run_row) = run_row else {
        return Ok(None);
    };

    let methodology: BenchmarkMethodology = serde_json::from_value(run_row.get("methodologyjson"))
        .context("deserialize BenchmarkRun.methodology_json")?;
    let diagnostics: BenchmarkDiagnostics = serde_json::from_value(run_row.get("diagnosticsjson"))
        .context("deserialize BenchmarkRun.diagnostics_json")?;
    let summary: BenchmarkSummary = serde_json::from_value(run_row.get("aggregatesummaryjson"))
        .context("deserialize BenchmarkRun.aggregate_summary_json")?;

    let environment = match client
        .query_opt(
            "SELECT EnvironmentJson
             FROM BenchmarkEnvironment
             WHERE BenchmarkRunId = $1",
            &[run_id],
        )
        .await
    {
        Ok(Some(environment_row)) => serde_json::from_value(environment_row.get("environmentjson"))
            .context("deserialize BenchmarkEnvironment.environment_json")?,
        Ok(None) => {
            tracing::warn!(run_id = %run_id, "BenchmarkEnvironment row missing; using empty fallback");
            empty_benchmark_environment()
        }
        Err(err) if is_missing_benchmark_table_error(&err) => {
            tracing::warn!(run_id = %run_id, "BenchmarkEnvironment table missing; using empty fallback");
            empty_benchmark_environment()
        }
        Err(err) => return Err(err).context("load BenchmarkEnvironment"),
    };

    let data_quality = match client
        .query_opt(
            "SELECT QualityJson
             FROM BenchmarkDataQuality
             WHERE BenchmarkRunId = $1",
            &[run_id],
        )
        .await
    {
        Ok(Some(quality_row)) => serde_json::from_value(quality_row.get("qualityjson"))
            .context("deserialize BenchmarkDataQuality.quality_json")?,
        Ok(None) => {
            tracing::warn!(run_id = %run_id, "BenchmarkDataQuality row missing; using empty fallback");
            empty_benchmark_data_quality()
        }
        Err(err) if is_missing_benchmark_table_error(&err) => {
            tracing::warn!(run_id = %run_id, "BenchmarkDataQuality table missing; using empty fallback");
            empty_benchmark_data_quality()
        }
        Err(err) => return Err(err).context("load BenchmarkDataQuality"),
    };

    let launch_rows = client
        .query(
            "SELECT LaunchIndex, Scenario, PrimaryPhase, StartedAt, FinishedAt,
                    SampleCount, PrimarySampleCount, WarmupSampleCount,
                    SuccessCount, FailureCount, PhasesJson
             FROM BenchmarkLaunch
             WHERE BenchmarkRunId = $1
             ORDER BY LaunchIndex",
            &[run_id],
        )
        .await
        .context("load BenchmarkLaunch rows")?;
    let launches = launch_rows
        .into_iter()
        .map(|row| -> anyhow::Result<BenchmarkLaunch> {
            Ok(BenchmarkLaunch {
                launch_index: to_u32(row.get::<_, i32>("launchindex")),
                scenario: row.get("scenario"),
                primary_phase: row.get("primaryphase"),
                started_at: row.get("startedat"),
                finished_at: row.get("finishedat"),
                phases_present: serde_json::from_value(row.get("phasesjson"))
                    .context("deserialize BenchmarkLaunch.phases_json")?,
                sample_count: to_u64(row.get::<_, i64>("samplecount")),
                primary_sample_count: to_u64(row.get::<_, i64>("primarysamplecount")),
                warmup_sample_count: to_u64(row.get::<_, i64>("warmupsamplecount")),
                success_count: to_u64(row.get::<_, i64>("successcount")),
                failure_count: to_u64(row.get::<_, i64>("failurecount")),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let case_rows = client
        .query(
            "SELECT CaseJson
             FROM BenchmarkCase
             WHERE BenchmarkRunId = $1
             ORDER BY Protocol, PayloadBytes NULLS FIRST, HttpStack NULLS FIRST, CaseId",
            &[run_id],
        )
        .await
        .context("load BenchmarkCase rows")?;
    let cases = case_rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row.get("casejson"))
                .context("deserialize BenchmarkCase.case_json")
        })
        .collect::<anyhow::Result<Vec<BenchmarkCase>>>()?;

    let sample_rows = client
        .query(
            "SELECT SampleJson
             FROM BenchmarkSample
             WHERE BenchmarkRunId = $1
             ORDER BY LaunchIndex, IterationIndex, AttemptId",
            &[run_id],
        )
        .await
        .context("load BenchmarkSample rows")?;
    let samples = sample_rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row.get("samplejson"))
                .context("deserialize BenchmarkSample.sample_json")
        })
        .collect::<anyhow::Result<Vec<BenchmarkSample>>>()?;

    let summary_rows = client
        .query(
            "SELECT SummaryJson
             FROM BenchmarkSummary
             WHERE BenchmarkRunId = $1
             ORDER BY Protocol, PayloadBytes NULLS FIRST, HttpStack NULLS FIRST, CaseId",
            &[run_id],
        )
        .await
        .context("load BenchmarkSummary rows")?;
    let summaries = summary_rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row.get("summaryjson"))
                .context("deserialize BenchmarkSummary.summary_json")
        })
        .collect::<anyhow::Result<Vec<BenchmarkSummary>>>()?;

    Ok(Some(BenchmarkArtifact {
        metadata: BenchmarkMetadata {
            contract_version: run_row.get("contractversion"),
            generated_at: run_row.get("generatedat"),
            run_id: run_row.get("benchmarkrunid"),
            source: run_row.get("source"),
            target_url: run_row.get("targeturl"),
            target_host: run_row.get("targethost"),
            modes: split_csv_modes(&run_row.get::<_, String>("modes")),
            total_runs: to_u32(run_row.get::<_, i32>("totalruns")),
            concurrency: to_u32(run_row.get::<_, i32>("concurrency")),
            timeout_ms: to_u64(run_row.get::<_, i64>("timeoutms")),
            client_os: run_row.get("clientos"),
            client_version: run_row.get("clientversion"),
        },
        environment,
        methodology,
        launches,
        cases,
        samples,
        summaries,
        comparisons: Vec::new(),
        data_quality,
        diagnostics,
        summary,
    }))
}

pub async fn compare(
    client: &Client,
    project_id: &Uuid,
    run_ids: &[Uuid],
    baseline_run_id: Option<Uuid>,
) -> anyhow::Result<BenchmarkComparisonReport> {
    let mut artifacts = Vec::new();
    for run_id in run_ids {
        let artifact = get_artifact(client, project_id, run_id)
            .await?
            .with_context(|| format!("benchmark run {} not found", run_id))?;
        artifacts.push(artifact);
    }

    tokio::task::spawn_blocking(move || build_comparison_report(&artifacts, baseline_run_id))
        .await
        .context("benchmark comparison task join failure")?
}

pub(crate) fn build_comparison_report(
    artifacts: &[BenchmarkArtifact],
    baseline_run_id: Option<Uuid>,
) -> anyhow::Result<BenchmarkComparisonReport> {
    anyhow::ensure!(
        artifacts.len() >= 2,
        "at least two benchmark runs are required for comparison"
    );

    let baseline_run_id = baseline_run_id.unwrap_or(artifacts[0].metadata.run_id);
    let baseline_index = artifacts
        .iter()
        .position(|artifact| artifact.metadata.run_id == baseline_run_id)
        .with_context(|| {
            format!(
                "baseline run {} is not present in selection",
                baseline_run_id
            )
        })?;
    let baseline = &artifacts[baseline_index];

    let runs = artifacts
        .iter()
        .map(compared_run_summary)
        .collect::<Vec<_>>();
    let baseline_case_map = baseline
        .cases
        .iter()
        .map(|case| (case.id.clone(), case))
        .collect::<BTreeMap<_, _>>();
    let baseline_summary_map = baseline
        .summaries
        .iter()
        .map(|summary| (summary.case_id.clone(), summary))
        .collect::<BTreeMap<_, _>>();

    let mut cases = Vec::new();
    let mut gated_candidate_count = 0usize;
    for (case_id, case) in baseline_case_map {
        let Some(baseline_summary) = baseline_summary_map.get(&case_id) else {
            continue;
        };

        let baseline_distribution = summarise_values(&included_sample_values(baseline, &case_id));
        let baseline_view =
            case_run_view(baseline, baseline_summary, baseline_distribution.clone());

        let mut candidates = Vec::new();
        for artifact in artifacts
            .iter()
            .filter(|artifact| artifact.metadata.run_id != baseline_run_id)
        {
            let Some(candidate_summary) = summary_for_case(artifact, &case_id) else {
                continue;
            };

            let candidate_distribution =
                summarise_values(&included_sample_values(artifact, &case_id));
            let run_view =
                case_run_view(artifact, candidate_summary, candidate_distribution.clone());
            let comparability_notes =
                environment_comparability_notes(&run_view.environment, &baseline_view.environment);
            let comparable = comparability_notes.is_empty();
            if !comparable {
                gated_candidate_count += 1;
            }

            let (absolute_delta, percent_delta, ratio, verdict) = if comparable {
                let absolute_delta = candidate_distribution.median - baseline_distribution.median;
                let percent_delta = if baseline_distribution.median.abs() > f64::EPSILON {
                    (absolute_delta / baseline_distribution.median) * 100.0
                } else {
                    0.0
                };
                let ratio = if baseline_distribution.median.abs() > f64::EPSILON {
                    candidate_distribution.median / baseline_distribution.median
                } else {
                    0.0
                };
                (
                    Some(absolute_delta),
                    Some(percent_delta),
                    Some(ratio),
                    comparison_verdict(
                        &baseline_distribution,
                        &candidate_distribution,
                        case.higher_is_better,
                        percent_delta,
                    ),
                )
            } else {
                (None, None, None, "not comparable".to_string())
            };

            candidates.push(BenchmarkCaseCandidateComparison {
                run: run_view,
                absolute_delta,
                percent_delta,
                ratio,
                comparable,
                comparability_notes,
                verdict,
            });
        }

        if !candidates.is_empty() {
            cases.push(BenchmarkCaseComparison {
                case_id,
                protocol: case.protocol.clone(),
                payload_bytes: case.payload_bytes,
                http_stack: case.http_stack.clone(),
                metric_name: case.metric_name.clone(),
                metric_unit: case.metric_unit.clone(),
                higher_is_better: case.higher_is_better,
                baseline: baseline_view,
                candidates,
            });
        }
    }

    Ok(BenchmarkComparisonReport {
        baseline_run_id,
        comparability_policy: format!(
            "Cross-run comparisons require matching client/server OS, architecture, CPU core counts, regions, network type, and no more than {:.1}x drift in baseline RTT medians before benchmark deltas are treated as publication-grade.",
            MAX_COMPARABLE_RTT_RATIO
        ),
        gated_candidate_count,
        runs,
        cases,
    })
}

fn compared_run_summary(artifact: &BenchmarkArtifact) -> ComparedBenchmarkRun {
    ComparedBenchmarkRun {
        run_id: artifact.metadata.run_id,
        generated_at: artifact.metadata.generated_at,
        target_host: artifact.metadata.target_host.clone(),
        scenario: artifact.methodology.scenario.clone(),
        primary_phase: artifact.methodology.sample_phase.clone(),
        phase_model: artifact.methodology.phase_model.clone(),
        publication_ready: artifact.data_quality.publication_ready,
        noise_level: artifact.data_quality.noise_level.clone(),
        sufficiency: artifact.data_quality.sufficiency.clone(),
        warning_count: artifact.data_quality.warnings.len(),
        environment: environment_fingerprint_view(&artifact.environment),
    }
}

fn case_run_view(
    artifact: &BenchmarkArtifact,
    summary: &BenchmarkSummary,
    distribution: BenchmarkDistributionStats,
) -> BenchmarkCaseRunView {
    BenchmarkCaseRunView {
        run_id: artifact.metadata.run_id,
        generated_at: artifact.metadata.generated_at,
        target_host: artifact.metadata.target_host.clone(),
        scenario: artifact.methodology.scenario.clone(),
        primary_phase: artifact.methodology.sample_phase.clone(),
        phase_model: artifact.methodology.phase_model.clone(),
        publication_ready: artifact.data_quality.publication_ready,
        noise_level: artifact.data_quality.noise_level.clone(),
        sufficiency: artifact.data_quality.sufficiency.clone(),
        warning_count: artifact.data_quality.warnings.len(),
        included_sample_count: summary.included_sample_count,
        failure_count: summary.failure_count,
        error_count: summary.error_count,
        rps: summary.rps,
        p95: summary.p95,
        p99: summary.p99,
        environment: environment_fingerprint_view(&artifact.environment),
        distribution,
    }
}

impl BenchmarkEnvironmentFingerprintView {
    fn is_empty(&self) -> bool {
        self.client_os.is_none()
            && self.client_arch.is_none()
            && self.client_cpu_cores.is_none()
            && self.client_region.is_none()
            && self.server_os.is_none()
            && self.server_arch.is_none()
            && self.server_cpu_cores.is_none()
            && self.server_region.is_none()
            && self.network_type.is_none()
            && self.baseline_rtt_p50_ms.is_none()
            && self.baseline_rtt_p95_ms.is_none()
    }
}

fn environment_fingerprint_view(
    environment: &BenchmarkEnvironment,
) -> BenchmarkEnvironmentFingerprintView {
    let network_type = environment
        .network_baseline
        .as_ref()
        .map(|baseline| baseline.network_type.to_string())
        .or_else(|| {
            environment
                .environment_check
                .as_ref()
                .map(|check| check.network_type.to_string())
        })
        .or_else(|| {
            environment
                .stability_check
                .as_ref()
                .map(|check| check.network_type.to_string())
        });
    let baseline_rtt_p50_ms = environment
        .network_baseline
        .as_ref()
        .map(|baseline| baseline.rtt_p50_ms)
        .or_else(|| {
            environment
                .environment_check
                .as_ref()
                .map(|check| check.rtt_p50_ms)
        })
        .or_else(|| {
            environment
                .stability_check
                .as_ref()
                .map(|check| check.rtt_p50_ms)
        });
    let baseline_rtt_p95_ms = environment
        .network_baseline
        .as_ref()
        .map(|baseline| baseline.rtt_p95_ms)
        .or_else(|| {
            environment
                .environment_check
                .as_ref()
                .map(|check| check.rtt_p95_ms)
        })
        .or_else(|| {
            environment
                .stability_check
                .as_ref()
                .map(|check| check.rtt_p95_ms)
        });

    BenchmarkEnvironmentFingerprintView {
        client_os: environment.client_info.as_ref().map(|info| info.os.clone()),
        client_arch: environment
            .client_info
            .as_ref()
            .map(|info| info.arch.clone()),
        client_cpu_cores: environment
            .client_info
            .as_ref()
            .and_then(|info| u32::try_from(info.cpu_cores).ok()),
        client_region: environment
            .client_info
            .as_ref()
            .and_then(|info| info.region.clone()),
        server_os: environment.server_info.as_ref().map(|info| info.os.clone()),
        server_arch: environment
            .server_info
            .as_ref()
            .map(|info| info.arch.clone()),
        server_cpu_cores: environment
            .server_info
            .as_ref()
            .and_then(|info| u32::try_from(info.cpu_cores).ok()),
        server_region: environment
            .server_info
            .as_ref()
            .and_then(|info| info.region.clone()),
        network_type,
        baseline_rtt_p50_ms,
        baseline_rtt_p95_ms,
    }
}

fn push_optional_string_mismatch(
    notes: &mut Vec<String>,
    label: &str,
    candidate: Option<&str>,
    baseline: Option<&str>,
) {
    if let (Some(candidate), Some(baseline)) = (candidate, baseline) {
        if !candidate.eq_ignore_ascii_case(baseline) {
            notes.push(format!("{label} differs ({candidate} vs {baseline})"));
        }
    }
}

fn push_optional_u32_mismatch(
    notes: &mut Vec<String>,
    label: &str,
    candidate: Option<u32>,
    baseline: Option<u32>,
) {
    if let (Some(candidate), Some(baseline)) = (candidate, baseline) {
        if candidate != baseline {
            notes.push(format!("{label} differs ({candidate} vs {baseline})"));
        }
    }
}

fn materially_different_ratio(candidate: Option<f64>, baseline: Option<f64>) -> Option<f64> {
    let (candidate, baseline) = (candidate?, baseline?);
    if candidate <= f64::EPSILON || baseline <= f64::EPSILON {
        return None;
    }
    Some(candidate.max(baseline) / candidate.min(baseline))
}

fn environment_comparability_notes(
    candidate: &BenchmarkEnvironmentFingerprintView,
    baseline: &BenchmarkEnvironmentFingerprintView,
) -> Vec<String> {
    let mut notes = Vec::new();

    if candidate.is_empty() {
        notes.push("candidate benchmark environment fingerprint is missing".to_string());
    }
    if baseline.is_empty() {
        notes.push("baseline benchmark environment fingerprint is missing".to_string());
    }
    if !notes.is_empty() {
        return notes;
    }

    push_optional_string_mismatch(
        &mut notes,
        "client OS",
        candidate.client_os.as_deref(),
        baseline.client_os.as_deref(),
    );
    push_optional_string_mismatch(
        &mut notes,
        "client architecture",
        candidate.client_arch.as_deref(),
        baseline.client_arch.as_deref(),
    );
    push_optional_u32_mismatch(
        &mut notes,
        "client CPU cores",
        candidate.client_cpu_cores,
        baseline.client_cpu_cores,
    );
    push_optional_string_mismatch(
        &mut notes,
        "client region",
        candidate.client_region.as_deref(),
        baseline.client_region.as_deref(),
    );
    push_optional_string_mismatch(
        &mut notes,
        "server OS",
        candidate.server_os.as_deref(),
        baseline.server_os.as_deref(),
    );
    push_optional_string_mismatch(
        &mut notes,
        "server architecture",
        candidate.server_arch.as_deref(),
        baseline.server_arch.as_deref(),
    );
    push_optional_u32_mismatch(
        &mut notes,
        "server CPU cores",
        candidate.server_cpu_cores,
        baseline.server_cpu_cores,
    );
    push_optional_string_mismatch(
        &mut notes,
        "server region",
        candidate.server_region.as_deref(),
        baseline.server_region.as_deref(),
    );

    match (
        candidate.network_type.as_deref(),
        baseline.network_type.as_deref(),
    ) {
        (Some(candidate), Some(baseline)) if !candidate.eq_ignore_ascii_case(baseline) => {
            notes.push(format!("network type differs ({candidate} vs {baseline})"))
        }
        (None, _) | (_, None) => notes
            .push("network type is missing from the benchmark environment fingerprint".to_string()),
        _ => {}
    }

    if let Some(ratio) =
        materially_different_ratio(candidate.baseline_rtt_p50_ms, baseline.baseline_rtt_p50_ms)
    {
        if ratio > MAX_COMPARABLE_RTT_RATIO {
            notes.push(format!(
                "baseline RTT p50 differs materially ({:.2} ms vs {:.2} ms)",
                candidate.baseline_rtt_p50_ms.unwrap_or_default(),
                baseline.baseline_rtt_p50_ms.unwrap_or_default()
            ));
        }
    } else {
        notes.push(
            "baseline RTT p50 is missing from the benchmark environment fingerprint".to_string(),
        );
    }

    if let Some(ratio) =
        materially_different_ratio(candidate.baseline_rtt_p95_ms, baseline.baseline_rtt_p95_ms)
    {
        if ratio > MAX_COMPARABLE_RTT_RATIO {
            notes.push(format!(
                "baseline RTT p95 differs materially ({:.2} ms vs {:.2} ms)",
                candidate.baseline_rtt_p95_ms.unwrap_or_default(),
                baseline.baseline_rtt_p95_ms.unwrap_or_default()
            ));
        }
    }

    notes.sort();
    notes.dedup();
    notes
}

fn summary_for_case<'a>(
    artifact: &'a BenchmarkArtifact,
    case_id: &str,
) -> Option<&'a BenchmarkSummary> {
    artifact
        .summaries
        .iter()
        .find(|summary| summary.case_id == case_id)
}

fn included_sample_values(artifact: &BenchmarkArtifact, case_id: &str) -> Vec<f64> {
    artifact
        .samples
        .iter()
        .filter(|sample| {
            sample.case_id == case_id
                && sample.phase == artifact.methodology.sample_phase
                && sample.inclusion_status.starts_with("included")
        })
        .filter_map(|sample| sample.metric_value)
        .collect()
}

fn summarise_values(values: &[f64]) -> BenchmarkDistributionStats {
    let mut sorted = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    if sorted.is_empty() {
        return BenchmarkDistributionStats {
            sample_count: 0,
            min: 0.0,
            mean: 0.0,
            median: 0.0,
            p95: 0.0,
            p99: 0.0,
            max: 0.0,
            stddev: 0.0,
            cv: 0.0,
            standard_error: 0.0,
            ci95_lower: 0.0,
            ci95_upper: 0.0,
        };
    }

    let sample_count = sorted.len() as u64;
    let min = sorted[0];
    let max = *sorted.last().unwrap_or(&min);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let median = percentile_from_sorted(&sorted, 50.0);
    let p95 = percentile_from_sorted(&sorted, 95.0);
    let p99 = percentile_from_sorted(&sorted, 99.0);
    let variance = if sorted.len() > 1 {
        sorted
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (sorted.len() as f64 - 1.0)
    } else {
        0.0
    };
    let stddev = variance.sqrt();
    let cv = if mean.abs() > f64::EPSILON {
        stddev / mean.abs()
    } else {
        0.0
    };
    let (standard_error, ci95_lower, ci95_upper) = bootstrap_median_interval(&sorted);

    BenchmarkDistributionStats {
        sample_count,
        min,
        mean,
        median,
        p95,
        p99,
        max,
        stddev,
        cv,
        standard_error,
        ci95_lower,
        ci95_upper,
    }
}

fn comparison_verdict(
    baseline: &BenchmarkDistributionStats,
    candidate: &BenchmarkDistributionStats,
    higher_is_better: bool,
    percent_delta: f64,
) -> String {
    if baseline.sample_count == 0 || candidate.sample_count == 0 {
        return "insufficient data".into();
    }

    if percent_delta.abs() <= 5.0 && confidence_intervals_overlap(baseline, candidate) {
        return "same within 5%".into();
    }

    let candidate_is_better = if higher_is_better {
        percent_delta >= 0.0
    } else {
        percent_delta <= 0.0
    };

    if candidate_is_better {
        "better".into()
    } else {
        "worse".into()
    }
}

fn confidence_intervals_overlap(
    baseline: &BenchmarkDistributionStats,
    candidate: &BenchmarkDistributionStats,
) -> bool {
    baseline.ci95_lower <= candidate.ci95_upper && candidate.ci95_lower <= baseline.ci95_upper
}

fn percentile_from_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = percentile / 100.0 * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
    }
}

fn median_from_sorted(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else if sorted.len().is_multiple_of(2) {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    }
}

fn bootstrap_median_interval(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if values.len() == 1 {
        return (0.0, values[0], values[0]);
    }

    let mut rng = DeterministicRng::from_values(values);
    let mut estimates = Vec::with_capacity(BOOTSTRAP_RESAMPLES);
    for _ in 0..BOOTSTRAP_RESAMPLES {
        let mut sample = Vec::with_capacity(values.len());
        for _ in 0..values.len() {
            sample.push(values[rng.next_index(values.len())]);
        }
        sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        estimates.push(median_from_sorted(&sample));
    }

    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let estimate_mean = estimates.iter().sum::<f64>() / estimates.len() as f64;
    let estimate_variance = estimates
        .iter()
        .map(|value| (value - estimate_mean).powi(2))
        .sum::<f64>()
        / (estimates.len() as f64 - 1.0);
    let standard_error = estimate_variance.sqrt();
    let tail = (1.0 - REPORT_CONFIDENCE_LEVEL) * 50.0;
    let lower = percentile_from_sorted(&estimates, tail);
    let upper = percentile_from_sorted(&estimates, 100.0 - tail);
    (standard_error, lower, upper)
}

fn split_csv_modes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn to_u32(value: i32) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn empty_benchmark_environment() -> BenchmarkEnvironment {
    BenchmarkEnvironment {
        client_info: None,
        server_info: None,
        network_baseline: None,
        environment_check: None,
        stability_check: None,
        packet_capture_enabled: false,
    }
}

fn empty_benchmark_data_quality() -> BenchmarkDataQuality {
    BenchmarkDataQuality {
        noise_level: "unknown".into(),
        sample_stability_cv: 0.0,
        sufficiency: "unknown".into(),
        warnings: Vec::new(),
        publication_ready: false,
        confidence_level: REPORT_CONFIDENCE_LEVEL,
        outlier_policy: DEFAULT_OUTLIER_POLICY.into(),
        uncertainty_method: DEFAULT_UNCERTAINTY_METHOD.into(),
        relative_margin_of_error: 0.0,
        quality_tier: "unknown".into(),
        low_outlier_count: 0,
        high_outlier_count: 0,
        outlier_count: 0,
        publication_blockers: Vec::new(),
    }
}

fn is_missing_benchmark_table_error(err: &tokio_postgres::Error) -> bool {
    err.as_db_error()
        .map(|db_err| db_err.code() == &SqlState::UNDEFINED_TABLE)
        .unwrap_or(false)
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
    let rows = client
        .query(
            "SELECT DISTINCT ON (br.language) br.language, br.runtime, br.metrics,
                    br.server_os, br.client_os, br.cloud, br.phase, br.concurrency
             FROM benchmark_result br
             JOIN benchmark_run brun ON brun.run_id = br.run_id
             WHERE brun.status = 'completed'
             ORDER BY br.language, brun.started_at DESC, br.started_at DESC NULLS LAST",
            &[],
        )
        .await?;

    let mut entries: Vec<LeaderboardEntry> = rows
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
        .collect();

    entries.sort_by(|a, b| {
        let a_ms = a
            .metrics
            .get("latency_mean_ms")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::MAX);
        let b_ms = b
            .metrics
            .get("latency_mean_ms")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::MAX);
        a_ms.partial_cmp(&b_ms).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{build_comparison_report, summarise_values};
    use chrono::Utc;
    use networker_tester::metrics::{
        BenchmarkEnvironmentCheck, BenchmarkExecutionPlan, BenchmarkNoiseThresholds,
        BenchmarkStabilityCheck, HostInfo, HttpResult, NetworkBaseline, NetworkType, Protocol,
        RequestAttempt, TestRun,
    };
    use networker_tester::output::json::{
        to_benchmark_artifact, BenchmarkArtifact, BenchmarkCase, BenchmarkDataQuality,
        BenchmarkDiagnostics, BenchmarkEnvironment, BenchmarkLaunch, BenchmarkMetadata,
        BenchmarkMethodology, BenchmarkSample, BenchmarkSummary,
    };
    use uuid::Uuid;

    fn benchmark_environment() -> BenchmarkEnvironment {
        BenchmarkEnvironment {
            client_info: Some(HostInfo {
                os: "macos".into(),
                arch: "aarch64".into(),
                cpu_cores: 12,
                total_memory_mb: Some(32_768),
                os_version: Some("15.0".into()),
                hostname: None,
                server_version: None,
                uptime_secs: None,
                region: Some("us-east".into()),
            }),
            server_info: Some(HostInfo {
                os: "ubuntu".into(),
                arch: "x86_64".into(),
                cpu_cores: 4,
                total_memory_mb: Some(8_192),
                os_version: Some("24.04".into()),
                hostname: None,
                server_version: Some("0.16.2".into()),
                uptime_secs: Some(60),
                region: Some("eastus".into()),
            }),
            network_baseline: Some(NetworkBaseline {
                samples: 10,
                rtt_min_ms: 0.8,
                rtt_avg_ms: 1.0,
                rtt_max_ms: 1.4,
                rtt_p50_ms: 0.9,
                rtt_p95_ms: 1.3,
                network_type: NetworkType::LAN,
            }),
            environment_check: None,
            stability_check: None,
            packet_capture_enabled: false,
        }
    }

    fn artifact(
        run_id: Uuid,
        target_host: &str,
        values: &[f64],
        scenario: &str,
    ) -> BenchmarkArtifact {
        let now = Utc::now();
        let case = BenchmarkCase {
            id: "http1:default:default".into(),
            protocol: "http1".into(),
            payload_bytes: None,
            http_stack: Some("axum".into()),
            metric_name: "Latency".into(),
            metric_unit: "ms".into(),
            higher_is_better: false,
        };
        let samples = values
            .iter()
            .enumerate()
            .map(|(idx, value)| BenchmarkSample {
                attempt_id: Uuid::new_v4(),
                case_id: case.id.clone(),
                launch_index: 0,
                phase: "measured".into(),
                iteration_index: idx as u32,
                success: true,
                retry_count: 0,
                inclusion_status: "included".into(),
                metric_value: Some(*value),
                metric_unit: "ms".into(),
                started_at: now,
                finished_at: Some(now),
                total_duration_ms: Some(*value),
                ttfb_ms: Some(*value),
            })
            .collect::<Vec<_>>();
        let dist = summarise_values(values);
        let summary = BenchmarkSummary {
            case_id: case.id.clone(),
            protocol: case.protocol.clone(),
            payload_bytes: case.payload_bytes,
            http_stack: case.http_stack.clone(),
            metric_name: case.metric_name.clone(),
            metric_unit: case.metric_unit.clone(),
            higher_is_better: case.higher_is_better,
            sample_count: values.len() as u64,
            included_sample_count: values.len() as u64,
            excluded_sample_count: 0,
            success_count: values.len() as u64,
            failure_count: 0,
            total_requests: values.len() as u64,
            error_count: 0,
            bytes_transferred: 0,
            wall_time_ms: 1000.0,
            rps: values.len() as f64,
            min: dist.min,
            mean: dist.mean,
            p5: dist.min,
            p25: dist.median,
            p50: dist.median,
            p75: dist.p95,
            p95: dist.p95,
            p99: dist.p99,
            p999: dist.max,
            max: dist.max,
            stddev: dist.stddev,
            standard_error: dist.standard_error,
            variance: dist.stddev * dist.stddev,
            cv: dist.cv,
            iqr: dist.p95 - dist.median,
            lower_fence: dist.min,
            upper_fence: dist.max,
            low_outlier_count: 0,
            high_outlier_count: 0,
            outlier_count: 0,
            skewness: 0.0,
            kurtosis: 0.0,
            mad: 0.0,
            ci95_lower: dist.ci95_lower,
            ci95_upper: dist.ci95_upper,
            relative_margin_of_error: 0.0,
            quality_tier: "excellent".into(),
            latency_mean_ms: Some(dist.mean),
            latency_p50_ms: Some(dist.median),
            latency_p99_ms: Some(dist.p99),
            latency_p999_ms: Some(dist.max),
            latency_max_ms: Some(dist.max),
        };

        BenchmarkArtifact {
            metadata: BenchmarkMetadata {
                contract_version: "1.2".into(),
                generated_at: now,
                run_id,
                source: "networker-tester".into(),
                target_url: format!("https://{target_host}/health"),
                target_host: target_host.into(),
                modes: vec!["http1".into()],
                total_runs: values.len() as u32,
                concurrency: 4,
                timeout_ms: 30_000,
                client_os: "linux".into(),
                client_version: "0.1.0".into(),
            },
            environment: BenchmarkEnvironment {
                ..benchmark_environment()
            },
            methodology: BenchmarkMethodology {
                mode: "benchmark".into(),
                phase_model: "warmup->measured".into(),
                sample_phase: "measured".into(),
                scenario: scenario.into(),
                launch_count: 1,
                phases_present: vec!["warmup".into(), "measured".into()],
                retries_recorded: true,
                higher_is_better_depends_on_workload: true,
                confidence_level: 0.95,
                outlier_policy:
                    "Tukey 1.5xIQR fences flag outliers for audit while raw measured samples remain preserved in the artifact."
                        .into(),
                uncertainty_method:
                    "Percentile bootstrap confidence intervals around the median with deterministic resampling."
                        .into(),
                execution_plan: None,
                noise_thresholds: None,
            },
            launches: vec![BenchmarkLaunch {
                launch_index: 0,
                scenario: scenario.into(),
                primary_phase: "measured".into(),
                started_at: now,
                finished_at: Some(now),
                phases_present: vec!["warmup".into(), "measured".into()],
                sample_count: values.len() as u64,
                primary_sample_count: values.len() as u64,
                warmup_sample_count: 0,
                success_count: values.len() as u64,
                failure_count: 0,
            }],
            cases: vec![case],
            samples,
            summaries: vec![summary.clone()],
            comparisons: vec![],
            data_quality: BenchmarkDataQuality {
                noise_level: "low".into(),
                sample_stability_cv: 0.01,
                sufficiency: "adequate".into(),
                warnings: vec![],
                publication_ready: true,
                confidence_level: 0.95,
                outlier_policy:
                    "Tukey 1.5xIQR fences flag outliers for audit while raw measured samples remain preserved in the artifact."
                        .into(),
                uncertainty_method:
                    "Percentile bootstrap confidence intervals around the median with deterministic resampling."
                        .into(),
                relative_margin_of_error: 0.0,
                quality_tier: "excellent".into(),
                low_outlier_count: 0,
                high_outlier_count: 0,
                outlier_count: 0,
                publication_blockers: vec![],
            },
            diagnostics: BenchmarkDiagnostics {
                raw_attempt_count: values.len() as u64,
                raw_success_count: values.len() as u64,
                raw_failure_count: 0,
            },
            summary,
        }
    }

    fn download_attempt(run_id: Uuid, throughput_mbps: f64) -> RequestAttempt {
        let now = Utc::now();
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Download,
            sequence_num: 0,
            started_at: now,
            finished_at: Some(now),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: Some(HttpResult {
                negotiated_version: "HTTP/2".into(),
                status_code: 200,
                headers_size_bytes: 128,
                body_size_bytes: 0,
                ttfb_ms: 8.0,
                total_duration_ms: 12.0,
                redirect_count: 0,
                started_at: now,
                response_headers: vec![],
                payload_bytes: 1_048_576,
                throughput_mbps: Some(throughput_mbps),
                goodput_mbps: Some(throughput_mbps * 0.92),
                cpu_time_ms: Some(1.5),
                csw_voluntary: Some(4),
                csw_involuntary: Some(1),
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: Some("axum".into()),
        }
    }

    fn benchmark_test_run(run_id: Uuid, throughput_mbps: f64) -> TestRun {
        let now = Utc::now();
        TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "https://bench.example/health".into(),
            target_host: "bench.example".into(),
            modes: vec!["download".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 30_000,
            client_os: "linux".into(),
            client_version: "0.16.2".into(),
            server_info: Some(HostInfo {
                os: "ubuntu".into(),
                arch: "x86_64".into(),
                cpu_cores: 8,
                total_memory_mb: Some(32_768),
                os_version: Some("24.04".into()),
                hostname: None,
                server_version: Some("0.16.2".into()),
                uptime_secs: Some(100),
                region: Some("eastus".into()),
            }),
            client_info: Some(HostInfo {
                os: "linux".into(),
                arch: "x86_64".into(),
                cpu_cores: 4,
                total_memory_mb: Some(16_384),
                os_version: Some("6.8".into()),
                hostname: None,
                server_version: None,
                uptime_secs: None,
                region: Some("us-east".into()),
            }),
            packet_capture_summary: None,
            baseline: Some(NetworkBaseline {
                samples: 10,
                rtt_min_ms: 0.8,
                rtt_avg_ms: 0.9,
                rtt_max_ms: 1.1,
                rtt_p50_ms: 0.9,
                rtt_p95_ms: 1.0,
                network_type: NetworkType::LAN,
            }),
            benchmark_environment_check: Some(BenchmarkEnvironmentCheck {
                attempted_samples: 5,
                successful_samples: 5,
                failed_samples: 0,
                duration_ms: 12.0,
                rtt_min_ms: 0.8,
                rtt_avg_ms: 0.9,
                rtt_max_ms: 1.1,
                rtt_p50_ms: 0.9,
                rtt_p95_ms: 1.0,
                packet_loss_percent: 0.0,
                network_type: NetworkType::LAN,
            }),
            benchmark_stability_check: Some(BenchmarkStabilityCheck {
                attempted_samples: 5,
                successful_samples: 5,
                failed_samples: 0,
                duration_ms: 18.0,
                rtt_min_ms: 0.8,
                rtt_avg_ms: 0.9,
                rtt_max_ms: 1.1,
                rtt_p50_ms: 0.9,
                rtt_p95_ms: 1.0,
                jitter_ms: 0.05,
                packet_loss_percent: 0.0,
                network_type: NetworkType::LAN,
            }),
            benchmark_phase: Some("measured".into()),
            benchmark_scenario: Some("warm".into()),
            benchmark_launch_index: Some(0),
            benchmark_warmup_attempt_count: 0,
            benchmark_pilot_attempt_count: 0,
            benchmark_overhead_attempt_count: 0,
            benchmark_cooldown_attempt_count: 0,
            benchmark_execution_plan: Some(BenchmarkExecutionPlan {
                source: "pilot-derived".into(),
                min_samples: 3,
                max_samples: 6,
                min_duration_ms: 1000,
                target_relative_error: Some(0.05),
                target_absolute_error: Some(2.0),
                pilot_sample_count: 5,
                pilot_elapsed_ms: Some(18.0),
            }),
            benchmark_noise_thresholds: Some(BenchmarkNoiseThresholds {
                max_packet_loss_percent: 1.0,
                max_jitter_ratio: 0.25,
                max_rtt_spread_ratio: 2.0,
            }),
            attempts: vec![download_attempt(run_id, throughput_mbps)],
        }
    }

    #[test]
    fn summarise_values_reports_ci_and_percentiles() {
        let stats = summarise_values(&[10.0, 12.0, 14.0, 16.0]);
        assert_eq!(stats.sample_count, 4);
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 16.0);
        assert_eq!(stats.median, 13.0);
        assert!(stats.p95 >= stats.median);
        assert!(stats.ci95_upper >= stats.ci95_lower);
    }

    #[test]
    fn summarise_values_ci_tracks_median_for_skewed_samples() {
        let stats = summarise_values(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 1_000.0]);
        assert_eq!(stats.median, 5.5);
        assert!(stats.ci95_lower <= stats.median);
        assert!(stats.ci95_upper >= stats.median);
        assert!(stats.ci95_upper < stats.mean);
    }

    #[test]
    fn comparison_report_uses_selected_baseline_and_marks_improvement() {
        let baseline_id = Uuid::new_v4();
        let candidate_id = Uuid::new_v4();
        let baseline = artifact(baseline_id, "baseline.local", &[20.0, 21.0, 22.0], "warm");
        let candidate = artifact(candidate_id, "candidate.local", &[10.0, 11.0, 12.0], "warm");

        let report = build_comparison_report(&[baseline, candidate], Some(baseline_id)).unwrap();
        assert_eq!(report.baseline_run_id, baseline_id);
        assert_eq!(report.cases.len(), 1);
        assert_eq!(report.cases[0].candidates.len(), 1);
        assert_eq!(report.gated_candidate_count, 0);
        assert_eq!(report.runs[0].phase_model, "warmup->measured");
        assert_eq!(
            report.runs[0].environment.network_type.as_deref(),
            Some("LAN")
        );
        assert_eq!(report.cases[0].candidates[0].verdict, "better");
        assert!(report.cases[0].candidates[0].comparable);
        assert!(
            report.cases[0].candidates[0]
                .percent_delta
                .unwrap_or_default()
                < 0.0
        );
    }

    #[test]
    fn comparison_report_gates_materially_different_environments() {
        let baseline_id = Uuid::new_v4();
        let candidate_id = Uuid::new_v4();
        let baseline = artifact(baseline_id, "baseline.local", &[20.0, 21.0, 22.0], "warm");
        let mut candidate = artifact(candidate_id, "candidate.local", &[10.0, 11.0, 12.0], "warm");
        if let Some(server_info) = candidate.environment.server_info.as_mut() {
            server_info.region = Some("westus".into());
        }
        if let Some(network_baseline) = candidate.environment.network_baseline.as_mut() {
            network_baseline.rtt_p50_ms = 2.0;
            network_baseline.rtt_p95_ms = 3.0;
        }

        let report = build_comparison_report(&[baseline, candidate], Some(baseline_id)).unwrap();
        let comparison = &report.cases[0].candidates[0];

        assert_eq!(report.gated_candidate_count, 1);
        assert!(!comparison.comparable);
        assert_eq!(comparison.verdict, "not comparable");
        assert!(comparison.ratio.is_none());
        assert!(comparison
            .comparability_notes
            .iter()
            .any(|note| note.contains("server region")));
        assert!(comparison
            .comparability_notes
            .iter()
            .any(|note| note.contains("baseline RTT p50")));
    }

    #[test]
    fn comparison_report_requires_at_least_two_runs() {
        let only = artifact(Uuid::new_v4(), "solo.local", &[10.0, 11.0], "warm");
        assert!(build_comparison_report(&[only], None).is_err());
    }

    #[test]
    fn tester_json_round_trips_into_dashboard_comparison_contract() {
        let baseline_run_id = Uuid::new_v4();
        let candidate_run_id = Uuid::new_v4();
        let baseline_run = benchmark_test_run(baseline_run_id, 95.0);
        let candidate_run = benchmark_test_run(candidate_run_id, 120.0);

        let baseline_json = serde_json::to_string(&to_benchmark_artifact(&baseline_run).unwrap())
            .expect("serialize baseline artifact");
        let candidate_json = serde_json::to_string(&to_benchmark_artifact(&candidate_run).unwrap())
            .expect("serialize candidate artifact");

        let baseline: BenchmarkArtifact =
            serde_json::from_str(&baseline_json).expect("deserialize baseline artifact");
        let candidate: BenchmarkArtifact =
            serde_json::from_str(&candidate_json).expect("deserialize candidate artifact");

        assert_eq!(
            baseline.methodology.phase_model,
            "environment-check->stability-check->measured"
        );
        assert_eq!(
            baseline
                .methodology
                .execution_plan
                .as_ref()
                .map(|plan| plan.source.as_str()),
            Some("pilot-derived")
        );
        assert_eq!(
            baseline
                .environment
                .network_baseline
                .as_ref()
                .map(|baseline| baseline.network_type),
            Some(NetworkType::LAN)
        );

        let report = build_comparison_report(
            &[baseline.clone(), candidate.clone()],
            Some(baseline_run_id),
        )
        .expect("build comparison report");
        let compared_run = report
            .runs
            .iter()
            .find(|run| run.run_id == baseline_run_id)
            .expect("baseline run should be present");
        assert_eq!(compared_run.phase_model, baseline.methodology.phase_model);
        assert_eq!(
            compared_run.environment.network_type.as_deref(),
            Some("LAN")
        );

        let case = &report.cases[0];
        let baseline_distribution = &case.baseline.distribution;
        assert_eq!(baseline_distribution.sample_count, 1);
        assert_eq!(baseline_distribution.median, baseline.summaries[0].p50);
        assert_eq!(
            baseline_distribution.ci95_lower,
            baseline.summaries[0].ci95_lower
        );
        assert_eq!(
            baseline_distribution.ci95_upper,
            baseline.summaries[0].ci95_upper
        );
        assert!(case.candidates[0].comparable);
        assert!(case.candidates[0].ratio.is_some());
    }
}
