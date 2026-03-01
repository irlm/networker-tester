/// Curl probe: spawns the system `curl` binary and captures per-phase timing.
///
/// Uses `--write-out` to extract DNS, TCP connect, TLS handshake, TTFB, and
/// total time.  Maps to the same result structs as an http1 probe so all
/// existing reporting code works unchanged.
///
/// Requirements: `curl` must be on `$PATH`.  If it is not found the probe
/// returns a graceful error instead of panicking.
use crate::metrics::{
    DnsResult, ErrorCategory, ErrorRecord, HttpResult, Protocol, RequestAttempt, TcpResult,
    TlsResult,
};
use crate::runner::http::RunConfig;
use chrono::Utc;
use tracing::debug;
use uuid::Uuid;

/// Write-out format — one `key:value` per line.
/// Times are in seconds (curl default); we multiply by 1000 to get ms.
const WRITE_OUT: &str =
    "dns:%{time_namelookup}\nconnect:%{time_connect}\ntls:%{time_appconnect}\nttfb:%{time_starttransfer}\ntotal:%{time_total}\ncode:%{http_code}\nsize:%{size_download}\nurl_effective:%{url_effective}";

/// Run one curl probe and return a fully populated `RequestAttempt`.
pub async fn run_curl_probe(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t0 = std::time::Instant::now();

    let mut cmd = tokio::process::Command::new("curl");

    cmd.arg("--silent")
        .arg("--output")
        .arg(if cfg!(target_os = "windows") {
            "NUL"
        } else {
            "/dev/null"
        })
        .arg("--write-out")
        .arg(WRITE_OUT);

    // ── Options mapped from RunConfig ─────────────────────────────────────────
    if cfg.insecure {
        cmd.arg("--insecure");
    }
    if let Some(ref proxy) = cfg.proxy {
        cmd.arg("--proxy").arg(proxy);
    } else if cfg.no_proxy {
        // curl respects $no_proxy; for explicit bypass pass --noproxy '*'
        cmd.arg("--noproxy").arg("*");
    }
    if let Some(ref bundle) = cfg.ca_bundle {
        cmd.arg("--cacert").arg(bundle);
    }
    if cfg.ipv4_only {
        cmd.arg("--ipv4");
    } else if cfg.ipv6_only {
        cmd.arg("--ipv6");
    }

    // timeout in seconds (curl accepts fractional)
    let timeout_secs = cfg.timeout_ms as f64 / 1000.0;
    cmd.arg("--max-time").arg(format!("{timeout_secs:.3}"));

    cmd.arg(target.as_str());

    debug!("curl probe: {:?}", cmd);

    let output = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms + 5_000),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                "curl binary not found on PATH — install curl to use this probe mode".into()
            } else {
                format!("curl execution error: {e}")
            };
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Config,
                msg,
                None,
            );
        }
        Err(_) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Timeout,
                format!(
                    "curl probe timed out after {:.0}s",
                    cfg.timeout_ms as f64 / 1000.0
                ),
                None,
            );
        }
    };

    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // ── Parse write-out output ────────────────────────────────────────────────
    let stdout = String::from_utf8_lossy(&output.stdout);
    debug!("curl write-out: {stdout:?}");

    let parsed = parse_write_out(&stdout);

    // curl exit codes: 0 = success, 28 = timeout, 7 = couldn't connect, etc.
    let exit_ok = output.status.success();
    let exit_code = output.status.code().unwrap_or(-1);

    if !exit_ok && parsed.code == 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.is_empty() {
            None
        } else {
            Some(stderr.trim().to_string())
        };
        return make_failed(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            error_category_for_exit(exit_code),
            format!("curl exited with code {exit_code}"),
            detail,
        );
    }

    // ── DNS result ────────────────────────────────────────────────────────────
    let dns_result = if parsed.dns_ms > 0.0 {
        Some(DnsResult {
            query_name: target.host_str().unwrap_or("").to_string(),
            resolved_ips: vec![], // curl doesn't report the resolved IP in --write-out
            duration_ms: parsed.dns_ms,
            started_at,
            success: true,
        })
    } else {
        None
    };

    // ── TCP result ────────────────────────────────────────────────────────────
    // time_connect is cumulative from request start; subtract dns time.
    let tcp_ms = (parsed.connect_ms - parsed.dns_ms).max(0.0);
    let tcp_result = if parsed.connect_ms > 0.0 {
        Some(TcpResult {
            local_addr: None,
            remote_addr: target
                .host_str()
                .map(|h| {
                    let port =
                        target
                            .port()
                            .unwrap_or(if target.scheme() == "https" { 443 } else { 80 });
                    format!("{h}:{port}")
                })
                .unwrap_or_default(),
            connect_duration_ms: tcp_ms,
            attempt_count: 1,
            started_at,
            success: true,
            mss_bytes: None,
            rtt_estimate_ms: None,
            retransmits: None,
            total_retrans: None,
            snd_cwnd: None,
            snd_ssthresh: None,
            rtt_variance_ms: None,
            rcv_space: None,
            segs_out: None,
            segs_in: None,
            congestion_algorithm: None,
            delivery_rate_bps: None,
            min_rtt_ms: None,
        })
    } else {
        None
    };

    // ── TLS result (HTTPS only) ───────────────────────────────────────────────
    let is_https = target.scheme() == "https";
    // time_appconnect is cumulative; subtract connect time for TLS-only duration.
    let tls_ms = if is_https && parsed.tls_ms > 0.0 {
        (parsed.tls_ms - parsed.connect_ms).max(0.0)
    } else {
        0.0
    };
    let tls_result = if is_https && parsed.tls_ms > 0.0 {
        Some(TlsResult {
            protocol_version: "unknown".into(), // curl --write-out doesn't expose this
            cipher_suite: "unknown".into(),
            alpn_negotiated: None,
            cert_subject: None,
            cert_issuer: None,
            cert_expiry: None,
            handshake_duration_ms: tls_ms,
            started_at,
            success: true,
            cert_chain: vec![],
            tls_backend: Some("curl".into()),
        })
    } else {
        None
    };

    // ── HTTP result ───────────────────────────────────────────────────────────
    // time_starttransfer is cumulative from start; subtract tls (or connect for HTTP).
    let baseline = if is_https {
        parsed.tls_ms
    } else {
        parsed.connect_ms
    };
    let ttfb_ms = (parsed.ttfb_ms - baseline).max(0.0);
    // Use curl's own reported total time; fall back to wall-clock if curl reports zero.
    let total_duration_ms = if parsed.total_ms > 0.0 {
        parsed.total_ms
    } else {
        elapsed_ms
    };

    let http_result = HttpResult {
        negotiated_version: "HTTP/1.1".into(), // curl auto-negotiates; we don't know
        status_code: parsed.code as u16,
        headers_size_bytes: 0,
        body_size_bytes: parsed.size_bytes,
        ttfb_ms,
        total_duration_ms,
        redirect_count: 0,
        started_at,
        response_headers: vec![],
        payload_bytes: 0,
        throughput_mbps: None,
    };
    let success = parsed.code > 0 && parsed.code < 400;

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Curl,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success,
        dns: dns_result,
        tcp: tcp_result,
        tls: tls_result,
        http: Some(http_result),
        udp: None,
        error: if !success && parsed.code > 0 {
            Some(ErrorRecord {
                category: ErrorCategory::Http,
                message: format!("HTTP {}", parsed.code),
                detail: None,
                occurred_at: Utc::now(),
            })
        } else {
            None
        },
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Write-out parser
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct CurlTimes {
    dns_ms: f64,
    connect_ms: f64,
    tls_ms: f64,
    ttfb_ms: f64,
    total_ms: f64,
    code: u32,
    size_bytes: usize,
}

