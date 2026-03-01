/// HTTP/1.1, HTTP/2 and raw-TCP probes with per-phase timing.
///
/// Timing phases (all relative to request start):
///   dns_ms       – DNS resolution
///   tcp_ms       – TCP connect (after DNS)
///   tls_ms       – TLS handshake (after TCP, HTTPS only)
///   ttfb_ms      – Time from request sent to first response byte (HTTP status + headers)
///   total_ms     – dns + tcp + tls + body download
use crate::metrics::{
    DnsResult, ErrorCategory, ErrorRecord, HttpResult, Protocol, RequestAttempt,
    ServerTimingResult, TcpResult, TlsResult,
};
use crate::runner::{dns as dns_runner, socket_info::SocketInfo};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, warn};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Public configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub timeout_ms: u64,
    pub dns_enabled: bool,
    pub ipv4_only: bool,
    pub ipv6_only: bool,
    pub insecure: bool,
    pub payload_size: usize,
    /// Path to probe (defaults to "/")
    pub path: String,
    /// Path to a PEM CA bundle file to add to the trust store.
    pub ca_bundle: Option<String>,
    /// Explicit HTTP proxy URL (from --proxy flag).
    /// None means use env vars (HTTP_PROXY / HTTPS_PROXY) unless no_proxy is true.
    pub proxy: Option<String>,
    /// When true, bypass all proxy settings (--no-proxy flag).
    pub no_proxy: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            dns_enabled: true,
            ipv4_only: false,
            ipv6_only: false,
            insecure: false,
            payload_size: 0,
            path: "/".to_string(),
            ca_bundle: None,
            proxy: None,
            no_proxy: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Run one probe attempt for the given protocol and return a fully populated
