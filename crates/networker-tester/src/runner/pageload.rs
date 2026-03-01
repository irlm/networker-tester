/// Page-load simulation probes.
///
/// `run_pageload_probe`  – HTTP/1.1 with up to 6 parallel TCP connections (browser-like).
/// `run_pageload2_probe` – HTTP/2 with all assets multiplexed on one TLS connection.
///
/// Both probes:
///   1. Generate the same asset URLs that `/page?assets=N&bytes=B` would return.
///   2. Fetch a manifest request via `/page` first (for HTTP timing breakdown).
///   3. Fetch all listed assets concurrently.
///   4. Return a `RequestAttempt` with `protocol = PageLoad | PageLoad2` and
///      `page_load = Some(PageLoadResult{…})`.
use crate::metrics::{
    ErrorCategory, ErrorRecord, HttpResult, PageLoadResult, Protocol, RequestAttempt,
    ServerTimingResult,
};
use crate::runner::dns as dns_runner;
use crate::runner::http::{build_tls_config, run_probe, RunConfig};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_rustls::TlsConnector;
use tracing::debug;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PageLoadConfig {
    pub run_cfg: RunConfig,
    pub base_url: url::Url,
    pub asset_count: usize,
    pub asset_size: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/1.1 page-load probe (up to 6 parallel connections)
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch the `/page` manifest then download all assets with up to 6 concurrent
/// HTTP/1.1 connections — mimicking browser behaviour.
pub async fn run_pageload_probe(run_id: Uuid, seq: u32, cfg: &PageLoadConfig) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    // Step 1 — Fetch the manifest (records DNS/TCP/TLS/HTTP timing for this attempt).
    let mut manifest_url = cfg.base_url.clone();
    manifest_url.set_path("/page");
    manifest_url.set_query(Some(&format!(
        "assets={}&bytes={}",
        cfg.asset_count, cfg.asset_size
    )));
    let manifest_cfg = RunConfig {
        payload_size: 0,
        ..cfg.run_cfg.clone()
    };
    let manifest_attempt =
        run_probe(run_id, seq, Protocol::Http1, &manifest_url, &manifest_cfg).await;

    if !manifest_attempt.success {
        return RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::PageLoad,
            sequence_num: seq,
            started_at,
            finished_at: Some(Utc::now()),
            success: false,
            dns: manifest_attempt.dns,
            tcp: manifest_attempt.tcp,
            tls: manifest_attempt.tls,
            http: manifest_attempt.http,
            udp: None,
            error: manifest_attempt.error.or(Some(ErrorRecord {
                category: ErrorCategory::Http,
                message: "Page manifest fetch failed".into(),
                detail: None,
                occurred_at: Utc::now(),
            })),
            retry_count: 0,
            server_timing: manifest_attempt.server_timing,
            udp_throughput: None,
            page_load: None,
        };
    }

    let ttfb_ms = manifest_attempt
        .http
        .as_ref()
        .map(|h| h.ttfb_ms)
        .unwrap_or(0.0);

    // Step 2 — Build asset URLs (mirrors what the /page endpoint returns).
    let asset_urls = build_asset_urls(&cfg.base_url, cfg.asset_count, cfg.asset_size);
    let n = asset_urls.len();
    let connections_opened = n.min(6) as u32;

    // Step 3 — Fetch assets with at most 6 concurrent tasks.
    const MAX_CONNS: usize = 6;
    let mut join_set: JoinSet<RequestAttempt> = JoinSet::new();
    let mut assets_fetched = 0usize;
    let mut total_bytes = 0usize;
    let mut asset_timings: Vec<f64> = Vec::with_capacity(n);

    let mut iter = asset_urls.into_iter();
    let mut in_flight = 0usize;

    loop {
        // Fill up to MAX_CONNS
        while in_flight < MAX_CONNS {
            match iter.next() {
                Some(url) => {
                    let rc = cfg.run_cfg.clone();
                    join_set.spawn(async move {
                        run_probe(run_id, 0, Protocol::Http1, &url, &rc).await
                    });
                    in_flight += 1;
                }
                None => break,
            }
        }
        if in_flight == 0 {
            break;
        }
        if let Some(Ok(a)) = join_set.join_next().await {
            in_flight -= 1;
            if a.success {
                assets_fetched += 1;
                if let Some(h) = &a.http {
                    asset_timings.push(h.total_duration_ms);
                    total_bytes += h.body_size_bytes;
                }
            }
        }
    }

    let total_ms = t_wall.elapsed().as_secs_f64() * 1000.0;

    let page_load = PageLoadResult {
        asset_count: n,
        assets_fetched,
        total_bytes,
        total_ms,
        ttfb_ms,
        connections_opened,
        asset_timings_ms: asset_timings,
        started_at,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::PageLoad,
        sequence_num: seq,
        started_at,
        finished_at: Some(Utc::now()),
        success: assets_fetched == n,
        dns: manifest_attempt.dns,
        tcp: manifest_attempt.tcp,
        tls: manifest_attempt.tls,
        http: manifest_attempt.http,
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: manifest_attempt.server_timing,
        udp_throughput: None,
        page_load: Some(page_load),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/2 page-load probe (all assets multiplexed on one connection)
// ─────────────────────────────────────────────────────────────────────────────

/// Establish one TLS+HTTP/2 connection to the target host and fetch all assets
/// concurrently via H2 stream multiplexing.
pub async fn run_pageload2_probe(run_id: Uuid, seq: u32, cfg: &PageLoadConfig) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    let target = &cfg.base_url;
    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Config,
                "Target URL has no host".into(),
            );
        }
    };
    let scheme = target.scheme();
    if scheme != "https" {
        return error_attempt(
            attempt_id,
            run_id,
            seq,
            started_at,
            ErrorCategory::Config,
            "pageload2 requires an HTTPS target (HTTP/2 needs TLS+ALPN)".into(),
        );
    }
    let port = target.port().unwrap_or(443);
    let run_cfg = &cfg.run_cfg;

    // ── DNS ──────────────────────────────────────────────────────────────────
    let (addr, dns_result) = if run_cfg.dns_enabled {
        match dns_runner::resolve(&host, run_cfg.ipv4_only, run_cfg.ipv6_only).await {
            Ok((ips, r)) => {
                let ip = pick_ip(&ips, run_cfg.ipv4_only);
                debug!("DNS {} → {} ({:.1}ms)", host, ip, r.duration_ms);
                (SocketAddr::new(ip, port), Some(r))
            }
            Err(e) => {
                return error_attempt(attempt_id, run_id, seq, started_at, e.category, e.message);
            }
        }
    } else {
        match host.parse::<IpAddr>() {
            Ok(ip) => (SocketAddr::new(ip, port), None),
            Err(_) => {
                return error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Config,
                    format!("dns_enabled=false but '{host}' is not a valid IP"),
                );
            }
        }
    };

    // ── TCP connect ──────────────────────────────────────────────────────────
    let tcp_started_at = Utc::now();
    let t_tcp = Instant::now();
    let tcp_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(run_cfg.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Tcp,
                e.to_string(),
            );
        }
        Err(_) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Timeout,
                format!(
                    "TCP connect to {addr} timed out after {}ms",
                    run_cfg.timeout_ms
                ),
            );
        }
    };
    let tcp_duration_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let tcp_result = crate::metrics::TcpResult {
        local_addr,
        remote_addr: addr.to_string(),
        connect_duration_ms: tcp_duration_ms,
        attempt_count: 1,
        started_at: tcp_started_at,
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
    };

    // ── TLS handshake with ALPN h2 ───────────────────────────────────────────
    let tls_started_at = Utc::now();
    let t_tls = Instant::now();
    let tls_config = match build_tls_config(
        &Protocol::Http2,
        run_cfg.insecure,
        run_cfg.ca_bundle.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Tls,
                e.to_string(),
            );
        }
    };
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = match ServerName::try_from(host.clone()) {
        Ok(n) => n,
        Err(e) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Tls,
                format!("Invalid SNI: {e}"),
            );
        }
    };
    let tls_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(run_cfg.timeout_ms),
        connector.connect(server_name, tcp_stream),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Tls,
                e.to_string(),
            );
        }
        Err(_) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Timeout,
                format!("TLS handshake timed out after {}ms", run_cfg.timeout_ms),
            );
        }
    };
    let tls_duration_ms = t_tls.elapsed().as_secs_f64() * 1000.0;
    let tls_result = {
        let (_, conn) = tls_stream.get_ref();
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
        crate::metrics::TlsResult {
            protocol_version,
            cipher_suite,
            alpn_negotiated,
            cert_subject: None,
            cert_issuer: None,
            cert_expiry: None,
            handshake_duration_ms: tls_duration_ms,
            started_at: tls_started_at,
            success: true,
            cert_chain: vec![],
            tls_backend: Some("rustls".into()),
        }
    };

    // ── HTTP/2 handshake ─────────────────────────────────────────────────────
    let io = TokioIo::new(tls_stream);
    let (sender, conn) =
        match hyper::client::conn::http2::handshake::<_, _, Full<Bytes>>(TokioExecutor::new(), io)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                return error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Http,
                    format!("HTTP/2 handshake failed: {e}"),
                );
            }
        };

    // Drive the connection in the background.
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("pageload2 H2 connection error: {e}");
        }
    });

    // ── Manifest request ──────────────────────────────────────────────────────
    let manifest_path = format!("/page?assets={}&bytes={}", cfg.asset_count, cfg.asset_size);
    let t_manifest = Instant::now();
    let manifest_req = Request::builder()
        .method("GET")
        .uri(&manifest_path)
        .header("host", &host)
        .header("user-agent", "networker-tester/0.1")
        .header("accept", "*/*")
        .body(Full::new(Bytes::new()))
        .expect("valid request");

    let manifest_send_at = Utc::now();
    let t_sent = Instant::now();
    let manifest_resp = match tokio::time::timeout(
        std::time::Duration::from_millis(run_cfg.timeout_ms),
        sender.clone().send_request(manifest_req),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Http,
                format!("Manifest request failed: {e}"),
            );
        }
        Err(_) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Timeout,
                "Manifest request timed out".into(),
            );
        }
    };
    let ttfb_ms = t_sent.elapsed().as_secs_f64() * 1000.0;
    let manifest_status = manifest_resp.status().as_u16();
    let manifest_headers = manifest_resp.headers().clone();
    let manifest_body = manifest_resp.collect().await.ok().map(|b| b.to_bytes());
    let manifest_body_bytes = manifest_body.as_ref().map(|b| b.len()).unwrap_or(0);
    let manifest_total_ms = t_manifest.elapsed().as_secs_f64() * 1000.0;

    let server_timing = parse_server_timing_simple(&manifest_headers, manifest_send_at, ttfb_ms);
    let manifest_http = HttpResult {
        negotiated_version: "HTTP/2".into(),
        status_code: manifest_status,
        headers_size_bytes: manifest_headers
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len() + 4)
            .sum(),
        body_size_bytes: manifest_body_bytes,
        ttfb_ms,
        total_duration_ms: manifest_total_ms,
        redirect_count: 0,
        started_at: tls_started_at,
        response_headers: manifest_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        payload_bytes: 0,
        throughput_mbps: None,
    };

    // ── Asset requests (all in-flight simultaneously over the H2 connection) ──
    let asset_urls = build_asset_urls(&cfg.base_url, cfg.asset_count, cfg.asset_size);
    let n = asset_urls.len();

    let t_assets = Instant::now();
    let asset_futures: Vec<_> = asset_urls
        .iter()
        .map(|url| {
            let path = format!(
                "{}{}",
                url.path(),
                url.query().map(|q| format!("?{q}")).unwrap_or_default()
            );
            let req = Request::builder()
                .method("GET")
                .uri(&path)
                .header("host", &host)
                .header("user-agent", "networker-tester/0.1")
                .header("accept", "*/*")
                .body(Full::new(Bytes::new()))
                .expect("valid asset request");
            let mut s = sender.clone();
            async move {
                let t0 = Instant::now();
                match tokio::time::timeout(
                    std::time::Duration::from_millis(run_cfg.timeout_ms),
                    s.send_request(req),
                )
                .await
                {
                    Ok(Ok(resp)) => {
                        let status = resp.status().as_u16();
                        let body = resp.collect().await.ok().map(|b| b.to_bytes());
                        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
                        let body_bytes = body.as_ref().map(|b| b.len()).unwrap_or(0);
                        Some((status, body_bytes, elapsed))
                    }
                    _ => None,
                }
            }
        })
        .collect();

    let asset_results = futures::future::join_all(asset_futures).await;
    let _ = t_assets.elapsed(); // measured by t_wall below

    let mut assets_fetched = 0usize;
    let mut total_bytes = 0usize;
    let mut asset_timings: Vec<f64> = Vec::with_capacity(n);

    for (status, body_bytes, elapsed) in asset_results.into_iter().flatten() {
        if status < 500 {
            assets_fetched += 1;
            total_bytes += body_bytes;
            asset_timings.push(elapsed);
        }
    }

    let total_ms = t_wall.elapsed().as_secs_f64() * 1000.0;
    let page_load = PageLoadResult {
        asset_count: n,
        assets_fetched,
        total_bytes,
        total_ms,
        ttfb_ms,
        connections_opened: 1,
        asset_timings_ms: asset_timings,
        started_at,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::PageLoad2,
        sequence_num: seq,
        started_at,
        finished_at: Some(Utc::now()),
        success: assets_fetched == n,
        dns: dns_result,
        tcp: Some(tcp_result),
        tls: Some(tls_result),
        http: Some(manifest_http),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing,
        udp_throughput: None,
        page_load: Some(page_load),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn build_asset_urls(base: &url::Url, asset_count: usize, asset_size: usize) -> Vec<url::Url> {
    (0..asset_count)
        .map(|i| {
            let mut u = base.clone();
            u.set_path("/asset");
            u.set_query(Some(&format!("id={i}&bytes={asset_size}")));
            u
        })
        .collect()
}

fn pick_ip(ips: &[std::net::IpAddr], ipv4_only: bool) -> std::net::IpAddr {
    if ipv4_only {
        ips.iter()
            .find(|ip| ip.is_ipv4())
            .copied()
            .unwrap_or(ips[0])
    } else {
        ips[0]
    }
}

fn error_attempt(
    attempt_id: Uuid,
    run_id: Uuid,
    seq: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::PageLoad2,
        sequence_num: seq,
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
            detail: None,
            occurred_at: Utc::now(),
        }),
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
    }
}

fn parse_server_timing_simple(
    headers: &hyper::HeaderMap,
    client_send_at: chrono::DateTime<Utc>,
    ttfb_ms: f64,
) -> Option<ServerTimingResult> {
    let has_networker = headers.contains_key("x-networker-server-timestamp")
        || headers.contains_key("x-networker-server-version");
    if !has_networker && !headers.contains_key("server-timing") {
        return None;
    }
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
    Some(ServerTimingResult {
        request_id: None,
        server_timestamp,
        clock_skew_ms,
        recv_body_ms: None,
        processing_ms: None,
        total_server_ms: None,
        server_version,
    })
}