fn parse_write_out(s: &str) -> CurlTimes {
    let mut t = CurlTimes::default();
    for line in s.lines() {
        if let Some((key, val)) = line.split_once(':') {
            let val = val.trim();
            match key.trim() {
                "dns" => t.dns_ms = secs_to_ms(val),
                "connect" => t.connect_ms = secs_to_ms(val),
                "tls" => t.tls_ms = secs_to_ms(val),
                "ttfb" => t.ttfb_ms = secs_to_ms(val),
                "total" => t.total_ms = secs_to_ms(val),
                "code" => t.code = val.parse().unwrap_or(0),
                "size" => t.size_bytes = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    t
}

fn secs_to_ms(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0) * 1000.0
}

fn error_category_for_exit(code: i32) -> ErrorCategory {
    match code {
        6 | 7 => ErrorCategory::Dns,
        28 => ErrorCategory::Timeout,
        35 | 51 | 53 | 54 | 58 | 59 | 60 | 64 | 66 | 77 | 80 | 82 | 83 | 90 | 91 => {
            ErrorCategory::Tls
        }
        _ => ErrorCategory::Http,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error helper
// ─────────────────────────────────────────────────────────────────────────────

fn make_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Curl,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: false,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: Some(ErrorRecord {
            category,
            message,
            detail,
            occurred_at: Utc::now(),
        }),
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
    }
}