/// `RequestAttempt`.  Failures at any phase are recorded in `attempt.error`.
pub async fn run_probe(
    run_id: Uuid,
    sequence_num: u32,
    protocol: Protocol,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    match protocol {
        Protocol::Http1
        | Protocol::Http2
        | Protocol::Tcp
        | Protocol::Download
        | Protocol::Upload
        | Protocol::WebDownload
        | Protocol::WebUpload => {
            run_http_or_tcp(
                run_id,
                attempt_id,
                sequence_num,
                protocol,
                target,
                cfg,
                started_at,
            )
            .await
        }
        other => RequestAttempt {
            attempt_id,
            run_id,
            protocol: other,
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
                category: ErrorCategory::Config,
                message: "Protocol not handled by http runner".into(),
                detail: None,
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core implementation
// ─────────────────────────────────────────────────────────────────────────────

async fn run_http_or_tcp(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    protocol: Protocol,
    target: &url::Url,
    cfg: &RunConfig,
    started_at: chrono::DateTime<Utc>,
) -> RequestAttempt {
    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return failed_attempt(
                run_id,
                attempt_id,
                sequence_num,
                protocol,
                started_at,
                ErrorCategory::Config,
                "Target URL has no host".into(),
                None,
                None,
                None,
            );
        }
    };

    let scheme = target.scheme();
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port = target.port().unwrap_or(default_port);

    // Determine effective proxy (None = direct connection).
    let proxy_url = effective_proxy(scheme, &host, cfg);

    // ── 1. DNS ────────────────────────────────────────────────────────────────
    // When routing through a proxy, resolve the proxy host; otherwise resolve
    // the target host directly.
    let (connect_host, connect_port) = if let Some(ref p) = proxy_url {
        let ph = p.host_str().unwrap_or("").to_string();
        let pp = p
            .port()
            .unwrap_or(if p.scheme() == "https" { 443 } else { 3128 });
        (ph, pp)
    } else {
        (host.clone(), port)
    };

    let (addr, dns_result): (SocketAddr, Option<DnsResult>) = if cfg.dns_enabled {
        match dns_runner::resolve(&connect_host, cfg.ipv4_only, cfg.ipv6_only).await {
            Ok((ips, r)) => {
                let ip = pick_ip(&ips, cfg.ipv4_only);
                debug!("DNS {} → {} ({:.1}ms)", connect_host, ip, r.duration_ms);
                (SocketAddr::new(ip, connect_port), Some(r))
            }
            Err(e) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    e.category,
                    e.message,
                    e.detail,
                    None,
                    None,
                );
            }
        }
    } else {
        match connect_host.parse::<IpAddr>() {
            Ok(ip) => (SocketAddr::new(ip, connect_port), None),
            Err(_) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Config,
                    format!("dns_enabled=false but '{connect_host}' is not a valid IP"),
                    None,
                    None,
                    None,
                );
            }
        }
    };

    // ── 2. TCP connect ────────────────────────────────────────────────────────
    let tcp_started_at = Utc::now();
    let t_tcp = Instant::now();
    let tcp_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return failed_attempt(
                run_id,
                attempt_id,
                sequence_num,
                protocol,
                started_at,
                ErrorCategory::Tcp,
                e.to_string(),
                Some(format!("connect to {addr}")),
                dns_result,
                None,
            );
        }
        Err(_) => {
            return failed_attempt(
                run_id,
                attempt_id,
                sequence_num,
                protocol,
                started_at,
                ErrorCategory::Timeout,
                format!("TCP connect to {addr} timed out after {}ms", cfg.timeout_ms),
                None,
                dns_result,
                None,
            );
        }
    };
    let tcp_duration_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());

    let sock_info = SocketInfo::from_stream(&tcp_stream);
    let tcp_result = TcpResult {
        local_addr: local_addr.clone(),
        remote_addr: addr.to_string(),
        connect_duration_ms: tcp_duration_ms,
        attempt_count: 1,
        started_at: tcp_started_at,
        success: true,
        mss_bytes: sock_info.mss_bytes,
        rtt_estimate_ms: sock_info.rtt_estimate_ms,
        retransmits: sock_info.retransmits,
        total_retrans: sock_info.total_retrans,
        snd_cwnd: sock_info.snd_cwnd,
        snd_ssthresh: sock_info.snd_ssthresh,
        rtt_variance_ms: sock_info.rtt_variance_ms,
        rcv_space: sock_info.rcv_space,
        segs_out: sock_info.segs_out,
        segs_in: sock_info.segs_in,
        congestion_algorithm: sock_info.congestion_algorithm,
        delivery_rate_bps: sock_info.delivery_rate_bps,
        min_rtt_ms: sock_info.min_rtt_ms,
    };
    debug!("TCP connected to {addr} in {tcp_duration_ms:.1}ms (local={local_addr:?})");

    // ── 2b. Proxy CONNECT tunnel (HTTPS through proxy only) ───────────────────
    // For HTTPS targets: establish a transparent tunnel via CONNECT before TLS.
    // For HTTP targets: no tunnel needed; we use an absolute-form URI instead.
    let tcp_stream = if proxy_url.is_some() && scheme == "https" {
        match tokio::time::timeout(
            std::time::Duration::from_millis(cfg.timeout_ms),
            connect_via_proxy_tunnel(tcp_stream, &host, port),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Tcp,
                    format!("Proxy CONNECT failed: {e}"),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
            Err(_) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Timeout,
                    format!("Proxy CONNECT timed out after {}ms", cfg.timeout_ms),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        }
    } else {
        tcp_stream
    };

    // TCP-only mode: record connect, return.
    if protocol == Protocol::Tcp {
        drop(tcp_stream);
        return RequestAttempt {
            attempt_id,
            run_id,
            protocol,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: true,
            dns: dns_result,
            tcp: Some(tcp_result),
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
        };
    }

    // ── 3. TLS handshake (HTTPS only) ─────────────────────────────────────────
    let (tls_result, io_box): (Option<TlsResult>, Box<dyn IoStream>) = if scheme == "https" {
        let tls_started_at = Utc::now();
        let t_tls = Instant::now();

        let tls_config = match build_tls_config(&protocol, cfg.insecure, cfg.ca_bundle.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Tls,
                    e.to_string(),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        };
        let connector = TlsConnector::from(Arc::new(tls_config));

        let server_name = match ServerName::try_from(host.clone()) {
            Ok(n) => n,
            Err(e) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Tls,
                    format!("Invalid SNI: {e}"),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        };

        let tls_stream = match tokio::time::timeout(
            std::time::Duration::from_millis(cfg.timeout_ms),
            connector.connect(server_name, tcp_stream),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Tls,
                    e.to_string(),
                    Some("TLS handshake".into()),
                    dns_result,
                    Some(tcp_result),
                );
            }
            Err(_) => {
                return failed_attempt(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    ErrorCategory::Timeout,
                    format!("TLS handshake timed out after {}ms", cfg.timeout_ms),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        };
        let tls_duration_ms = t_tls.elapsed().as_secs_f64() * 1000.0;

        let tls_res = extract_tls_info(&tls_stream, tls_started_at, tls_duration_ms);
        debug!(
            "TLS handshake done in {tls_duration_ms:.1}ms (ALPN={:?}, ver={})",
            tls_res.alpn_negotiated, tls_res.protocol_version
        );

        (Some(tls_res), Box::new(tls_stream))
    } else {
        (None, Box::new(tcp_stream))
    };

    // ── 4. HTTP request ───────────────────────────────────────────────────────
    let http_started_at = Utc::now();
    let t_http = Instant::now();

    let path = if target.path().is_empty() {
        "/"
    } else {
        target.path()
    };
    let query = target.query().map(|q| format!("?{q}")).unwrap_or_default();
    let full_path = format!("{path}{query}");

    // HTTP through proxy requires an absolute-form URI:
    //   GET http://example.com:80/path HTTP/1.1
    // HTTPS through proxy uses a tunnel so we keep the origin-form URI.
    let request_uri = if proxy_url.is_some() && scheme != "https" {
        format!("http://{}:{}{}", host, port, full_path)
    } else {
        full_path
    };

    let http_result = match protocol {
        Protocol::Http1
        | Protocol::Download
        | Protocol::Upload
        | Protocol::WebDownload
        | Protocol::WebUpload => {
            send_http1(
                io_box,
                &host,
                &request_uri,
                cfg,
                http_started_at,
                t_http,
                attempt_id,
            )
            .await
        }
        Protocol::Http2 => {
            send_http2(
                io_box,
                &host,
                &request_uri,
                cfg,
                http_started_at,
                t_http,
                attempt_id,
            )
            .await
        }
        _ => unreachable!(),
    };

    match http_result {
        Ok((h, server_timing)) => RequestAttempt {
            attempt_id,
            run_id,
            protocol,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: h.status_code < 500,
            dns: dns_result,
            tcp: Some(tcp_result),
            tls: tls_result,
            http: Some(h),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing,
            udp_throughput: None,
            page_load: None,
        },
        Err(e) => {
            warn!("HTTP request failed: {e}");
            failed_attempt(
                run_id,
                attempt_id,
                sequence_num,
                protocol,
                started_at,
                ErrorCategory::Http,
                e.to_string(),
                None,
                dns_result,
                Some(tcp_result),
            )
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/1.1
// ─────────────────────────────────────────────────────────────────────────────

async fn send_http1(
    io_box: Box<dyn IoStream>,
    host: &str,
    path: &str,
    cfg: &RunConfig,
    started_at: chrono::DateTime<Utc>,
    t0: Instant,
    attempt_id: Uuid,
) -> anyhow::Result<(HttpResult, Option<ServerTimingResult>)> {
    let io = TokioIo::new(io_box);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io).await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("HTTP/1.1 connection error: {e}");
        }
    });

    let req = build_request(host, path, cfg, "HTTP/1.1", attempt_id)?;
    let client_send_at = Utc::now();
    let t_sent = Instant::now();

    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        sender.send_request(req),
    )
    .await
    .map_err(|_| anyhow::anyhow!("HTTP/1.1 request timed out"))??;

    let ttfb_ms = t_sent.elapsed().as_secs_f64() * 1000.0;
    collect_response(resp, "HTTP/1.1", started_at, ttfb_ms, t0, client_send_at).await
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/2
// ─────────────────────────────────────────────────────────────────────────────

