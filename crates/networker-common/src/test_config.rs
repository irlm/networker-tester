//! Canonical `TestConfig` primitive and surrounding types.
//!
//! Unifies the former `Job` (simple tests) and `BenchmarkConfig` (benchmark-grade
//! tests) under a single definition. See `.critique/refactor/03-spec.md` for the
//! authoritative shape; this module is the Rust mirror of spec §2.
//!
//! Polymorphic fields (`endpoint`, `workload`, `methodology`) are strongly typed
//! here but serialize to JSONB at the DB boundary — see
//! `crates/networker-dashboard/src/db/test_configs.rs`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Top-level TestConfig
// ─────────────────────────────────────────────────────────────────────────────

/// The canonical unit of work: the definition of a test.
///
/// `methodology` is `None` for a simple test and `Some(...)` for a benchmark-grade
/// test. The only difference at storage time is whether a `BenchmarkArtifact` is
/// produced alongside the `TestRun`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    pub id: Uuid,
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub endpoint: EndpointRef,
    pub workload: Workload,
    #[serde(default)]
    pub methodology: Option<Methodology>,
    #[serde(default)]
    pub baseline_run_id: Option<Uuid>,
    pub max_duration_secs: u32,
    #[serde(default)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TestConfig {
    /// True when this config carries a `Methodology` block — i.e. it runs in
    /// benchmark mode and will produce a `BenchmarkArtifact` on completion.
    pub fn is_benchmark(&self) -> bool {
        self.methodology.is_some()
    }

    /// String form of the endpoint kind — matches the `endpoint_kind` column.
    pub fn endpoint_kind(&self) -> &'static str {
        match &self.endpoint {
            EndpointRef::Network { .. } => "network",
            EndpointRef::Proxy { .. } => "proxy",
            EndpointRef::Runtime { .. } => "runtime",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EndpointRef — tagged union, JSONB-backed
// ─────────────────────────────────────────────────────────────────────────────

/// The target of a test. One of three kinds.
///
/// Serializes with an internal `kind` tag so the JSONB document stored in
/// `test_config.endpoint_ref` mirrors the `endpoint_kind` column.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EndpointRef {
    /// Direct protocol probe against a host/port.
    Network {
        host: String,
        #[serde(default)]
        port: Option<u16>,
    },
    /// Endpoint fronted by a reverse proxy (nginx, IIS, etc.).
    Proxy { proxy_endpoint_id: Uuid },
    /// Language/framework stack (Node, Go, Rust, etc.) from the runtime catalog.
    Runtime { runtime_id: Uuid, language: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Workload (required)
// ─────────────────────────────────────────────────────────────────────────────

/// What protocols, how many runs, and at what concurrency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workload {
    pub modes: Vec<Mode>,
    pub runs: u32,
    pub concurrency: u32,
    pub timeout_ms: u32,
    #[serde(default)]
    pub payload_sizes: Vec<u32>,
    pub capture_mode: CaptureMode,
}

/// Probe mode / protocol variant. Union of current `networker_tester::Protocol`
/// variants. Serializes as lowercase string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Http1,
    Http2,
    Http3,
    Tcp,
    Tls,
    Dns,
    Udp,
    Pageload,
    Pageload2,
    Pageload3,
    Browser1,
    Browser2,
    Browser3,
    Download,
    Upload,
    Curl,
    Native,
    Tlsresume,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Http1 => "http1",
            Mode::Http2 => "http2",
            Mode::Http3 => "http3",
            Mode::Tcp => "tcp",
            Mode::Tls => "tls",
            Mode::Dns => "dns",
            Mode::Udp => "udp",
            Mode::Pageload => "pageload",
            Mode::Pageload2 => "pageload2",
            Mode::Pageload3 => "pageload3",
            Mode::Browser1 => "browser1",
            Mode::Browser2 => "browser2",
            Mode::Browser3 => "browser3",
            Mode::Download => "download",
            Mode::Upload => "upload",
            Mode::Curl => "curl",
            Mode::Native => "native",
            Mode::Tlsresume => "tlsresume",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    HeadersOnly,
    Full,
    MetricsOnly,
}

// ─────────────────────────────────────────────────────────────────────────────
// Methodology (optional — benchmark mode)
// ─────────────────────────────────────────────────────────────────────────────

/// Statistical methodology. Present means "benchmark mode"; absent means
/// "simple test".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Methodology {
    pub warmup_runs: u32,
    pub measured_runs: u32,
    pub cooldown_ms: u32,
    /// Target CV% — the measurement tries to converge below this.
    pub target_error_pct: f32,
    pub outlier_policy: OutlierPolicy,
    pub quality_gates: QualityGates,
    pub publication_gates: PublicationGates,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "kebab-case")]
pub enum OutlierPolicy {
    None,
    Iqr { k: f32 },
    StdDev { sigma: f32 },
    Percentile { lo: f32, hi: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityGates {
    pub max_cv_pct: f32,
    pub min_samples: u32,
    pub max_noise_level: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicationGates {
    pub max_failure_pct: f32,
    pub require_all_phases: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// TestRun
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
        }
    }

    /// Parse from the lowercase wire form. Returns `None` on unknown input.
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(RunStatus::Queued),
            "running" => Some(RunStatus::Running),
            "completed" => Some(RunStatus::Completed),
            "failed" => Some(RunStatus::Failed),
            "cancelled" => Some(RunStatus::Cancelled),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
        )
    }
}

