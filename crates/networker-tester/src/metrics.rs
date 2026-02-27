/// Core data model – one row per concept, normalised.
///
/// TestRun  → 1:N  RequestAttempt
/// RequestAttempt → 0:1 DnsResult, TcpResult, TlsResult, HttpResult, UdpResult, ErrorRecord
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Top-level run
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub target_url: String,
    pub target_host: String,
    pub modes: Vec<String>,
    pub total_runs: u32,
    pub concurrency: u32,
    pub timeout_ms: u64,
    pub client_os: String,
    pub client_version: String,
    pub attempts: Vec<RequestAttempt>,
}

impl TestRun {
    pub fn success_count(&self) -> usize {
        self.attempts.iter().filter(|a| a.success).count()
    }

    pub fn failure_count(&self) -> usize {
        self.attempts.iter().filter(|a| !a.success).count()
    }

    pub fn protocols_tested(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.attempts
            .iter()
            .filter_map(|a| {
                if seen.insert(a.protocol.to_string()) {
                    Some(a.protocol.to_string())
                } else {
                    None
                }
            })
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-attempt record
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestAttempt {
    pub attempt_id: Uuid,
    pub run_id: Uuid,
    pub protocol: Protocol,
    pub sequence_num: u32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub success: bool,
    pub dns: Option<DnsResult>,
    pub tcp: Option<TcpResult>,
    pub tls: Option<TlsResult>,
    pub http: Option<HttpResult>,
    pub udp: Option<UdpResult>,
    pub error: Option<ErrorRecord>,
}

impl RequestAttempt {
    pub fn total_duration_ms(&self) -> Option<f64> {
        let start = self.started_at;
        let end = self.finished_at?;
        Some((end - start).num_microseconds()? as f64 / 1000.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Protocol enum
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Http1,
    Http2,
    Http3,
    Udp,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Tcp   => write!(f, "tcp"),
            Protocol::Http1 => write!(f, "http1"),
            Protocol::Http2 => write!(f, "http2"),
            Protocol::Http3 => write!(f, "http3"),
            Protocol::Udp   => write!(f, "udp"),
        }
    }
}

impl std::str::FromStr for Protocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tcp"   => Ok(Protocol::Tcp),
            "http1" => Ok(Protocol::Http1),
            "http2" => Ok(Protocol::Http2),
            "http3" => Ok(Protocol::Http3),
            "udp"   => Ok(Protocol::Udp),
            other   => Err(format!("Unknown protocol: {other}")),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-result types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsResult {
    pub query_name: String,
    pub resolved_ips: Vec<String>,
    pub duration_ms: f64,
    pub started_at: DateTime<Utc>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpResult {
    pub local_addr: Option<String>,
    pub remote_addr: String,
    pub connect_duration_ms: f64,
    pub attempt_count: u32,
    pub started_at: DateTime<Utc>,
    pub success: bool,
    /// MSS as reported by TCP_MAXSEG setsockopt (best-effort, Unix only).
    pub mss_bytes: Option<u32>,
    /// Smoothed RTT in ms from TCP_INFO (Linux only).
    pub rtt_estimate_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsResult {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub alpn_negotiated: Option<String>,
    pub cert_subject: Option<String>,
    pub cert_issuer: Option<String>,
    pub cert_expiry: Option<DateTime<Utc>>,
    pub handshake_duration_ms: f64,
    pub started_at: DateTime<Utc>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResult {
    pub negotiated_version: String,
    pub status_code: u16,
    pub headers_size_bytes: usize,
    pub body_size_bytes: usize,
    pub ttfb_ms: f64,
    pub total_duration_ms: f64,
    pub redirect_count: u32,
    pub started_at: DateTime<Utc>,
    pub response_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpResult {
    pub remote_addr: String,
    pub probe_count: u32,
    pub success_count: u32,
    pub loss_percent: f64,
    pub rtt_min_ms: f64,
    pub rtt_avg_ms: f64,
    pub rtt_p95_ms: f64,
    pub jitter_ms: f64,
    pub started_at: DateTime<Utc>,
    /// Per-probe RTT values (ms), None if probe was lost.
    pub probe_rtts_ms: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub category: ErrorCategory,
    pub message: String,
    pub detail: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ErrorCategory {
    Dns,
    Tcp,
    Tls,
    Http,
    Udp,
    Timeout,
    Config,
    Other,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ErrorCategory::Dns     => "dns",
            ErrorCategory::Tcp     => "tcp",
            ErrorCategory::Tls     => "tls",
            ErrorCategory::Http    => "http",
            ErrorCategory::Udp     => "udp",
            ErrorCategory::Timeout => "timeout",
            ErrorCategory::Config  => "config",
            ErrorCategory::Other   => "other",
        };
        write!(f, "{s}")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UDP RTT aggregation helper
// ─────────────────────────────────────────────────────────────────────────────

pub struct RttStats {
    pub min: f64,
    pub avg: f64,
    pub p95: f64,
    pub jitter: f64,
    pub loss_percent: f64,
}

/// Compute aggregate stats from a slice of Option<f64> RTT samples.
/// None values count as lost probes.
pub fn aggregate_udp_rtts(samples: &[Option<f64>]) -> RttStats {
    let total = samples.len() as f64;
    let mut rtts: Vec<f64> = samples.iter().filter_map(|v| *v).collect();
    let received = rtts.len() as f64;
    let loss = if total > 0.0 { (total - received) / total * 100.0 } else { 100.0 };

    if rtts.is_empty() {
        return RttStats { min: 0.0, avg: 0.0, p95: 0.0, jitter: 0.0, loss_percent: loss };
    }

    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = rtts[0];
    let avg = rtts.iter().sum::<f64>() / received;
    let p95_idx = ((rtts.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    let p95 = rtts[p95_idx.min(rtts.len() - 1)];

    // Jitter: mean of successive absolute differences
    let jitter = if rtts.len() > 1 {
        let diffs: Vec<f64> = rtts.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        diffs.iter().sum::<f64>() / diffs.len() as f64
    } else {
        0.0
    };

    RttStats { min, avg, p95, jitter, loss_percent: loss }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtt_stats_no_loss() {
        let samples: Vec<Option<f64>> = vec![
            Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(5.0),
            Some(6.0), Some(7.0), Some(8.0), Some(9.0), Some(10.0),
        ];
        let s = aggregate_udp_rtts(&samples);
        assert_eq!(s.loss_percent, 0.0);
        assert!((s.min - 1.0).abs() < 1e-9);
        assert!((s.avg - 5.5).abs() < 1e-9);
        // 95th percentile of 10 values → index ceil(9.5)-1 = 9 → 10.0
        assert!((s.p95 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_rtt_stats_with_loss() {
        let samples: Vec<Option<f64>> =
            vec![Some(5.0), None, Some(10.0), None, Some(15.0)];
        let s = aggregate_udp_rtts(&samples);
        assert!((s.loss_percent - 40.0).abs() < 1e-9);
        assert!((s.min - 5.0).abs() < 1e-9);
        assert!((s.avg - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_rtt_stats_all_lost() {
        let samples: Vec<Option<f64>> = vec![None, None, None];
        let s = aggregate_udp_rtts(&samples);
        assert_eq!(s.loss_percent, 100.0);
    }

    #[test]
    fn test_protocol_roundtrip() {
        use std::str::FromStr;
        for p in &["tcp", "http1", "http2", "http3", "udp"] {
            let parsed = Protocol::from_str(p).unwrap();
            assert_eq!(parsed.to_string(), *p);
        }
    }

    #[test]
    fn test_test_run_counts() {
        let run_id = Uuid::new_v4();
        let mk = |success: bool| RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Http1,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: None,
            success,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
        };
        let run = TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: None,
            target_url: "http://x".into(),
            target_host: "x".into(),
            modes: vec![],
            total_runs: 3,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            attempts: vec![mk(true), mk(false), mk(true)],
        };
        assert_eq!(run.success_count(), 2);
        assert_eq!(run.failure_count(), 1);
    }

    #[test]
    fn test_json_serialisation() {
        let r = DnsResult {
            query_name: "example.com".into(),
            resolved_ips: vec!["93.184.216.34".into()],
            duration_ms: 12.5,
            started_at: Utc::now(),
            success: true,
        };
        let json = serde_json::to_string(&r).unwrap();
        let de: DnsResult = serde_json::from_str(&json).unwrap();
        assert_eq!(de.query_name, r.query_name);
        assert!((de.duration_ms - r.duration_ms).abs() < 1e-9);
    }
}