async fn send_http2(
    io_box: Box<dyn IoStream>,
    host: &str,
    path: &str,
    cfg: &RunConfig,
    started_at: chrono::DateTime<Utc>,
    t0: Instant,
    attempt_id: Uuid,
) -> anyhow::Result<(HttpResult, Option<ServerTimingResult>)> {
    let io = TokioIo::new(io_box);
    let (mut sender, conn) =
        hyper::client::conn::http2::handshake::<_, _, Full<Bytes>>(TokioExecutor::new(), io)
            .await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("HTTP/2 connection error: {e}");
        }
    });

    let req = build_request(host, path, cfg, "HTTP/2", attempt_id)?;
    let client_send_at = Utc::now();
    let t_sent = Instant::now();

    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        sender.send_request(req),
    )
    .await
    .map_err(|_| anyhow::anyhow!("HTTP/2 request timed out"))??;

    let ttfb_ms = t_sent.elapsed().as_secs_f64() * 1000.0;
    collect_response(resp, "HTTP/2", started_at, ttfb_ms, t0, client_send_at).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

fn build_request(
    host: &str,
    path: &str,
    cfg: &RunConfig,
    _version_hint: &str,
    attempt_id: Uuid,
) -> anyhow::Result<Request<Full<Bytes>>> {
    let body = if cfg.payload_size > 0 {
        Bytes::from(vec![0u8; cfg.payload_size])
    } else {
        Bytes::new()
    };

    let method = if cfg.payload_size > 0 { "POST" } else { "GET" };

    Ok(Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("user-agent", "networker-tester/0.1")
        .header("accept", "*/*")
        .header("x-networker-request-id", attempt_id.to_string())
        .body(Full::new(body))?)
}

