use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn default_report_confidence_level() -> f64 {
    0.95
}

fn default_report_outlier_policy() -> String {
    "Tukey 1.5xIQR fences flag outliers for audit while repeated-launch summaries keep all launches and avoid cherry-picking.".to_string()
}

fn default_report_uncertainty_method() -> String {
    "Percentile bootstrap confidence intervals around the median with deterministic resampling."
        .to_string()
}

fn default_publication_ready() -> bool {
    true
}

fn default_quality_tier() -> String {
    "unknown".to_string()
}

fn default_result_scenario() -> String {
    "unknown".to_string()
}

fn default_comparable() -> bool {
    true
}

/// Policy for bounded orchestrator-managed reruns when publication quality is weak.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkAutoRerunPolicy {
    pub target_repeat_count: u32,
    pub max_additional_repeats: u32,
    pub max_relative_margin_of_error: f64,
}

/// One orchestrator-scheduled rerun cycle for a benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkScheduledRerun {
    pub language: String,
    pub runtime: String,
    pub concurrency: u32,
    pub repeat_index: u32,
    pub reasons: Vec<String>,
}

/// Top-level record for a single benchmark execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRun {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub config_path: String,
    #[serde(default)]
    pub case_randomization_enabled: bool,
    #[serde(default)]
    pub case_randomization_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_rerun_policy: Option<BenchmarkAutoRerunPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled_reruns: Vec<BenchmarkScheduledRerun>,
    #[serde(default)]
    pub baseline: Option<BenchmarkBaseline>,
    pub results: Vec<BenchmarkResult>,
}

/// Result of benchmarking one (language, concurrency) combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub language: String,
    pub runtime: String,
    pub concurrency: u32,
    pub repeat_index: u32,
    #[serde(default = "default_result_scenario")]
    pub scenario: String,
    #[serde(
        default,
        skip_serializing_if = "BenchmarkEnvironmentFingerprint::is_empty"
    )]
    pub environment: BenchmarkEnvironmentFingerprint,
    pub network: NetworkMetrics,
    pub resources: ResourceMetrics,
    pub startup: StartupMetrics,
    pub binary: BinaryMetrics,
}

/// Minimal environment fingerprint captured from the tester artifact so
/// baseline comparisons can reject materially different environments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BenchmarkEnvironmentFingerprint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cpu_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_cpu_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_rtt_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_rtt_p95_ms: Option<f64>,
}

impl BenchmarkEnvironmentFingerprint {
    pub fn is_empty(&self) -> bool {
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

/// Versioned JSON report written by the benchmark reporter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub format_version: String,
    pub generated_at: DateTime<Utc>,
    pub run: BenchmarkRun,
    pub aggregation: ReportAggregation,
}

/// Derived aggregation metadata for a benchmark report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportAggregation {
    pub grouped_by: Vec<String>,
    pub ranking_metric: String,
    pub primary_estimator: String,
    #[serde(default = "default_report_confidence_level")]
    pub confidence_level: f64,
    #[serde(default = "default_report_outlier_policy")]
    pub outlier_policy: String,
    #[serde(default = "default_report_uncertainty_method")]
    pub uncertainty_method: String,
    pub anti_cherry_picking_policy: String,
    #[serde(default = "default_publication_ready")]
    pub publication_ready: bool,
    #[serde(default)]
    pub rerun_recommended: bool,
    #[serde(default)]
    pub recommendations: Vec<String>,
    pub case_summaries: Vec<BenchmarkCaseSummary>,
    pub comparisons: Vec<BenchmarkComparison>,
}

/// Explicit baseline selection for benchmark comparisons.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkBaseline {
    pub language: String,
    pub runtime: Option<String>,
}

/// Repeat-aware summary for a single benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkCaseSummary {
    pub case_label: String,
    pub language: String,
    pub runtime: String,
    pub concurrency: u32,
    pub warm: Option<ScenarioSummary>,
    pub cold: Option<ScenarioSummary>,
}

/// Aggregated statistics for one scenario of a benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioSummary {
    pub scenario: String,
    pub repeat_count: u32,
    pub repeat_indices: Vec<u32>,
    pub rps: MetricSummary,
    pub latency_mean_ms: MetricSummary,
    pub latency_p50_ms: MetricSummary,
    pub latency_p99_ms: MetricSummary,
    pub latency_p999_ms: MetricSummary,
    pub latency_max_ms: MetricSummary,
    pub error_rate_pct: MetricSummary,
    pub avg_cpu_fraction: MetricSummary,
    pub peak_rss_bytes: MetricSummary,
    pub time_to_first_response_ms: MetricSummary,
    pub binary_size_bytes: MetricSummary,
}