impl std::str::FromStr for RunStatus {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_str(s).ok_or("unknown run status")
    }
}

/// Every execution of a `TestConfig` produces one `TestRun`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    pub id: Uuid,
    pub test_config_id: Uuid,
    pub project_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    pub success_count: u32,
    pub failure_count: u32,
    #[serde(default)]
    pub error_message: Option<String>,
    /// `Some` iff the config carried a `Methodology` and the artifact row exists.
    #[serde(default)]
    pub artifact_id: Option<Uuid>,
    #[serde(default)]
    pub tester_id: Option<Uuid>,
    #[serde(default)]
    pub worker_id: Option<String>,
    #[serde(default)]
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ─────────────────────────────────────────────────────────────────────────────
// TestSchedule
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSchedule {
    pub id: Uuid,
    pub test_config_id: Uuid,
    pub project_id: String,
    pub cron_expr: String,
    pub timezone: String,
    pub enabled: bool,
    #[serde(default)]
    pub last_fired_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run_id: Option<Uuid>,
    #[serde(default)]
    pub next_fire_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> TestConfig {
        TestConfig {
            id: Uuid::nil(),
            project_id: "proj123456789a".to_string(),
            name: "cloudflare".to_string(),
            description: None,
            endpoint: EndpointRef::Network {
                host: "www.cloudflare.com".to_string(),
                port: None,
            },
            workload: Workload {
                modes: vec![Mode::Dns, Mode::Tcp, Mode::Tls, Mode::Http2],
                runs: 10,
                concurrency: 1,
                timeout_ms: 5000,
                payload_sizes: vec![],
                capture_mode: CaptureMode::HeadersOnly,
            },
            methodology: None,
            baseline_run_id: None,
            max_duration_secs: 900,
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn simple_test_is_not_a_benchmark() {
        let cfg = sample_config();
        assert!(!cfg.is_benchmark());
        assert_eq!(cfg.endpoint_kind(), "network");
    }

    #[test]
    fn benchmark_mode_flagged_by_methodology() {
        let mut cfg = sample_config();
        cfg.methodology = Some(Methodology {
            warmup_runs: 5,
            measured_runs: 30,
            cooldown_ms: 100,
            target_error_pct: 2.0,
            outlier_policy: OutlierPolicy::Iqr { k: 1.5 },
            quality_gates: QualityGates {
                max_cv_pct: 5.0,
                min_samples: 20,
                max_noise_level: 0.3,
            },
            publication_gates: PublicationGates {
                max_failure_pct: 1.0,
                require_all_phases: true,
            },
        });
        assert!(cfg.is_benchmark());
    }

    #[test]
    fn endpoint_ref_round_trips_as_tagged_json() {
        let network = EndpointRef::Network {
            host: "example.com".into(),
            port: Some(443),
        };
        let json = serde_json::to_string(&network).unwrap();
        assert!(json.contains("\"kind\":\"network\""));
        let back: EndpointRef = serde_json::from_str(&json).unwrap();
        match back {
            EndpointRef::Network { host, port } => {
                assert_eq!(host, "example.com");
                assert_eq!(port, Some(443));
            }
            _ => panic!("wrong variant"),
        }

        let proxy = EndpointRef::Proxy {
            proxy_endpoint_id: Uuid::nil(),
        };
        let json = serde_json::to_string(&proxy).unwrap();
        assert!(json.contains("\"kind\":\"proxy\""));

        let runtime = EndpointRef::Runtime {
            runtime_id: Uuid::nil(),
            language: "rust".into(),
        };
        let json = serde_json::to_string(&runtime).unwrap();
        assert!(json.contains("\"kind\":\"runtime\""));
    }

    #[test]
    fn run_status_lowercase_string() {
        assert_eq!(
            serde_json::to_string(&RunStatus::Queued).unwrap(),
            "\"queued\""
        );
        assert_eq!(RunStatus::parse_str("running"), Some(RunStatus::Running));
        assert!(RunStatus::Completed.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
    }

    #[test]
    fn capture_mode_kebab_case() {
        assert_eq!(
            serde_json::to_string(&CaptureMode::HeadersOnly).unwrap(),
            "\"headers-only\""
        );
        assert_eq!(
            serde_json::to_string(&CaptureMode::MetricsOnly).unwrap(),
            "\"metrics-only\""
        );
    }

    #[test]
    fn outlier_policy_internally_tagged() {
        let p = OutlierPolicy::Iqr { k: 1.5 };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"policy\":\"iqr\""));
        let back: OutlierPolicy = serde_json::from_str(&json).unwrap();
        match back {
            OutlierPolicy::Iqr { k } => assert!((k - 1.5).abs() < f32::EPSILON),
            _ => panic!("wrong variant"),
        }
    }
}