async fn collect_response(
    resp: Response<Incoming>,
    version: &str,
    started_at: chrono::DateTime<Utc>,
    ttfb_ms: f64,
    t0: Instant,
    client_send_at: chrono::DateTime<Utc>,
) -> anyhow::Result<(HttpResult, Option<ServerTimingResult>)> {
    let status_code = resp.status().as_u16();
    let headers = resp.headers().clone();

    let headers_size_bytes: usize = headers
        .iter()
        .map(|(k, v)| k.as_str().len() + v.len() + 4)
        .sum();

    let response_headers: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    // Parse server timing before consuming the body.
    let server_timing = parse_server_timing(&headers, client_send_at, ttfb_ms);

    let body = resp.collect().await?.to_bytes();
    let body_size_bytes = body.len();
    let total_duration_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let http = HttpResult {
        negotiated_version: version.to_string(),
        status_code,
        headers_size_bytes,
        body_size_bytes,
        ttfb_ms,
        total_duration_ms,
        redirect_count: 0,
        started_at,
        response_headers,
        payload_bytes: 0,
        throughput_mbps: None,
    };

    Ok((http, server_timing))
}

// ─────────────────────────────────────────────────────────────────────────────
// Server-Timing header parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Parse X-Networker-* and Server-Timing response headers into a
/// `ServerTimingResult`. Returns None if none of the relevant headers are present.
fn parse_server_timing(
    headers: &hyper::HeaderMap,
    client_send_at: chrono::DateTime<Utc>,
    ttfb_ms: f64,
) -> Option<ServerTimingResult> {
    let has_networker = headers.contains_key("x-networker-server-timestamp")
        || headers.contains_key("x-networker-request-id")
        || headers.contains_key("x-networker-server-version");

    if !has_networker && !headers.contains_key("server-timing") {
        return None;
    }

    let request_id = headers
        .get("x-networker-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    let server_version = headers
        .get("x-networker-server-version")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    let server_timestamp = headers
        .get("x-networker-server-timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let clock_skew_ms = server_timestamp.map(|ts| {
        let diff_ms = (ts - client_send_at)
            .num_microseconds()
            .map(|us| us as f64 / 1000.0)
            .unwrap_or_else(|| (ts - client_send_at).num_milliseconds() as f64);
        diff_ms - ttfb_ms / 2.0
    });

    let (recv_body_ms, processing_ms, total_server_ms) = headers
        .get("server-timing")
        .and_then(|v| v.to_str().ok())
        .map(parse_server_timing_header)
        .unwrap_or((None, None, None));

    Some(ServerTimingResult {
        request_id,
        server_timestamp,
        clock_skew_ms,
        recv_body_ms,
        processing_ms,
        total_server_ms,
        server_version,
    })
}