/// Basic repeat-aware descriptive statistics for one numeric metric.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricSummary {
    pub sample_count: u32,
    pub min: f64,
    pub mean: f64,
    pub median: f64,
    pub max: f64,
    pub stddev: f64,
    pub cv: f64,
    pub standard_error: f64,
    #[serde(default)]
    pub variance: f64,
    #[serde(default)]
    pub iqr: f64,
    #[serde(default)]
    pub lower_fence: f64,
    #[serde(default)]
    pub upper_fence: f64,
    #[serde(default)]
    pub low_outlier_count: u32,
    #[serde(default)]
    pub high_outlier_count: u32,
    #[serde(default)]
    pub outlier_count: u32,
    #[serde(default)]
    pub skewness: f64,
    #[serde(default)]
    pub kurtosis: f64,
    #[serde(default)]
    pub mad: f64,
    pub ci95_lower: f64,
    pub ci95_upper: f64,
    #[serde(default)]
    pub relative_margin_of_error: f64,
    #[serde(default = "default_quality_tier")]
    pub quality_tier: String,
}

/// Comparison of a candidate case against the configured baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkComparison {
    pub case_label: String,
    pub language: String,
    pub runtime: String,
    pub concurrency: u32,
    pub baseline_case_label: String,
    #[serde(default = "default_comparable")]
    pub comparable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparability_notes: Vec<String>,
    pub warm: Option<ScenarioComparison>,
    pub cold: Option<ScenarioComparison>,
}

/// Scenario-level comparison metrics against the configured baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioComparison {
    pub scenario: String,
    pub shared_repeat_count: u32,
    pub throughput: ComparisonMetricSummary,
    pub latency_p99: ComparisonMetricSummary,
}

/// Metric comparison including deltas, ratios, and practical-significance verdicts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonMetricSummary {
    pub higher_is_better: bool,
    pub candidate_summary: MetricSummary,
    pub baseline_summary: MetricSummary,
    pub absolute_delta: f64,
    pub percent_delta: f64,
    pub ratio: f64,
    pub ratio_summary: MetricSummary,
    pub effect_size: f64,
    pub verdict: String,
}

/// Per-request network timing aggregates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkMetrics {
    /// Requests per second achieved.
    pub rps: f64,
    /// Mean latency in milliseconds.
    pub latency_mean_ms: f64,
    /// p50 latency in milliseconds.
    pub latency_p50_ms: f64,
    /// p99 latency in milliseconds.
    pub latency_p99_ms: f64,
    /// p99.9 latency in milliseconds.
    pub latency_p999_ms: f64,
    /// Maximum observed latency in milliseconds.
    pub latency_max_ms: f64,
    /// Total bytes transferred.
    pub bytes_transferred: u64,
    /// Error count during the benchmark.
    pub error_count: u64,
    /// Total number of requests issued.
    pub total_requests: u64,
    /// Phase model reported by the benchmark artifact when available.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase_model: String,
    /// Ordered phase list reported by the benchmark artifact when available.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases_present: Vec<String>,
}

/// Resource consumption during the benchmark.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceMetrics {
    /// Peak RSS in bytes.
    pub peak_rss_bytes: u64,
    /// Average CPU usage as a fraction (0.0–1.0+).
    pub avg_cpu_fraction: f64,
    /// Peak CPU usage as a fraction.
    pub peak_cpu_fraction: f64,
    /// Number of open file descriptors at peak.
    pub peak_open_fds: u64,
}

/// Cold-start / warm-start timing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartupMetrics {
    /// Time from process start to first successful health-check response, in ms.
    pub time_to_first_response_ms: f64,
    /// Time from process start to stable request serving, in ms.
    pub time_to_ready_ms: f64,
}

/// Static binary / artifact characteristics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BinaryMetrics {
    /// Uncompressed binary or artifact size in bytes.
    pub size_bytes: u64,
    /// Compressed (gzip) size in bytes.
    pub compressed_size_bytes: u64,
    /// Docker image size in bytes (if applicable).
    pub docker_image_bytes: Option<u64>,
}

impl BenchmarkRun {
    pub fn new(config_path: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            finished_at: None,
            config_path: config_path.to_string(),
            case_randomization_enabled: false,
            case_randomization_seed: None,
            auto_rerun_policy: None,
            scheduled_reruns: Vec::new(),
            baseline: None,
            results: Vec::new(),
        }
    }

    pub fn finish(&mut self) {
        self.finished_at = Some(Utc::now());
    }
}
