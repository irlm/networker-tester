use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Top-level record for a single benchmark execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRun {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub config_path: String,
    pub results: Vec<BenchmarkResult>,
}

/// Result of benchmarking one (language, concurrency) combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub language: String,
    pub runtime: String,
    pub concurrency: u32,
    pub repeat_index: u32,
    pub network: NetworkMetrics,
    pub resources: ResourceMetrics,
    pub startup: StartupMetrics,
    pub binary: BinaryMetrics,
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
            results: Vec::new(),
        }
    }

    pub fn finish(&mut self) {
        self.finished_at = Some(Utc::now());
    }
}