/// Parse `Server-Timing: recv;dur=X, proc;dur=Y, total;dur=Z` into a tuple.
fn parse_server_timing_header(value: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut recv = None;
    let mut proc_ms = None;
    let mut total = None;

    for entry in value.split(',') {
        let entry = entry.trim();
        let mut parts = entry.splitn(2, ';');
        let name = parts.next().map(str::trim).unwrap_or("").to_lowercase();
        let rest = parts.next().unwrap_or("");

        // Find dur= attribute among semicolon-separated attributes
        let dur = rest.split(';').find_map(|attr| {
            let attr = attr.trim().to_lowercase();
            attr.strip_prefix("dur=")
                .and_then(|s| s.parse::<f64>().ok())
        });

        match name.as_str() {
            "recv" => recv = dur,
            "proc" => proc_ms = dur,
            "total" => total = dur,
            _ => {}
        }
    }

    (recv, proc_ms, total)
}

// ─────────────────────────────────────────────────────────────────────────────
// Proxy helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the effective proxy URL for this request, or `None` for a direct
/// connection.  Priority: `cfg.proxy` > `HTTPS_PROXY`/`HTTP_PROXY` > `ALL_PROXY`.
/// `NO_PROXY` is respected when reading from environment variables (not when
/// `cfg.proxy` is set explicitly by the user via `--proxy`).
fn effective_proxy(scheme: &str, host: &str, cfg: &RunConfig) -> Option<url::Url> {
    if cfg.no_proxy {
        return None;
    }

    let raw = if let Some(p) = &cfg.proxy {
        p.clone()
    } else {
        // Check NO_PROXY before reading proxy env vars.
        let no_proxy = std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default();
        if is_no_proxy(host, &no_proxy) {
            return None;
        }

        let env_val = if scheme == "https" {
            std::env::var("HTTPS_PROXY")
                .or_else(|_| std::env::var("https_proxy"))
                .ok()
        } else {
            std::env::var("HTTP_PROXY")
                .or_else(|_| std::env::var("http_proxy"))
                .ok()
        }
        .or_else(|| {
            std::env::var("ALL_PROXY")
                .or_else(|_| std::env::var("all_proxy"))
                .ok()
        });

        env_val?
    };

    url::Url::parse(&raw).ok()
}

/// Returns `true` when `host` matches an entry in a comma-separated `NO_PROXY`
/// list (exact match or suffix match with a leading `.`).
fn is_no_proxy(host: &str, no_proxy: &str) -> bool {
    if no_proxy.is_empty() {
        return false;
    }
    let host_lower = host.to_lowercase();
    for entry in no_proxy.split(',') {
        let entry = entry.trim().to_lowercase();
        if entry.is_empty() {
            continue;
        }
        if entry == "*" || host_lower == entry || host_lower.ends_with(&format!(".{entry}")) {
            return true;
        }
    }
    false
}

