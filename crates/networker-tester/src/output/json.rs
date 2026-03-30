use crate::metrics::{
    attempt_payload_bytes, primary_metric_label, primary_metric_value, BenchmarkEnvironmentCheck,
    BenchmarkExecutionPlan, BenchmarkNoiseThresholds, BenchmarkStabilityCheck, HostInfo,
    NetworkBaseline, Protocol, RequestAttempt, TestRun, UrlTestRun,
};
use crate::tls_profile::TlsEndpointProfile;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use uuid::Uuid;

pub const BENCHMARK_CONTRACT_VERSION: &str = "1.2";

const REPORT_CONFIDENCE_LEVEL: f64 = 0.95;
const TUKEY_FENCE_MULTIPLIER: f64 = 1.5;
const BOOTSTRAP_RESAMPLES: usize = 2_048;

fn default_benchmark_confidence_level() -> f64 {
    REPORT_CONFIDENCE_LEVEL
}

fn default_benchmark_outlier_policy() -> String {
    "Tukey 1.5xIQR fences flag outliers for audit while raw measured samples remain preserved in the artifact."
        .into()
}

fn default_benchmark_uncertainty_method() -> String {
    "Percentile bootstrap confidence intervals around the median with deterministic resampling."
        .into()
}

fn default_quality_tier() -> String {
    "unknown".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkArtifact {
    pub metadata: BenchmarkMetadata,
    pub environment: BenchmarkEnvironment,
    pub methodology: BenchmarkMethodology,
    pub launches: Vec<BenchmarkLaunch>,
    pub cases: Vec<BenchmarkCase>,
    pub samples: Vec<BenchmarkSample>,
    pub summaries: Vec<BenchmarkSummary>,
    #[serde(default)]
    pub comparisons: Vec<BenchmarkComparison>,
    pub data_quality: BenchmarkDataQuality,
    pub diagnostics: BenchmarkDiagnostics,
    /// Aggregate convenience summary for downstream tools that expect a single
    /// summary object for the entire run.
    pub summary: BenchmarkSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkMetadata {
    pub contract_version: String,
    pub generated_at: DateTime<Utc>,
    pub run_id: Uuid,
    pub source: String,
    pub target_url: String,
    pub target_host: String,
    pub modes: Vec<String>,
    pub total_runs: u32,
    pub concurrency: u32,
    pub timeout_ms: u64,
    pub client_os: String,
    pub client_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkEnvironment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<HostInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<HostInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_baseline: Option<NetworkBaseline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_check: Option<BenchmarkEnvironmentCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability_check: Option<BenchmarkStabilityCheck>,
    pub packet_capture_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkMethodology {
    pub mode: String,
    pub phase_model: String,
    pub sample_phase: String,
    pub scenario: String,
    pub launch_count: u32,
    pub phases_present: Vec<String>,
    pub retries_recorded: bool,
    pub higher_is_better_depends_on_workload: bool,
    #[serde(default = "default_benchmark_confidence_level")]
    pub confidence_level: f64,
    #[serde(default = "default_benchmark_outlier_policy")]
    pub outlier_policy: String,
    #[serde(default = "default_benchmark_uncertainty_method")]
    pub uncertainty_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_plan: Option<BenchmarkExecutionPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_thresholds: Option<BenchmarkNoiseThresholds>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkLaunch {
    pub launch_index: u32,
    pub scenario: String,
    pub primary_phase: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub phases_present: Vec<String>,
    pub sample_count: u64,
    pub primary_sample_count: u64,
    pub warmup_sample_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkCase {
    pub id: String,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_stack: Option<String>,
    pub metric_name: String,
    pub metric_unit: String,
    pub higher_is_better: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSample {
    pub attempt_id: Uuid,
    pub case_id: String,
    pub launch_index: u32,
    pub phase: String,
    pub iteration_index: u32,
    pub success: bool,
    pub retry_count: u32,
    pub inclusion_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_value: Option<f64>,
    pub metric_unit: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    pub case_id: String,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_stack: Option<String>,
    pub metric_name: String,
    pub metric_unit: String,
    pub higher_is_better: bool,
    pub sample_count: u64,
    pub included_sample_count: u64,
    pub excluded_sample_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub total_requests: u64,
    pub error_count: u64,
    pub bytes_transferred: u64,
    pub wall_time_ms: f64,
    pub rps: f64,
    pub min: f64,
    pub mean: f64,
    pub p5: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
    pub stddev: f64,
    #[serde(default)]
    pub standard_error: f64,
    #[serde(default)]
    pub variance: f64,
    #[serde(default)]
    pub cv: f64,
    #[serde(default)]
    pub iqr: f64,
    #[serde(default)]
    pub lower_fence: f64,
    #[serde(default)]
    pub upper_fence: f64,
    #[serde(default)]
    pub low_outlier_count: u64,
    #[serde(default)]
    pub high_outlier_count: u64,
    #[serde(default)]
    pub outlier_count: u64,
    #[serde(default)]
    pub skewness: f64,
    #[serde(default)]
    pub kurtosis: f64,
    #[serde(default)]
    pub mad: f64,
    #[serde(default)]
    pub ci95_lower: f64,
    #[serde(default)]
    pub ci95_upper: f64,
    #[serde(default)]
    pub relative_margin_of_error: f64,
    #[serde(default = "default_quality_tier")]
    pub quality_tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_mean_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_p99_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_p999_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_max_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BenchmarkComparison {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkDataQuality {
    pub noise_level: String,
    pub sample_stability_cv: f64,
    pub sufficiency: String,
    pub warnings: Vec<String>,
    pub publication_ready: bool,
    #[serde(default = "default_benchmark_confidence_level")]
    pub confidence_level: f64,
    #[serde(default = "default_benchmark_outlier_policy")]
    pub outlier_policy: String,
    #[serde(default = "default_benchmark_uncertainty_method")]
    pub uncertainty_method: String,
    #[serde(default)]
    pub relative_margin_of_error: f64,
    #[serde(default = "default_quality_tier")]
    pub quality_tier: String,
    #[serde(default)]
    pub low_outlier_count: u64,
    #[serde(default)]
    pub high_outlier_count: u64,
    #[serde(default)]
    pub outlier_count: u64,
    #[serde(default)]
    pub publication_blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkDiagnostics {
    pub raw_attempt_count: u64,
    pub raw_success_count: u64,
    pub raw_failure_count: u64,
}

#[derive(Debug, Clone)]
struct CaseAccumulator<'a> {
    case: BenchmarkCase,
    attempts: Vec<&'a RequestAttempt>,
}

#[derive(Debug, Clone)]
struct BenchmarkAttemptRecord<'a> {
    attempt: &'a RequestAttempt,
    case_id: String,
    phase: String,
    launch_index: u32,
}

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

/// Serialize a `TestRun` to pretty-printed JSON and write to `path`.
pub fn save(run: &TestRun, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = to_string(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string without writing to disk (useful for testing).
pub fn to_string(run: &TestRun) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

/// Return the normalized benchmark JSON contract without writing to disk.
pub fn to_benchmark_string(run: &TestRun) -> anyhow::Result<String> {
    let artifact = to_benchmark_artifact(run)?;
    Ok(serde_json::to_string_pretty(&artifact)?)
}

/// Return normalized benchmark JSON for one or more runs.
pub fn to_benchmark_string_many(runs: &[TestRun]) -> anyhow::Result<String> {
    if runs.len() == 1 {
        return to_benchmark_string(&runs[0]);
    }
    let artifacts: Vec<_> = runs
        .iter()
        .map(to_benchmark_artifact)
        .collect::<anyhow::Result<_>>()?;
    Ok(serde_json::to_string_pretty(&artifacts)?)
}

/// Serialize a `UrlTestRun` to pretty-printed JSON and write to `path`.
pub fn save_url_test(run: &UrlTestRun, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string for a `UrlTestRun` without writing to disk.
pub fn to_string_url_test(run: &UrlTestRun) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

/// Serialize a `TlsEndpointProfile` to pretty-printed JSON and write to `path`.
pub fn save_tls_profile(run: &TlsEndpointProfile, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = to_string_tls_profile(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string for a `TlsEndpointProfile` without writing to disk.
pub fn to_string_tls_profile(run: &TlsEndpointProfile) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

pub fn benchmark_artifact_if_present(run: &TestRun) -> anyhow::Result<Option<BenchmarkArtifact>> {
    if run.benchmark_phase.is_none()
        && run.benchmark_scenario.is_none()
        && run.benchmark_launch_index.is_none()
        && run.benchmark_warmup_attempt_count == 0
        && run.benchmark_pilot_attempt_count == 0
        && run.benchmark_overhead_attempt_count == 0
        && run.benchmark_cooldown_attempt_count == 0
        && run.benchmark_environment_check.is_none()
        && run.benchmark_execution_plan.is_none()
    {
        return Ok(None);
    }

    Ok(Some(to_benchmark_artifact(run)?))
}

pub fn to_benchmark_artifact(run: &TestRun) -> anyhow::Result<BenchmarkArtifact> {
    let primary_phase = run
        .benchmark_phase
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("benchmark artifact requires benchmark_phase"))?
        .to_string();
    let scenario = run
        .benchmark_scenario
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("benchmark artifact requires benchmark_scenario"))?
        .to_string();
    let launch_index = run.benchmark_launch_index.unwrap_or(0);

    let attempt_records: Vec<_> = run
        .attempts
        .iter()
        .enumerate()
        .map(|(idx, attempt)| {
            let case_id = benchmark_case_id(attempt);
            let warmup_end = run.benchmark_warmup_attempt_count as usize;
            let overhead_end = warmup_end + run.benchmark_overhead_attempt_count as usize;
            let pilot_end = overhead_end + run.benchmark_pilot_attempt_count as usize;
            let cooldown_start = run
                .attempts
                .len()
                .saturating_sub(run.benchmark_cooldown_attempt_count as usize);
            let phase = if primary_phase == "warmup" {
                "warmup".to_string()
            } else if primary_phase == "overhead" {
                "overhead".to_string()
            } else if primary_phase == "pilot" {
                "pilot".to_string()
            } else if primary_phase == "cooldown" {
                "cooldown".to_string()
            } else if idx < warmup_end {
                "warmup".to_string()
            } else if idx < overhead_end {
                "overhead".to_string()
            } else if idx < pilot_end {
                "pilot".to_string()
            } else if run.benchmark_cooldown_attempt_count > 0 && idx >= cooldown_start {
                "cooldown".to_string()
            } else {
                primary_phase.clone()
            };

            BenchmarkAttemptRecord {
                attempt,
                case_id,
                phase,
                launch_index,
            }
        })
        .collect();

    let mut cases: BTreeMap<String, CaseAccumulator<'_>> = BTreeMap::new();
    for record in &attempt_records {
        let case = benchmark_case_for_attempt(record.attempt);
        let entry = cases
            .entry(case.id.clone())
            .or_insert_with(|| CaseAccumulator {
                case,
                attempts: Vec::new(),
            });
        entry.attempts.push(record.attempt);
    }

    let case_list: Vec<_> = cases.values().map(|entry| entry.case.clone()).collect();
    let sample_list: Vec<_> = attempt_records
        .iter()
        .map(|record| benchmark_sample_for_attempt(record, &primary_phase))
        .collect();
    let summaries: Vec<_> = cases
        .values()
        .map(|entry| {
            let matching_attempts: Vec<_> = attempt_records
                .iter()
                .filter(|record| {
                    record.case_id == entry.case.id && record.phase.as_str() == primary_phase
                })
                .map(|record| record.attempt)
                .collect();
            benchmark_summary_from_attempts(entry.case.clone(), &matching_attempts)
        })
        .collect();

    let aggregate_case = BenchmarkCase {
        id: "aggregate".into(),
        protocol: "aggregate".into(),
        payload_bytes: None,
        http_stack: None,
        metric_name: aggregate_metric_name(&case_list),
        metric_unit: aggregate_metric_unit(&case_list),
        higher_is_better: case_list.iter().all(|case| case.higher_is_better),
    };
    let aggregate_attempts: Vec<_> = attempt_records
        .iter()
        .filter(|record| record.phase.as_str() == primary_phase)
        .map(|record| record.attempt)
        .collect();
    let summary = benchmark_summary_from_attempts(aggregate_case, &aggregate_attempts);
    let data_quality = benchmark_data_quality(run, &summary);
    let launches = vec![benchmark_launch_from_records(
        run,
        &attempt_records,
        &scenario,
        &primary_phase,
        launch_index,
    )];
    let phases_present = launches
        .first()
        .map(|launch| launch.phases_present.clone())
        .unwrap_or_else(|| vec![primary_phase.clone()]);

    Ok(BenchmarkArtifact {
        metadata: BenchmarkMetadata {
            contract_version: BENCHMARK_CONTRACT_VERSION.into(),
            generated_at: Utc::now(),
            run_id: run.run_id,
            source: "networker-tester".into(),
            target_url: run.target_url.clone(),
            target_host: run.target_host.clone(),
            modes: run.modes.clone(),
            total_runs: run.total_runs,
            concurrency: run.concurrency,
            timeout_ms: run.timeout_ms,
            client_os: run.client_os.clone(),
            client_version: run.client_version.clone(),
        },
        environment: BenchmarkEnvironment {
            client_info: run.client_info.clone(),
            server_info: run.server_info.clone(),
            network_baseline: run.baseline.clone(),
            environment_check: run.benchmark_environment_check.clone(),
            stability_check: run.benchmark_stability_check.clone(),
            packet_capture_enabled: run.packet_capture_summary.is_some(),
        },
        methodology: BenchmarkMethodology {
            mode: "benchmark".into(),
            phase_model: phase_model_for(&phases_present),
            sample_phase: primary_phase.clone(),
            scenario: scenario.clone(),
            launch_count: launches.len() as u32,
            phases_present,
            retries_recorded: true,
            higher_is_better_depends_on_workload: true,
            confidence_level: REPORT_CONFIDENCE_LEVEL,
            outlier_policy: default_benchmark_outlier_policy(),
            uncertainty_method: default_benchmark_uncertainty_method(),
            execution_plan: run.benchmark_execution_plan.clone(),
            noise_thresholds: run.benchmark_noise_thresholds.clone(),
        },
        launches,
        cases: case_list,
        samples: sample_list,
        summaries,
        comparisons: Vec::new(),
        data_quality,
        diagnostics: BenchmarkDiagnostics {
            raw_attempt_count: run.attempts.len() as u64,
            raw_success_count: run.success_count() as u64,
            raw_failure_count: run.failure_count() as u64,
        },
        summary,
    })
}

fn benchmark_case_for_attempt(attempt: &RequestAttempt) -> BenchmarkCase {
    let payload_bytes = attempt_payload_bytes(attempt);
    let http_stack = attempt.http_stack.clone();
    let protocol = attempt.protocol.to_string();
    let metric_name = primary_metric_label(&attempt.protocol).to_string();
    let metric_unit = metric_unit_for_protocol(&attempt.protocol).to_string();
    BenchmarkCase {
        id: benchmark_case_id(attempt),
        protocol,
        payload_bytes,
        http_stack,
        metric_name,
        metric_unit,
        higher_is_better: protocol_is_throughput(&attempt.protocol),
    }
}

fn benchmark_sample_for_attempt(
    record: &BenchmarkAttemptRecord<'_>,
    primary_phase: &str,
) -> BenchmarkSample {
    let metric_value = primary_metric_value(record.attempt);
    let inclusion_status = if record.phase != primary_phase {
        format!("excluded_phase_{}", record.phase.replace('-', "_"))
    } else if !record.attempt.success {
        "excluded_failure".to_string()
    } else if metric_value.is_none() {
        "excluded_missing_metric".to_string()
    } else if record.attempt.retry_count > 0 {
        "included_after_retry".to_string()
    } else {
        "included".to_string()
    };

    BenchmarkSample {
        attempt_id: record.attempt.attempt_id,
        case_id: record.case_id.clone(),
        launch_index: record.launch_index,
        phase: record.phase.clone(),
        iteration_index: record.attempt.sequence_num,
        success: record.attempt.success,
        retry_count: record.attempt.retry_count,
        inclusion_status,
        metric_value,
        metric_unit: metric_unit_for_protocol(&record.attempt.protocol).into(),
        started_at: record.attempt.started_at,
        finished_at: record.attempt.finished_at,
        total_duration_ms: record.attempt.total_duration_ms(),
        ttfb_ms: record.attempt.http.as_ref().map(|http| http.ttfb_ms),
    }
}

fn benchmark_launch_from_records(
    run: &TestRun,
    records: &[BenchmarkAttemptRecord<'_>],
    scenario: &str,
    primary_phase: &str,
    launch_index: u32,
) -> BenchmarkLaunch {
    let mut phases_present = Vec::new();
    if run.benchmark_environment_check.is_some() || run.baseline.is_some() {
        phases_present.push("environment-check".to_string());
    }
    if run.benchmark_stability_check.is_some()
        && !phases_present
            .iter()
            .any(|existing| existing == "stability-check")
    {
        phases_present.push("stability-check".to_string());
    }
    for phase in records.iter().map(|record| record.phase.as_str()) {
        if !phases_present.iter().any(|existing| existing == phase) {
            phases_present.push(phase.to_string());
        }
    }
    if phases_present.is_empty() {
        phases_present.push(primary_phase.to_string());
    }

    let warmup_sample_count = records
        .iter()
        .filter(|record| record.phase == "warmup")
        .count() as u64;
    let primary_records: Vec<_> = records
        .iter()
        .filter(|record| record.phase.as_str() == primary_phase)
        .collect();
    let success_count = primary_records
        .iter()
        .filter(|record| record.attempt.success)
        .count() as u64;
    let failure_count = primary_records.len() as u64 - success_count;

    BenchmarkLaunch {
        launch_index,
        scenario: scenario.to_string(),
        primary_phase: primary_phase.to_string(),
        started_at: run.started_at,
        finished_at: run.finished_at,
        phases_present,
        sample_count: records.len() as u64,
        primary_sample_count: primary_records.len() as u64,
        warmup_sample_count,
        success_count,
        failure_count,
    }
}

fn benchmark_summary_from_attempts(
    case: BenchmarkCase,
    attempts: &[&RequestAttempt],
) -> BenchmarkSummary {
    let mut included_values = Vec::new();
    let mut timestamps = Vec::new();
    let mut success_count = 0_u64;
    let mut failure_count = 0_u64;
    let mut bytes_transferred = 0_u64;

    for attempt in attempts {
        timestamps.push(attempt.started_at);
        if let Some(finished_at) = attempt.finished_at {
            timestamps.push(finished_at);
        }

        if attempt.success {
            success_count += 1;
        } else {
            failure_count += 1;
        }

        bytes_transferred += attempt_bytes_transferred(attempt);

        if attempt.success {
            if let Some(value) = primary_metric_value(attempt) {
                included_values.push(value);
            }
        }
    }

    included_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let sample_count = attempts.len() as u64;
    let included_sample_count = included_values.len() as u64;
    let excluded_sample_count = sample_count.saturating_sub(included_sample_count);
    let wall_time_ms = wall_time_ms(&timestamps);
    let rps = if wall_time_ms > 0.0 {
        sample_count as f64 / (wall_time_ms / 1000.0)
    } else {
        0.0
    };

    let mean = if included_values.is_empty() {
        0.0
    } else {
        included_values.iter().sum::<f64>() / included_values.len() as f64
    };
    let variance = if included_values.len() > 1 {
        included_values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (included_values.len() as f64 - 1.0)
    } else {
        0.0
    };
    let stddev = variance.sqrt();
    let min = included_values.first().copied().unwrap_or(0.0);
    let max = included_values.last().copied().unwrap_or(0.0);
    let p5 = percentile_from_sorted(&included_values, 5.0);
    let p25 = percentile_from_sorted(&included_values, 25.0);
    let p50 = percentile_from_sorted(&included_values, 50.0);
    let p75 = percentile_from_sorted(&included_values, 75.0);
    let p95 = percentile_from_sorted(&included_values, 95.0);
    let p99 = percentile_from_sorted(&included_values, 99.0);
    let p999 = percentile_from_sorted(&included_values, 99.9);
    let iqr = p75 - p25;
    let lower_fence = p25 - (iqr * TUKEY_FENCE_MULTIPLIER);
    let upper_fence = p75 + (iqr * TUKEY_FENCE_MULTIPLIER);
    let low_outlier_count = included_values
        .iter()
        .filter(|value| **value < lower_fence)
        .count() as u64;
    let high_outlier_count = included_values
        .iter()
        .filter(|value| **value > upper_fence)
        .count() as u64;
    let outlier_count = low_outlier_count + high_outlier_count;
    let cv = if mean.abs() > f64::EPSILON {
        stddev / mean.abs()
    } else {
        0.0
    };
    let mut deviations: Vec<f64> = included_values
        .iter()
        .map(|value| (value - p50).abs())
        .collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = median_from_sorted(&deviations);
    let skewness = if included_values.len() > 2 && stddev > f64::EPSILON {
        let third_moment = included_values
            .iter()
            .map(|value| (value - mean).powi(3))
            .sum::<f64>()
            / included_values.len() as f64;
        third_moment / stddev.powi(3)
    } else {
        0.0
    };
    let kurtosis = if included_values.len() > 3 && stddev > f64::EPSILON {
        let fourth_moment = included_values
            .iter()
            .map(|value| (value - mean).powi(4))
            .sum::<f64>()
            / included_values.len() as f64;
        (fourth_moment / stddev.powi(4)) - 3.0
    } else {
        0.0
    };
    let (standard_error, ci95_lower, ci95_upper) = bootstrap_median_interval(&included_values);
    let relative_margin_of_error = if p50.abs() > f64::EPSILON {
        ((ci95_upper - ci95_lower) / 2.0) / p50.abs()
    } else {
        0.0
    };
    let quality_tier = if included_values.is_empty() {
        default_quality_tier()
    } else {
        quality_tier_for_cv(cv)
    };
    let latency_aliases = if case.metric_unit == "ms" {
        (Some(mean), Some(p50), Some(p99), Some(p999), Some(max))
    } else {
        (None, None, None, None, None)
    };

    BenchmarkSummary {
        case_id: case.id,
        protocol: case.protocol,
        payload_bytes: case.payload_bytes,
        http_stack: case.http_stack,
        metric_name: case.metric_name,
        metric_unit: case.metric_unit,
        higher_is_better: case.higher_is_better,
        sample_count,
        included_sample_count,
        excluded_sample_count,
        success_count,
        failure_count,
        total_requests: sample_count,
        error_count: failure_count,
        bytes_transferred,
        wall_time_ms,
        rps,
        min,
        mean,
        p5,
        p25,
        p50,
        p75,
        p95,
        p99,
        p999,
        max,
        stddev,
        standard_error,
        variance,
        cv,
        iqr,
        lower_fence,
        upper_fence,
        low_outlier_count,
        high_outlier_count,
        outlier_count,
        skewness,
        kurtosis,
        mad,
        ci95_lower,
        ci95_upper,
        relative_margin_of_error,
        quality_tier,
        latency_mean_ms: latency_aliases.0,
        latency_p50_ms: latency_aliases.1,
        latency_p99_ms: latency_aliases.2,
        latency_p999_ms: latency_aliases.3,
        latency_max_ms: latency_aliases.4,
    }
}

fn packet_loss_exceeds_threshold(packet_loss_percent: f64, threshold_percent: f64) -> bool {
    if threshold_percent <= 0.0 {
        packet_loss_percent > 0.0
    } else {
        packet_loss_percent >= threshold_percent
    }
}

fn benchmark_data_quality(run: &TestRun, summary: &BenchmarkSummary) -> BenchmarkDataQuality {
    let mut warnings = Vec::new();
    let mut publication_blockers = Vec::new();
    let noise_thresholds = run.benchmark_noise_thresholds.clone().unwrap_or_default();

    let noise_level = if let Some(stability) = run.benchmark_stability_check.as_ref() {
        if stability.successful_samples == 0 {
            warnings.push("Stability-check did not collect any successful RTT samples".into());
            publication_blockers
                .push("Stability-check did not collect any successful RTT samples".into());
            "high".into()
        } else {
            let spread_ratio = if stability.rtt_p50_ms > 0.0 {
                stability.rtt_p95_ms / stability.rtt_p50_ms
            } else {
                1.0
            };
            let jitter_ratio = if stability.rtt_p50_ms > 0.0 {
                stability.jitter_ms / stability.rtt_p50_ms
            } else {
                0.0
            };

            if stability.packet_loss_percent >= 1.0 {
                warnings.push(format!(
                    "Stability-check observed {:.1}% packet loss",
                    stability.packet_loss_percent
                ));
            }
            if packet_loss_exceeds_threshold(
                stability.packet_loss_percent,
                noise_thresholds.max_packet_loss_percent,
            ) {
                publication_blockers.push(format!(
                    "Stability-check packet loss {:.1}% exceeds the configured publication threshold {:.1}%",
                    stability.packet_loss_percent, noise_thresholds.max_packet_loss_percent
                ));
            }
            if jitter_ratio >= 0.10 {
                warnings.push("Stability-check detected elevated RTT jitter".into());
            }
            if jitter_ratio >= noise_thresholds.max_jitter_ratio {
                publication_blockers.push(format!(
                    "Stability-check jitter ratio {:.2} exceeds the configured publication threshold {:.2}",
                    jitter_ratio, noise_thresholds.max_jitter_ratio
                ));
            }
            if spread_ratio >= 1.3 {
                warnings.push("Stability-check detected elevated RTT spread".into());
            }
            if spread_ratio >= noise_thresholds.max_rtt_spread_ratio {
                publication_blockers.push(format!(
                    "Stability-check RTT spread ratio {:.2} exceeds the configured publication threshold {:.2}",
                    spread_ratio, noise_thresholds.max_rtt_spread_ratio
                ));
            }

            if packet_loss_exceeds_threshold(
                stability.packet_loss_percent,
                noise_thresholds.max_packet_loss_percent,
            ) || jitter_ratio >= noise_thresholds.max_jitter_ratio
                || spread_ratio >= noise_thresholds.max_rtt_spread_ratio
            {
                "high".into()
            } else if stability.packet_loss_percent > 0.0
                || jitter_ratio >= 0.10
                || spread_ratio >= 1.3
            {
                "medium".into()
            } else {
                "low".into()
            }
        }
    } else if let Some(environment_check) = run.benchmark_environment_check.as_ref() {
        if environment_check.successful_samples == 0 {
            warnings.push("Environment-check did not collect any successful RTT samples".into());
            publication_blockers
                .push("Environment-check did not collect any successful RTT samples".into());
            "high".into()
        } else if packet_loss_exceeds_threshold(
            environment_check.packet_loss_percent,
            noise_thresholds.max_packet_loss_percent,
        ) {
            warnings.push(format!(
                "Environment-check observed {:.1}% packet loss",
                environment_check.packet_loss_percent
            ));
            publication_blockers.push(format!(
                "Environment-check packet loss {:.1}% exceeds the configured publication threshold {:.1}%",
                environment_check.packet_loss_percent, noise_thresholds.max_packet_loss_percent
            ));
            "high".into()
        } else if environment_check.packet_loss_percent > 0.0 {
            warnings.push(format!(
                "Environment-check observed {:.1}% packet loss",
                environment_check.packet_loss_percent
            ));
            "medium".into()
        } else {
            let ratio = if environment_check.rtt_p50_ms > 0.0 {
                environment_check.rtt_p95_ms / environment_check.rtt_p50_ms
            } else {
                1.0
            };
            if ratio >= noise_thresholds.max_rtt_spread_ratio {
                warnings.push("High environment-check RTT spread detected".into());
                publication_blockers.push(format!(
                    "Environment-check RTT spread ratio {:.2} exceeds the configured publication threshold {:.2}",
                    ratio, noise_thresholds.max_rtt_spread_ratio
                ));
                "high".into()
            } else if ratio >= 1.3 {
                warnings.push("Moderate environment-check RTT spread detected".into());
                "medium".into()
            } else {
                "low".into()
            }
        }
    } else {
        match run.baseline.as_ref() {
            Some(baseline) if baseline.samples == 0 => {
                warnings.push("Baseline RTT measurement did not collect samples".into());
                publication_blockers
                    .push("Baseline RTT measurement did not collect samples".into());
                "high".into()
            }
            Some(baseline) => {
                let ratio = if baseline.rtt_p50_ms > 0.0 {
                    baseline.rtt_p95_ms / baseline.rtt_p50_ms
                } else {
                    1.0
                };
                if ratio >= noise_thresholds.max_rtt_spread_ratio {
                    warnings.push("High baseline RTT spread detected".into());
                    publication_blockers.push(format!(
                        "Baseline RTT spread ratio {:.2} exceeds the configured publication threshold {:.2}",
                        ratio, noise_thresholds.max_rtt_spread_ratio
                    ));
                    "high".into()
                } else if ratio >= 1.3 {
                    warnings.push("Moderate baseline RTT spread detected".into());
                    "medium".into()
                } else {
                    "low".into()
                }
            }
            None => {
                warnings.push("No network baseline recorded".into());
                "medium".into()
            }
        }
    };

    let sample_stability_cv = summary.cv;

    if summary.failure_count > 0 {
        warnings.push(format!(
            "{} failed attempts were excluded from the primary metric summary",
            summary.failure_count
        ));
        publication_blockers.push("Measured phase contained failed attempts".into());
    }
    if sample_stability_cv > 0.15 {
        warnings.push("High sample variability detected".into());
    }
    if sample_stability_cv > 0.10 {
        publication_blockers.push("Sample variability exceeds the publication threshold".into());
    }
    if summary.outlier_count > 0 {
        warnings.push(format!(
            "{} measured samples were flagged outside Tukey 1.5xIQR fences",
            summary.outlier_count
        ));
    }
    if summary.relative_margin_of_error > 0.10 {
        warnings.push("Confidence interval is wide relative to the median".into());
    }
    if summary.relative_margin_of_error > 0.05 {
        publication_blockers
            .push("Confidence interval is too wide for publication-ready claims".into());
    }
    if summary.kurtosis > 1.0 {
        warnings.push("Heavy-tailed distribution detected in the primary metric".into());
    }
    if summary.skewness.abs() > 1.0 {
        warnings.push("Skewed distribution detected in the primary metric".into());
    }

    let sufficiency = if summary.included_sample_count >= 100 {
        "adequate"
    } else if summary.included_sample_count >= 30 {
        warnings.push("Sample count is marginal for publication-quality claims".into());
        "marginal"
    } else {
        warnings.push("Insufficient included samples for strong benchmark claims".into());
        "insufficient"
    };

    if sufficiency != "adequate" {
        publication_blockers.push(format!(
            "Sample sufficiency is {} rather than adequate",
            sufficiency
        ));
    }

    let publication_ready = publication_blockers.is_empty();

    BenchmarkDataQuality {
        noise_level,
        sample_stability_cv,
        sufficiency: sufficiency.into(),
        warnings,
        publication_ready,
        confidence_level: REPORT_CONFIDENCE_LEVEL,
        outlier_policy: default_benchmark_outlier_policy(),
        uncertainty_method: default_benchmark_uncertainty_method(),
        relative_margin_of_error: summary.relative_margin_of_error,
        quality_tier: summary.quality_tier.clone(),
        low_outlier_count: summary.low_outlier_count,
        high_outlier_count: summary.high_outlier_count,
        outlier_count: summary.outlier_count,
        publication_blockers,
    }
}

fn benchmark_case_id(attempt: &RequestAttempt) -> String {
    let payload = attempt_payload_bytes(attempt)
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "default".into());
    let stack = attempt
        .http_stack
        .as_deref()
        .unwrap_or("default")
        .replace(':', "_");
    format!("{}:{}:{}", attempt.protocol, payload, stack)
}

fn metric_unit_for_protocol(protocol: &Protocol) -> &'static str {
    if protocol_is_throughput(protocol) {
        "MB/s"
    } else {
        "ms"
    }
}

fn protocol_is_throughput(protocol: &Protocol) -> bool {
    matches!(
        protocol,
        Protocol::Download
            | Protocol::Download1
            | Protocol::Download2
            | Protocol::Download3
            | Protocol::Upload
            | Protocol::Upload1
            | Protocol::Upload2
            | Protocol::Upload3
            | Protocol::WebDownload
            | Protocol::WebUpload
            | Protocol::UdpDownload
            | Protocol::UdpUpload
    )
}

fn aggregate_metric_name(cases: &[BenchmarkCase]) -> String {
    let mut names = cases.iter().map(|case| case.metric_name.as_str());
    match names.next() {
        Some(first) if names.all(|name| name == first) => first.to_string(),
        Some(_) => "Primary metric".into(),
        None => "Primary metric".into(),
    }
}

fn aggregate_metric_unit(cases: &[BenchmarkCase]) -> String {
    let mut units = cases.iter().map(|case| case.metric_unit.as_str());
    match units.next() {
        Some(first) if units.all(|unit| unit == first) => first.to_string(),
        Some(_) => "mixed".into(),
        None => "unknown".into(),
    }
}

fn phase_model_for(phases_present: &[String]) -> String {
    if phases_present.is_empty() {
        "single-run-measured".into()
    } else {
        phases_present.join("->")
    }
}

fn median_from_sorted(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else if sorted.len() % 2 == 0 {
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
        sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        estimates.push(median_from_sorted(&sample));
    }

    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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

fn quality_tier_for_cv(cv: f64) -> String {
    if !cv.is_finite() {
        "unknown".into()
    } else if cv <= 0.03 {
        "excellent".into()
    } else if cv <= 0.08 {
        "good".into()
    } else if cv <= 0.15 {
        "fair".into()
    } else {
        "unreliable".into()
    }
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

fn wall_time_ms(timestamps: &[DateTime<Utc>]) -> f64 {
    match (timestamps.iter().min(), timestamps.iter().max()) {
        (Some(start), Some(end)) => (*end - *start)
            .num_microseconds()
            .map(|micros| micros as f64 / 1000.0)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

fn attempt_bytes_transferred(attempt: &RequestAttempt) -> u64 {
    if let Some(http) = &attempt.http {
        return (http.body_size_bytes + http.headers_size_bytes) as u64;
    }
    if let Some(udp) = &attempt.udp_throughput {
        return udp
            .bytes_acked
            .unwrap_or(udp.payload_bytes)
            .try_into()
            .unwrap_or(u64::MAX);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{
        BenchmarkEnvironmentCheck, BenchmarkExecutionPlan, BenchmarkNoiseThresholds,
        BenchmarkStabilityCheck, HttpResult, NetworkType, Protocol, RequestAttempt, TestRun,
        UrlConnectionSummary, UrlDiagnosticStatus, UrlOriginSummary, UrlPageLoadStrategy,
        UrlTestRun,
    };
    use crate::tls_profile::{
        TlsCapabilitiesSection, TlsCertificateSection, TlsChainDiagnostics, TlsClientAuthStatus,
        TlsEndpointProfile, TlsPathCharacteristics, TlsPathClassification, TlsProfileConnectivity,
        TlsProfileCoverageLevel, TlsProfileSummary, TlsProfileTarget, TlsProfileTargetKind,
        TlsProtocolSupport, TlsResumptionSection, TlsRevocationInfo, TlsSniBehavior,
        TlsTrustSection,
    };
    use chrono::Utc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;
    fn dummy_run() -> TestRun {
        let run_id = Uuid::new_v4();
        TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            packet_capture_summary: None,
            benchmark_environment_check: None,
            benchmark_stability_check: None,
            benchmark_phase: None,
            benchmark_scenario: None,
            benchmark_launch_index: None,
            benchmark_warmup_attempt_count: 0,
            benchmark_pilot_attempt_count: 0,
            benchmark_overhead_attempt_count: 0,
            benchmark_cooldown_attempt_count: 0,
            benchmark_execution_plan: None,
            benchmark_noise_thresholds: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
                http_stack: None,
            }],
        }
    }

    fn benchmark_dummy_run() -> TestRun {
        let mut run = dummy_run();
        run.benchmark_phase = Some("measured".into());
        run.benchmark_scenario = Some("warm".into());
        run.benchmark_launch_index = Some(0);
        run
    }

    #[test]
    fn json_round_trip() {
        let run = dummy_run();
        let json = to_string(&run).unwrap();
        let de: TestRun = serde_json::from_str(&json).unwrap();
        assert_eq!(de.run_id, run.run_id);
        assert_eq!(de.attempts.len(), 1);
    }

    #[test]
    fn benchmark_contract_round_trip_has_expected_sections() {
        let run = benchmark_dummy_run();
        let json = to_benchmark_string(&run).unwrap();
        let de: BenchmarkArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(de.metadata.contract_version, BENCHMARK_CONTRACT_VERSION);
        assert_eq!(de.launches.len(), 1);
        assert_eq!(de.cases.len(), 1);
        assert_eq!(de.samples.len(), 1);
        assert_eq!(de.summary.sample_count, 1);
        assert_eq!(de.diagnostics.raw_attempt_count, 1);
        assert_eq!(de.methodology.confidence_level, REPORT_CONFIDENCE_LEVEL);
        assert!(de.methodology.outlier_policy.contains("Tukey 1.5xIQR"));
    }

    #[test]
    fn benchmark_contract_tags_warmup_and_primary_phase() {
        let mut run = benchmark_dummy_run();
        run.benchmark_launch_index = Some(3);
        run.benchmark_warmup_attempt_count = 1;

        let mut warmup = run.attempts[0].clone();
        warmup.protocol = Protocol::PageLoad2;
        run.attempts.insert(0, warmup);

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(artifact.launches[0].launch_index, 3);
        assert_eq!(artifact.launches[0].scenario, "warm");
        assert_eq!(artifact.samples[0].phase, "warmup");
        assert_eq!(
            artifact.samples[0].inclusion_status,
            "excluded_phase_warmup"
        );
        assert_eq!(artifact.samples[1].phase, "measured");
        assert_eq!(artifact.summary.sample_count, 1);
        assert_eq!(artifact.launches[0].warmup_sample_count, 1);
    }

    #[test]
    fn benchmark_contract_tags_pilot_phase_and_execution_plan() {
        let mut run = benchmark_dummy_run();
        run.benchmark_pilot_attempt_count = 1;
        run.benchmark_execution_plan = Some(BenchmarkExecutionPlan {
            source: "pilot-derived".into(),
            min_samples: 4,
            max_samples: 12,
            min_duration_ms: 50,
            target_relative_error: Some(0.05),
            target_absolute_error: None,
            pilot_sample_count: 1,
            pilot_elapsed_ms: Some(12.0),
        });

        let mut pilot = run.attempts[0].clone();
        pilot.protocol = Protocol::Http2;
        run.attempts.insert(0, pilot);

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(artifact.samples[0].phase, "pilot");
        assert_eq!(artifact.samples[0].inclusion_status, "excluded_phase_pilot");
        assert_eq!(
            artifact.methodology.phases_present,
            vec!["pilot", "measured"]
        );
        assert_eq!(
            artifact
                .methodology
                .execution_plan
                .as_ref()
                .map(|plan| plan.source.as_str()),
            Some("pilot-derived")
        );
    }

    #[test]
    fn benchmark_contract_tags_overhead_and_cooldown_phases() {
        let mut run = benchmark_dummy_run();
        run.benchmark_overhead_attempt_count = 1;
        run.benchmark_cooldown_attempt_count = 1;

        let measured = run.attempts[0].clone();
        let mut overhead = measured.clone();
        overhead.protocol = Protocol::Http2;
        let mut cooldown = measured.clone();
        cooldown.protocol = Protocol::PageLoad2;
        run.attempts = vec![overhead, measured, cooldown];

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(artifact.samples[0].phase, "overhead");
        assert_eq!(
            artifact.samples[0].inclusion_status,
            "excluded_phase_overhead"
        );
        assert_eq!(artifact.samples[1].phase, "measured");
        assert_eq!(artifact.samples[2].phase, "cooldown");
        assert_eq!(
            artifact.samples[2].inclusion_status,
            "excluded_phase_cooldown"
        );
        assert_eq!(
            artifact.methodology.phases_present,
            vec!["overhead", "measured", "cooldown"]
        );
        assert_eq!(
            artifact.launches[0].phases_present,
            vec!["overhead", "measured", "cooldown"]
        );
        assert_eq!(artifact.summary.sample_count, 1);
    }

    #[test]
    fn benchmark_contract_includes_environment_check_in_environment_and_launches() {
        let mut run = benchmark_dummy_run();
        run.benchmark_environment_check = Some(BenchmarkEnvironmentCheck {
            attempted_samples: 5,
            successful_samples: 5,
            failed_samples: 0,
            duration_ms: 250.0,
            rtt_min_ms: 0.7,
            rtt_avg_ms: 0.9,
            rtt_max_ms: 1.1,
            rtt_p50_ms: 0.9,
            rtt_p95_ms: 1.0,
            packet_loss_percent: 0.0,
            network_type: NetworkType::Loopback,
        });

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(
            artifact
                .environment
                .environment_check
                .as_ref()
                .map(|check| check.successful_samples),
            Some(5)
        );
        assert_eq!(
            artifact.launches[0].phases_present,
            vec!["environment-check", "measured"]
        );
        assert_eq!(
            artifact.methodology.phases_present,
            vec!["environment-check", "measured"]
        );
    }

    #[test]
    fn benchmark_contract_includes_stability_check_in_environment_and_launches() {
        let mut run = benchmark_dummy_run();
        run.benchmark_stability_check = Some(BenchmarkStabilityCheck {
            attempted_samples: 12,
            successful_samples: 12,
            failed_samples: 0,
            duration_ms: 600.0,
            rtt_min_ms: 0.8,
            rtt_avg_ms: 1.0,
            rtt_max_ms: 1.2,
            rtt_p50_ms: 1.0,
            rtt_p95_ms: 1.1,
            jitter_ms: 0.02,
            packet_loss_percent: 0.0,
            network_type: NetworkType::Loopback,
        });

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(
            artifact
                .environment
                .stability_check
                .as_ref()
                .map(|check| check.successful_samples),
            Some(12)
        );
        assert_eq!(
            artifact.launches[0].phases_present,
            vec!["stability-check", "measured"]
        );
        assert_eq!(
            artifact.methodology.phases_present,
            vec!["stability-check", "measured"]
        );
    }

    #[test]
    fn benchmark_contract_uses_stability_check_for_noise_and_publication_blockers() {
        let run_id = Uuid::new_v4();
        let started_at = Utc::now();
        let attempts = [10.0, 11.0, 12.0, 25.0]
            .into_iter()
            .enumerate()
            .map(|(sequence_num, total_duration_ms)| RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: sequence_num as u32,
                started_at,
                finished_at: Some(started_at),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: Some(HttpResult {
                    negotiated_version: "HTTP/1.1".into(),
                    status_code: 200,
                    headers_size_bytes: 256,
                    body_size_bytes: 1024,
                    ttfb_ms: total_duration_ms / 2.0,
                    total_duration_ms,
                    redirect_count: 0,
                    started_at,
                    response_headers: vec![],
                    payload_bytes: 0,
                    throughput_mbps: None,
                    goodput_mbps: None,
                    cpu_time_ms: None,
                    csw_voluntary: None,
                    csw_involuntary: None,
                }),
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
                http_stack: None,
            })
            .collect();
        let run = TestRun {
            run_id,
            started_at,
            finished_at: Some(started_at),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 4,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: Some(NetworkBaseline {
                samples: 10,
                rtt_min_ms: 0.8,
                rtt_avg_ms: 1.0,
                rtt_max_ms: 1.2,
                rtt_p50_ms: 1.0,
                rtt_p95_ms: 1.1,
                network_type: NetworkType::Loopback,
            }),
            packet_capture_summary: None,
            benchmark_environment_check: None,
            benchmark_stability_check: Some(BenchmarkStabilityCheck {
                attempted_samples: 12,
                successful_samples: 11,
                failed_samples: 1,
                duration_ms: 700.0,
                rtt_min_ms: 0.9,
                rtt_avg_ms: 1.6,
                rtt_max_ms: 3.4,
                rtt_p50_ms: 1.0,
                rtt_p95_ms: 2.2,
                jitter_ms: 0.35,
                packet_loss_percent: 8.0,
                network_type: NetworkType::Loopback,
            }),
            benchmark_phase: Some("measured".into()),
            benchmark_scenario: Some("warm".into()),
            benchmark_launch_index: Some(0),
            benchmark_warmup_attempt_count: 0,
            benchmark_pilot_attempt_count: 0,
            benchmark_overhead_attempt_count: 0,
            benchmark_cooldown_attempt_count: 0,
            benchmark_execution_plan: None,
            benchmark_noise_thresholds: None,
            attempts,
        };

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(artifact.data_quality.noise_level, "high");
        assert!(artifact
            .data_quality
            .warnings
            .iter()
            .any(|warning| warning.contains("packet loss")));
        assert!(artifact
            .data_quality
            .warnings
            .iter()
            .any(|warning| warning.contains("jitter")));
        assert!(artifact
            .data_quality
            .warnings
            .iter()
            .any(|warning| warning.contains("spread")));
        assert!(artifact
            .data_quality
            .publication_blockers
            .iter()
            .any(|blocker| blocker.contains("packet loss")));
        assert!(artifact
            .data_quality
            .publication_blockers
            .iter()
            .any(|blocker| blocker.contains("jitter")));
        assert!(artifact
            .data_quality
            .publication_blockers
            .iter()
            .any(|blocker| blocker.contains("spread")));
        assert!(!artifact.data_quality.publication_ready);
    }

    #[test]
    fn benchmark_contract_uses_configured_noise_thresholds_for_publication_blockers() {
        let mut run = benchmark_dummy_run();
        run.benchmark_noise_thresholds = Some(BenchmarkNoiseThresholds {
            max_packet_loss_percent: 3.0,
            max_jitter_ratio: 0.40,
            max_rtt_spread_ratio: 1.5,
        });
        run.benchmark_environment_check = Some(BenchmarkEnvironmentCheck {
            attempted_samples: 8,
            successful_samples: 8,
            failed_samples: 0,
            duration_ms: 160.0,
            rtt_min_ms: 0.8,
            rtt_avg_ms: 1.1,
            rtt_max_ms: 2.1,
            rtt_p50_ms: 1.0,
            rtt_p95_ms: 1.6,
            packet_loss_percent: 4.0,
            network_type: NetworkType::Loopback,
        });

        let artifact = to_benchmark_artifact(&run).unwrap();

        assert_eq!(
            artifact
                .methodology
                .noise_thresholds
                .as_ref()
                .unwrap()
                .max_packet_loss_percent,
            3.0
        );
        assert_eq!(artifact.data_quality.noise_level, "high");
        assert!(!artifact.data_quality.publication_ready);
        assert!(artifact
            .data_quality
            .publication_blockers
            .iter()
            .any(|blocker| blocker.contains("configured publication threshold 3.0%")));
    }

    #[test]
    fn benchmark_contract_reports_richer_summary_and_quality_fields() {
        let run_id = Uuid::new_v4();
        let started_at = Utc::now();
        let attempts = [10.0, 11.0, 12.0, 25.0]
            .into_iter()
            .enumerate()
            .map(|(sequence_num, total_duration_ms)| RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: sequence_num as u32,
                started_at,
                finished_at: Some(started_at),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: Some(HttpResult {
                    negotiated_version: "HTTP/1.1".into(),
                    status_code: 200,
                    headers_size_bytes: 256,
                    body_size_bytes: 1024,
                    ttfb_ms: total_duration_ms / 2.0,
                    total_duration_ms,
                    redirect_count: 0,
                    started_at,
                    response_headers: vec![],
                    payload_bytes: 0,
                    throughput_mbps: None,
                    goodput_mbps: None,
                    cpu_time_ms: None,
                    csw_voluntary: None,
                    csw_involuntary: None,
                }),
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
                http_stack: None,
            })
            .collect();
        let run = TestRun {
            run_id,
            started_at,
            finished_at: Some(started_at),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 4,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: Some(NetworkBaseline {
                samples: 10,
                rtt_min_ms: 0.8,
                rtt_avg_ms: 1.0,
                rtt_max_ms: 1.2,
                rtt_p50_ms: 1.0,
                rtt_p95_ms: 1.1,
                network_type: crate::metrics::NetworkType::Loopback,
            }),
            packet_capture_summary: None,
            benchmark_environment_check: None,
            benchmark_stability_check: None,
            benchmark_phase: Some("measured".into()),
            benchmark_scenario: Some("warm".into()),
            benchmark_launch_index: Some(0),
            benchmark_warmup_attempt_count: 0,
            benchmark_pilot_attempt_count: 0,
            benchmark_overhead_attempt_count: 0,
            benchmark_cooldown_attempt_count: 0,
            benchmark_execution_plan: None,
            benchmark_noise_thresholds: None,
            attempts,
        };

        let artifact = to_benchmark_artifact(&run).unwrap();
        assert_eq!(
            artifact.metadata.contract_version,
            BENCHMARK_CONTRACT_VERSION
        );
        assert!(artifact.summary.variance > 0.0);
        assert!(artifact.summary.standard_error >= 0.0);
        assert!(artifact.summary.iqr > 0.0);
        assert!(artifact.summary.upper_fence < 25.0);
        assert_eq!(artifact.summary.high_outlier_count, 1);
        assert_eq!(artifact.summary.outlier_count, 1);
        assert!(artifact.summary.ci95_upper >= artifact.summary.ci95_lower);
        assert!(artifact.summary.relative_margin_of_error >= 0.0);
        assert_eq!(artifact.summary.quality_tier, "unreliable");
        assert_eq!(artifact.data_quality.outlier_count, 1);
        assert_eq!(
            artifact.data_quality.confidence_level,
            REPORT_CONFIDENCE_LEVEL
        );
        assert!(artifact
            .data_quality
            .outlier_policy
            .contains("Tukey 1.5xIQR"));
        assert!(!artifact.data_quality.publication_ready);
        assert!(!artifact.data_quality.publication_blockers.is_empty());
    }

    #[test]
    fn benchmark_contract_deserializes_old_quality_and_summary_shapes() {
        let old_json = r#"{
            "noise_level": "low",
            "sample_stability_cv": 0.02,
            "sufficiency": "adequate",
            "warnings": [],
            "publication_ready": true
        }"#;
        let quality: BenchmarkDataQuality = serde_json::from_str(old_json).unwrap();
        assert_eq!(quality.confidence_level, REPORT_CONFIDENCE_LEVEL);
        assert_eq!(quality.outlier_count, 0);
        assert_eq!(quality.quality_tier, "unknown");

        let old_summary_json = r#"{
            "case_id": "http1:default:default",
            "protocol": "http1",
            "metric_name": "Total ms",
            "metric_unit": "ms",
            "higher_is_better": false,
            "sample_count": 4,
            "included_sample_count": 4,
            "excluded_sample_count": 0,
            "success_count": 4,
            "failure_count": 0,
            "total_requests": 4,
            "error_count": 0,
            "bytes_transferred": 0,
            "wall_time_ms": 1.0,
            "rps": 4.0,
            "min": 10.0,
            "mean": 12.0,
            "p5": 10.0,
            "p25": 11.0,
            "p50": 12.0,
            "p75": 13.0,
            "p95": 14.0,
            "p99": 15.0,
            "p999": 16.0,
            "max": 16.0,
            "stddev": 1.0
        }"#;
        let summary: BenchmarkSummary = serde_json::from_str(old_summary_json).unwrap();
        assert_eq!(summary.variance, 0.0);
        assert_eq!(summary.outlier_count, 0);
        assert_eq!(summary.quality_tier, "unknown");
    }

    #[test]
    fn benchmark_contract_many_runs_serializes_as_array() {
        let runs = vec![benchmark_dummy_run(), benchmark_dummy_run()];
        let json = to_benchmark_string_many(&runs).unwrap();
        let de: Vec<BenchmarkArtifact> = serde_json::from_str(&json).unwrap();
        assert_eq!(de.len(), 2);
        assert_eq!(de[0].metadata.contract_version, BENCHMARK_CONTRACT_VERSION);
    }

    #[test]
    fn benchmark_contract_rejects_plain_runs_without_metadata() {
        let run = dummy_run();
        assert!(to_benchmark_string(&run).is_err());
        assert!(benchmark_artifact_if_present(&run).unwrap().is_none());
    }

    #[test]
    fn save_creates_file() {
        let tmp = NamedTempFile::new().unwrap();
        let run = dummy_run();
        save(&run, tmp.path()).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"target_url\""));
    }

    fn dummy_url_test_run() -> UrlTestRun {
        UrlTestRun {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            requested_url: "https://example.com".into(),
            final_url: Some("https://www.example.com".into()),
            status: UrlDiagnosticStatus::Completed,
            page_load_strategy: UrlPageLoadStrategy::Browser,
            browser_engine: Some("chromium".into()),
            browser_version: Some("123.0".into()),
            user_agent: Some("NetworkerTester/0.13".into()),
            primary_origin: Some("https://www.example.com".into()),
            observed_protocol_primary_load: Some("h3".into()),
            advertised_alt_svc: None,
            validated_http_versions: vec!["h3".into()],
            tls_version: None,
            cipher_suite: None,
            alpn: Some("h3".into()),
            dns_ms: Some(10.0),
            connect_ms: Some(20.0),
            handshake_ms: Some(25.0),
            ttfb_ms: Some(50.0),
            dom_content_loaded_ms: Some(150.0),
            load_event_ms: Some(300.0),
            network_idle_ms: None,
            capture_end_ms: Some(300.0),
            total_requests: 4,
            total_transfer_bytes: 4096,
            peak_concurrent_connections: None,
            redirect_count: 1,
            failure_count: 0,
            har_path: None,
            pcap_path: None,
            pcap_summary: None,
            capture_errors: vec![],
            environment_notes: None,
            origin_summaries: vec![UrlOriginSummary {
                origin: "https://www.example.com".into(),
                request_count: 4,
                failure_count: 0,
                total_transfer_bytes: 4096,
                protocols: vec!["h3".into()],
                dominant_protocol: Some("h3".into()),
                average_duration_ms: Some(12.5),
                cache_hit_count: Some(1),
            }],
            connection_summary: Some(UrlConnectionSummary {
                total_connection_ids: 1,
                reused_connection_count: 1,
                reused_resource_count: 3,
                resources_with_connection_id: 4,
                peak_origin_request_count: Some(4),
            }),
            resources: vec![],
            protocol_runs: vec![],
        }
    }

    #[test]
    fn url_test_json_round_trip() {
        let run = dummy_url_test_run();
        let json = to_string_url_test(&run).unwrap();
        let de: UrlTestRun = serde_json::from_str(&json).unwrap();
        assert_eq!(de.id, run.id);
        assert_eq!(de.requested_url, run.requested_url);
    }

    #[test]
    fn save_url_test_creates_file() {
        let tmp = NamedTempFile::new().unwrap();
        let run = dummy_url_test_run();
        save_url_test(&run, tmp.path()).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"requested_url\""));
    }

    #[test]
    fn trust_section_deserializes_without_new_fields() {
        let old_json = r#"{
            "hostname_matches": true,
            "chain_valid": true,
            "trusted_by_system_store": true,
            "issues": [],
            "revocation": {
                "ocsp_stapled": false,
                "method": "best_effort",
                "status": "unknown",
                "notes": []
            }
        }"#;
        let section: TlsTrustSection = serde_json::from_str(old_json).unwrap();
        assert!(!section.verification_performed);
        assert!(!section.chain_presented);
        assert_eq!(section.chain_diagnostics.presented_chain_length, 0);
        assert!(section.revocation.ocsp_urls.is_empty());
        assert!(!section.revocation.online_check_attempted);
    }

    #[test]
    fn tls_profile_json_round_trip() {
        let run = TlsEndpointProfile {
            target_kind: TlsProfileTargetKind::ExternalUrl,
            coverage_level: TlsProfileCoverageLevel::ClientObserved,
            unsupported_checks: vec!["protocol_matrix".into()],
            limitations: vec!["client-visible only".into()],
            target: TlsProfileTarget {
                host: "example.com".into(),
                port: 443,
                requested_ip: None,
                sni: Some("example.com".into()),
                resolved_ips: vec!["93.184.216.34".into()],
                source_url: Some("https://example.com".into()),
            },
            path_characteristics: TlsPathCharacteristics {
                connected_ip: Some("93.184.216.34".into()),
                direct_ip_match: true,
                proxy_detected: false,
                classification: TlsPathClassification::Direct,
                evidence: vec![],
            },
            connectivity: TlsProfileConnectivity {
                tcp_connect_ms: Some(10.0),
                tls_handshake_ms: Some(20.0),
                negotiated_tls_version: Some("TLSv1_3".into()),
                negotiated_cipher_suite: Some("TLS_AES_128_GCM_SHA256".into()),
                negotiated_key_exchange_group: None,
                alpn: Some("h2".into()),
            },
            certificate: TlsCertificateSection {
                leaf: None,
                chain: vec![],
            },
            trust: TlsTrustSection {
                hostname_matches: true,
                chain_valid: true,
                trusted_by_system_store: true,
                verification_performed: true,
                chain_presented: false,
                verified_chain_depth: None,
                issues: vec![],
                chain_diagnostics: TlsChainDiagnostics {
                    presented_chain_length: 0,
                    leaf_self_signed: false,
                    has_intermediate: false,
                    ordered_subject_issuer_links: true,
                    root_included: false,
                    notes: vec![],
                },
                revocation: TlsRevocationInfo {
                    ocsp_stapled: false,
                    method: "best_effort".into(),
                    status: "unknown".into(),
                    ocsp_urls: vec![],
                    crl_urls: vec![],
                    online_check_attempted: false,
                    notes: vec![],
                },
                caa: None,
            },
            capabilities: Some(TlsCapabilitiesSection {
                protocol_support: vec![TlsProtocolSupport {
                    protocol: "tls1.3".into(),
                    supported: true,
                    accepted_ciphers: vec!["TLS_AES_128_GCM_SHA256".into()],
                    supported_groups: vec!["X25519".into()],
                }],
                alpn_support: vec!["h2".into()],
                sni_behavior: TlsSniBehavior {
                    with_sni_ok: true,
                    without_sni_ok: Some(false),
                    default_cert_subject: None,
                },
                client_auth: TlsClientAuthStatus {
                    requested: false,
                    required: false,
                },
            }),
            resumption: TlsResumptionSection {
                supported: true,
                method: None,
                initial_handshake_ms: Some(20.0),
                resumed_handshake_ms: Some(18.0),
                resumption_ratio: Some(20.0 / 18.0),
                resumed_tls_version: Some("TLSv1_3".into()),
                resumed_cipher_suite: Some("TLS_AES_128_GCM_SHA256".into()),
                early_data_offered: false,
                early_data_accepted: None,
                notes: vec!["0-RTT availability is not actively negotiated in this phase; values remain advisory".into()],
            },
            findings: vec![],
            summary: TlsProfileSummary {
                status: "ok".into(),
                score: None,
            },
        };
        let json = to_string_tls_profile(&run).unwrap();
        let de: TlsEndpointProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(de.target.host, "example.com");
        assert!(de.summary.score.is_none());
    }
}
