/// Core data model – one row per concept, normalised.
///
/// TestRun  → 1:N  RequestAttempt
/// RequestAttempt → 0:1 DnsResult, TcpResult, TlsResult, HttpResult, UdpResult, ErrorRecord
///              → 0:1 ServerTimingResult (when X-Networker-* headers present)
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
    /// Number of retries performed before this attempt succeeded (0 = first try succeeded).
    #[serde(default)]
    pub retry_count: u32,
    /// Server-side timing metadata parsed from response headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_timing: Option<ServerTimingResult>,
    /// UDP bulk transfer result (udpdownload / udpupload modes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_throughput: Option<UdpThroughputResult>,
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
    Download,
    Upload,
    /// GET the target URL as-is; measures HTTP timing + response body throughput.
    WebDownload,
    /// POST to the target URL with a payload; measures HTTP timing + upload throughput.
    WebUpload,
    /// UDP bulk download from the networker-endpoint UDP throughput server (port 9998).
    UdpDownload,
    /// UDP bulk upload to the networker-endpoint UDP throughput server (port 9998).
    UdpUpload,
    /// Standalone DNS resolution probe — resolves the target host and records timing without TCP.
    Dns,
    /// Standalone TLS probe — DNS + TCP + TLS handshake only, no HTTP request.
    /// Collects the full certificate chain, cipher suite, and negotiated ALPN.
    Tls,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Tcp => write!(f, "tcp"),
            Protocol::Http1 => write!(f, "http1"),
            Protocol::Http2 => write!(f, "http2"),
            Protocol::Http3 => write!(f, "http3"),
            Protocol::Udp => write!(f, "udp"),
            Protocol::Download => write!(f, "download"),
            Protocol::Upload => write!(f, "upload"),
            Protocol::WebDownload => write!(f, "webdownload"),
            Protocol::WebUpload => write!(f, "webupload"),
            Protocol::UdpDownload => write!(f, "udpdownload"),
            Protocol::UdpUpload => write!(f, "udpupload"),
            Protocol::Dns => write!(f, "dns"),
            Protocol::Tls => write!(f, "tls"),
        }
    }
}

impl std::str::FromStr for Protocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tcp" => Ok(Protocol::Tcp),
            "http1" => Ok(Protocol::Http1),
            "http2" => Ok(Protocol::Http2),
            "http3" => Ok(Protocol::Http3),
            "udp" => Ok(Protocol::Udp),
            "download" => Ok(Protocol::Download),
            "upload" => Ok(Protocol::Upload),
            "webdownload" => Ok(Protocol::WebDownload),
            "webupload" => Ok(Protocol::WebUpload),
            "udpdownload" => Ok(Protocol::UdpDownload),
            "udpupload" => Ok(Protocol::UdpUpload),
            "dns" => Ok(Protocol::Dns),
            "tls" => Ok(Protocol::Tls),
            other => Err(format!("Unknown protocol: {other}")),
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
    /// Smoothed RTT in ms from TCP_INFO (Linux) or TCP_CONNECTION_INFO (macOS).
    pub rtt_estimate_ms: Option<f64>,
    // ── Extended kernel stats (TCP_INFO / TCP_CONNECTION_INFO) ─────────────────
    /// Segments currently queued for retransmit (tcpi_retransmits).
    #[serde(default)]
    pub retransmits: Option<u32>,
    /// Lifetime retransmission count (tcpi_total_retrans).
    #[serde(default)]
    pub total_retrans: Option<u32>,
    /// Congestion window in segments (tcpi_snd_cwnd).
    #[serde(default)]
    pub snd_cwnd: Option<u32>,
    /// Slow-start threshold; None when set to the kernel sentinel (infinite).
    #[serde(default)]
    pub snd_ssthresh: Option<u32>,
    /// RTT variance in ms (tcpi_rttvar).
    #[serde(default)]
    pub rtt_variance_ms: Option<f64>,
    /// Receiver advertised window in bytes (tcpi_rcv_space).
    #[serde(default)]
    pub rcv_space: Option<u32>,
    /// Segments sent since connection start (Linux ≥ 4.2).
    #[serde(default)]
    pub segs_out: Option<u32>,
    /// Segments received since connection start (Linux ≥ 4.2).
    #[serde(default)]
    pub segs_in: Option<u32>,
    /// Congestion control algorithm name, e.g. "cubic", "bbr" (TCP_CONGESTION).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub congestion_algorithm: Option<String>,
    /// Estimated TCP delivery rate in bytes/sec (Linux ≥ 4.9: tcpi_delivery_rate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_rate_bps: Option<u64>,
    /// Minimum RTT ever observed by the kernel in ms (Linux ≥ 4.9: tcpi_min_rtt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_rtt_ms: Option<f64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Server-side timing