/// Send an HTTP `CONNECT` request through an already-open TCP stream to
/// establish a tunnel to `target_host:target_port`.  Returns the same stream
/// once the proxy replies with `200`.
async fn connect_via_proxy_tunnel(
    mut stream: TcpStream,
    target_host: &str,
    target_port: u16,
) -> anyhow::Result<TcpStream> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let req = format!(
        "CONNECT {target_host}:{target_port} HTTP/1.1\r\n\
         Host: {target_host}:{target_port}\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("Proxy CONNECT write error: {e}"))?;

    // Read the response byte-by-byte until we see the end of headers.
    let mut response = Vec::with_capacity(256);
    loop {
        let mut buf = [0u8; 1];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("Proxy CONNECT response read error: {e}"))?;
        response.push(buf[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
        if response.len() > 8192 {
            anyhow::bail!("Proxy CONNECT response too long (>8 KiB)");
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    let status_line = response_str.lines().next().unwrap_or("");
    if !status_line.contains("200") {
        anyhow::bail!("Proxy CONNECT rejected: {status_line}");
    }

    Ok(stream)
}

pub(crate) fn build_tls_config(
    protocol: &Protocol,
    insecure: bool,
    ca_bundle: Option<&str>,
) -> anyhow::Result<rustls::ClientConfig> {
    let mut config: rustls::ClientConfig = if insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        // Also add native OS roots (best-effort; API returns CertificateResult in 0.8)
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            let _ = root_store.add(cert);
        }

        if let Some(bundle_path) = ca_bundle {
            crate::runner::tls::load_ca_bundle(&mut root_store, bundle_path)?;
        }

        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    // Advertise ALPN
    config.alpn_protocols = match protocol {
        Protocol::Http2 => vec![b"h2".to_vec()],
        _ => vec![b"http/1.1".to_vec()],
    };

    Ok(config)
}

fn extract_tls_info(
    stream: &tokio_rustls::client::TlsStream<TcpStream>,
    started_at: chrono::DateTime<Utc>,
    duration_ms: f64,
) -> TlsResult {
    let (_, conn) = stream.get_ref();

    let protocol_version = conn
        .protocol_version()
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "unknown".into());

    let cipher_suite = conn
        .negotiated_cipher_suite()
        .map(|c| format!("{:?}", c.suite()))
        .unwrap_or_else(|| "unknown".into());

    let alpn_negotiated = conn
        .alpn_protocol()
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(String::from);

    // Extract cert subject/issuer from the first peer certificate
    let (cert_subject, cert_issuer, cert_expiry) = conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .and_then(|cert| parse_cert_fields(cert.as_ref()))
        .unwrap_or((None, None, None));

    TlsResult {
        protocol_version,
        cipher_suite,
        alpn_negotiated,
        cert_subject,
        cert_issuer,
        cert_expiry,
        handshake_duration_ms: duration_ms,
        started_at,
        success: true,
        cert_chain: vec![],
        tls_backend: Some("rustls".into()),
    }
}

type CertFields = (
    Option<String>,
    Option<String>,
    Option<chrono::DateTime<Utc>>,
);

fn parse_cert_fields(der: &[u8]) -> Option<CertFields> {
    use x509_parser::prelude::*;
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    let subject = Some(cert.subject().to_string());
    let issuer = Some(cert.issuer().to_string());
    let expiry = {
        let not_after = cert.validity().not_after.timestamp();
        chrono::DateTime::from_timestamp(not_after, 0)
    };
    Some((subject, issuer, expiry))
}

fn pick_ip(ips: &[std::net::IpAddr], prefer_v4: bool) -> std::net::IpAddr {
    if prefer_v4 {
        ips.iter()
            .find(|ip| ip.is_ipv4())
            .copied()
            .unwrap_or(ips[0])
    } else {
        ips[0]
    }
}

#[allow(clippy::too_many_arguments)]
fn failed_attempt(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    protocol: Protocol,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
    dns: Option<DnsResult>,
    tcp: Option<TcpResult>,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: false,
        dns,
        tcp,
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

// ─────────────────────────────────────────────────────────────────────────────
// Trait object glue for TcpStream / TlsStream
// ─────────────────────────────────────────────────────────────────────────────

trait IoStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl IoStream for TcpStream {}
impl IoStream for tokio_rustls::client::TlsStream<TcpStream> {}

// ─────────────────────────────────────────────────────────────────────────────
// Insecure certificate verifier
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn init_crypto() {
        // rustls 0.23 requires a global CryptoProvider; install once per process.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn tls_config_http2_uses_h2_alpn() {
        init_crypto();
        let cfg = build_tls_config(&Protocol::Http2, false, None).unwrap();
        assert_eq!(cfg.alpn_protocols, vec![b"h2".to_vec()]);
    }

    #[test]
    fn tls_config_http1_uses_http11_alpn() {
        init_crypto();
        let cfg = build_tls_config(&Protocol::Http1, false, None).unwrap();
        assert_eq!(cfg.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

    #[test]
    fn no_proxy_bypasses_env_vars() {
        let cfg = RunConfig {
            no_proxy: true,
            ..Default::default()
        };
        // Even if env var were set, no_proxy=true should return None.
        assert!(effective_proxy("http", "example.com", &cfg).is_none());
    }

    #[test]
    fn is_no_proxy_exact_match() {
        assert!(is_no_proxy("example.com", "example.com,foo.com"));
    }

    #[test]
    fn is_no_proxy_suffix_match() {
        assert!(is_no_proxy("sub.example.com", "example.com"));
    }

    #[test]
    fn is_no_proxy_no_match() {
        assert!(!is_no_proxy("other.com", "example.com"));
    }

    #[test]
    fn is_no_proxy_wildcard() {
        assert!(is_no_proxy("anything.com", "*"));
    }

    #[test]
    fn pick_ip_prefers_v4() {
        let ips = vec!["::1".parse().unwrap(), "127.0.0.1".parse().unwrap()];
        let ip = pick_ip(&ips, true);
        assert!(ip.is_ipv4());
    }

    #[test]
    fn parse_server_timing_header_all_fields() {
        let (recv, proc_ms, total) =
            parse_server_timing_header("recv;dur=3.2, proc;dur=1.5, total;dur=10.0");
        assert!((recv.unwrap() - 3.2).abs() < 1e-9);
        assert!((proc_ms.unwrap() - 1.5).abs() < 1e-9);
        assert!((total.unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn parse_server_timing_header_partial() {
        let (recv, proc_ms, total) = parse_server_timing_header("proc;dur=5.0");
        assert!(recv.is_none());
        assert!((proc_ms.unwrap() - 5.0).abs() < 1e-9);
        assert!(total.is_none());
    }

    #[test]
    fn parse_server_timing_header_empty() {
        let (recv, proc_ms, total) = parse_server_timing_header("");
        assert!(recv.is_none());
        assert!(proc_ms.is_none());
        assert!(total.is_none());
    }

    #[tokio::test]
    #[ignore = "requires local endpoint"]
    async fn http1_probe_succeeds() {
        let cfg = RunConfig {
            timeout_ms: 5000,
            dns_enabled: false,
            insecure: true,
            ..Default::default()
        };
        let url = url::Url::parse("http://127.0.0.1:8080/health").unwrap();
        let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;
        assert!(attempt.success, "attempt failed: {:?}", attempt.error);
        assert!(attempt.http.is_some());
        assert_eq!(attempt.http.unwrap().negotiated_version, "HTTP/1.1");
    }

    #[tokio::test]
    #[ignore = "requires local endpoint with TLS"]
    async fn http2_probe_negotiates_h2() {
        let cfg = RunConfig {
            timeout_ms: 5000,
            dns_enabled: false,
            insecure: true,
            ..Default::default()
        };
        let url = url::Url::parse("https://127.0.0.1:8443/health").unwrap();
        let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http2, &url, &cfg).await;
        assert!(attempt.success, "attempt failed: {:?}", attempt.error);
        assert!(attempt.tls.is_some());
        let tls = attempt.tls.unwrap();
        assert_eq!(tls.alpn_negotiated.as_deref(), Some("h2"));
    }
}