// ─────────────────────────────────────────────────────────────────────────────

/// Metadata extracted from X-Networker-* and Server-Timing response headers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerTimingResult {
    /// Echoed X-Networker-Request-Id from the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Server wall-clock time from X-Networker-Server-Timestamp header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_timestamp: Option<DateTime<Utc>>,
    /// Rough one-way clock skew estimate: (server_ts − client_send_at) − ttfb_ms/2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_skew_ms: Option<f64>,
    /// Body drain time on server side (Server-Timing: recv;dur=X, upload only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recv_body_ms: Option<f64>,
    /// Server processing time (Server-Timing: proc;dur=X, download only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing_ms: Option<f64>,
    /// Total server time (Server-Timing: total;dur=X).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_server_ms: Option<f64>,
    /// Server binary version from X-Networker-Server-Version header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
}

/// A single certificate in the peer's certificate chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertEntry {
    pub subject: String,
    pub issuer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<DateTime<Utc>>,
    /// Subject Alternative Names (DNS names and IP addresses).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sans: Vec<String>,
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
    /// Full certificate chain returned by the server (leaf cert first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cert_chain: Vec<CertEntry>,
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
    /// Bytes requested (download) or sent (upload); 0 for normal probes.
    #[serde(default)]
    pub payload_bytes: usize,
    /// Measured throughput in MB/s; None for normal latency probes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throughput_mbps: Option<f64>,
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

/// UDP bulk throughput transfer result (udpdownload / udpupload modes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpThroughputResult {
    pub remote_addr: String,
    /// Requested transfer size in bytes.
    pub payload_bytes: usize,
    /// Number of datagrams sent by the sender (server for download, client for upload).
    pub datagrams_sent: u32,
    /// Number of datagrams received by the receiver.
    pub datagrams_received: u32,
    /// Bytes acknowledged by the server (from CMD_REPORT); upload mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_acked: Option<usize>,
    /// Datagram loss percentage (based on unique seq_num gaps).
    pub loss_percent: f64,
    /// Total transfer window in ms (from first data packet to CMD_DONE/CMD_REPORT).
    pub transfer_ms: f64,
    /// Measured throughput in MB/s; None if transfer_ms = 0.
    pub throughput_mbps: Option<f64>,
    pub started_at: DateTime<Utc>,
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
            ErrorCategory::Dns => "dns",
            ErrorCategory::Tcp => "tcp",
            ErrorCategory::Tls => "tls",
            ErrorCategory::Http => "http",
            ErrorCategory::Udp => "udp",
            ErrorCategory::Timeout => "timeout",
            ErrorCategory::Config => "config",
            ErrorCategory::Other => "other",
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
    let loss = if total > 0.0 {
        (total - received) / total * 100.0
    } else {
        100.0
    };

    if rtts.is_empty() {
        return RttStats {
            min: 0.0,
            avg: 0.0,
            p95: 0.0,
            jitter: 0.0,
            loss_percent: loss,
        };
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

    RttStats {
        min,
        avg,
        p95,
        jitter,
        loss_percent: loss,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics aggregation
// ─────────────────────────────────────────────────────────────────────────────

/// Descriptive statistics for a series of floating-point measurements.
#[derive(Debug, Clone)]
pub struct Stats {
    pub count: usize,
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub stddev: f64,
}

/// Compute summary statistics from a slice of `f64`.
/// Returns `None` if `values` is empty.
pub fn compute_stats(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = sorted.len();
    let min = sorted[0];
    let max = sorted[count - 1];
    let mean = sorted.iter().sum::<f64>() / count as f64;
    let p50 = percentile_from_sorted(&sorted, 50.0);
    let p95 = percentile_from_sorted(&sorted, 95.0);
    let p99 = percentile_from_sorted(&sorted, 99.0);
    let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count as f64;
    let stddev = variance.sqrt();
    Some(Stats {
        count,
        min,
        mean,
        p50,
        p95,
        p99,
        max,
        stddev,
    })
}

fn percentile_from_sorted(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = p / 100.0 * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

/// Human-readable label for the primary metric used when computing statistics
/// for a given protocol.
pub fn primary_metric_label(proto: &Protocol) -> &'static str {
    match proto {
        Protocol::Http1 | Protocol::Http2 | Protocol::Http3 => "Total ms",
        Protocol::Tcp => "Connect ms",
        Protocol::Udp => "RTT avg ms",
        Protocol::Download
        | Protocol::Upload
        | Protocol::WebDownload
        | Protocol::WebUpload
        | Protocol::UdpDownload
        | Protocol::UdpUpload => "Throughput MB/s",
        Protocol::Dns => "Resolve ms",
        Protocol::Tls => "Handshake ms",
    }
}

/// Extracts payload bytes from an attempt (throughput protocols only).
/// Returns None for non-throughput protocols and for payload == 0.
pub fn attempt_payload_bytes(a: &RequestAttempt) -> Option<usize> {
    a.http
        .as_ref()
        .map(|h| h.payload_bytes)
        .filter(|&b| b > 0)
        .or_else(|| {
            a.udp_throughput
                .as_ref()
                .map(|ut| ut.payload_bytes)
                .filter(|&b| b > 0)
        })
}

/// Extract the primary metric value from an attempt for statistics purposes.
/// Returns `None` if the relevant sub-result is absent.
pub fn primary_metric_value(a: &RequestAttempt) -> Option<f64> {
    match a.protocol {
        Protocol::Http1 | Protocol::Http2 | Protocol::Http3 => {
            a.http.as_ref().map(|h| h.total_duration_ms)
        }
        Protocol::Tcp => a.tcp.as_ref().map(|t| t.connect_duration_ms),
        Protocol::Udp => a.udp.as_ref().map(|u| u.rtt_avg_ms),
        Protocol::Download | Protocol::Upload | Protocol::WebDownload | Protocol::WebUpload => {
            a.http.as_ref().and_then(|h| h.throughput_mbps)
        }
        Protocol::UdpDownload | Protocol::UdpUpload => {
            a.udp_throughput.as_ref().and_then(|ut| ut.throughput_mbps)
        }
        Protocol::Dns => a.dns.as_ref().map(|d| d.duration_ms),
        Protocol::Tls => a.tls.as_ref().map(|t| t.handshake_duration_ms),
    }
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
            Some(1.0),
            Some(2.0),
            Some(3.0),
            Some(4.0),
            Some(5.0),
            Some(6.0),
            Some(7.0),
            Some(8.0),
            Some(9.0),
            Some(10.0),
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
        let samples: Vec<Option<f64>> = vec![Some(5.0), None, Some(10.0), None, Some(15.0)];
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
        for p in &[
            "tcp",
            "http1",
            "http2",
            "http3",
            "udp",
            "download",
            "upload",
            "webdownload",
            "webupload",
            "udpdownload",
            "udpupload",
        ] {
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
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
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

    #[test]
    fn compute_stats_empty_returns_none() {
        assert!(compute_stats(&[]).is_none());
    }

    #[test]
    fn compute_stats_single_value() {
        let s = compute_stats(&[7.0]).unwrap();
        assert_eq!(s.count, 1);
        assert!((s.min - 7.0).abs() < 1e-9);
        assert!((s.max - 7.0).abs() < 1e-9);
        assert!((s.mean - 7.0).abs() < 1e-9);
        assert!((s.p50 - 7.0).abs() < 1e-9);
        assert!((s.p95 - 7.0).abs() < 1e-9);
        assert!((s.p99 - 7.0).abs() < 1e-9);
        assert!((s.stddev - 0.0).abs() < 1e-9);
    }

    #[test]
    fn compute_stats_known_values() {
        // 1..=10: mean=5.5, stddev=sqrt(8.25)≈2.872, p50=5.5, p95=9.55, p99=9.91
        let vals: Vec<f64> = (1..=10).map(|v| v as f64).collect();
        let s = compute_stats(&vals).unwrap();
        assert_eq!(s.count, 10);
        assert!((s.min - 1.0).abs() < 1e-9);
        assert!((s.max - 10.0).abs() < 1e-9);
        assert!((s.mean - 5.5).abs() < 1e-9);
        // p50: rank=4.5 → 5+0.5*(6-5)=5.5
        assert!((s.p50 - 5.5).abs() < 1e-9);
        // p95: rank=8.55 → 9+0.55*(10-9)=9.55
        assert!((s.p95 - 9.55).abs() < 1e-9);
        // stddev of 1..10: variance = (sum of (i-5.5)^2 for i in 1..10)/10 = 8.25
        assert!((s.stddev - 8.25f64.sqrt()).abs() < 1e-9);
    }
}
