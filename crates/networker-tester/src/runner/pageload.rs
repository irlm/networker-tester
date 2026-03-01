/// Page-load simulation probes.
///
/// `run_pageload_probe`  – HTTP/1.1 with up to 6 persistent keep-alive connections (browser-like).
/// `run_pageload2_probe` – HTTP/2 with all assets multiplexed on one TLS connection.
/// `run_pageload3_probe` – HTTP/3 over QUIC (requires `--features http3`).
///
/// All probes:
///   1. Generate asset URLs from `cfg.asset_sizes` (one entry per asset).
///   2. Fetch a manifest request via `/page` first (for HTTP timing breakdown).
///   3. Fetch all listed assets concurrently or via keep-alive pool.
///   4. Return a `RequestAttempt` with `protocol = PageLoad | PageLoad2 | PageLoad3` and
///      `page_load = Some(PageLoadResult{…})`.
use crate::metrics::{
    ErrorCategory, ErrorRecord, HttpResult, PageLoadResult, Protocol, RequestAttempt,
    ServerTimingResult, TcpResult, TlsResult,
};
use crate::runner::dns as dns_runner;
use crate::runner::http::{build_tls_config, RunConfig};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1;
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
    /// One entry per asset; value = byte count for that asset.
    pub asset_sizes: Vec<usize>,
    /// Display name of the active preset, if any (e.g. "mixed").
    pub preset_name: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Named presets
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a named preset to a vector of per-asset byte counts.
pub fn resolve_preset(name: &str) -> anyhow::Result<Vec<usize>> {
    match name.to_lowercase().as_str() {
        "tiny" => Ok(vec![1_024; 100]),    // 100 × 1 KB
        "small" => Ok(vec![5_120; 50]),    //  50 × 5 KB
        "default" => Ok(vec![10_240; 20]), //  20 × 10 KB  (unchanged default)
        "medium" => Ok(vec![102_400; 10]), //  10 × 100 KB
        "large" => Ok(vec![1_048_576; 5]), //   5 × 1 MB
        "mixed" => {
            // 30 assets, ~820 KB total
            let mut v = vec![204_800usize; 1]; //   1 × 200 KB
            v.extend(vec![51_200; 4]); //   4 × 50 KB
            v.extend(vec![20_480; 10]); //  10 × 20 KB
            v.extend(vec![5_120; 15]); //  15 × 5 KB
            Ok(v)
        }
        other => Err(anyhow::anyhow!(
            "Unknown preset '{other}'. Valid: tiny, small, default, medium, large, mixed"
        )),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal connection result
// ─────────────────────────────────────────────────────────────────────────────

/// Per-asset result: (original_asset_idx, Option<(http_status, bytes_received, elapsed_ms)>)
type AssetResultVec = Vec<(usize, Option<(u16, usize, f64)>)>;

/// Result returned by each keep-alive connection task.
struct ConnResult {
    conn_idx: usize,
    tls_ms: f64,
    /// Populated only for connection 0 (manifest connection).
    tcp_result: Option<TcpResult>,
    tls_result: Option<TlsResult>,
    manifest_http: Option<HttpResult>,
    server_timing: Option<ServerTimingResult>,
    manifest_ttfb_ms: f64,
    asset_results: AssetResultVec,
    /// Non-empty if the connection itself failed to establish.
    conn_error: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/1.1 page-load probe (keep-alive connection pool)
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch the `/page` manifest then download all assets with up to 6 persistent
/// HTTP/1.1 keep-alive connections — accurately mimicking browser behaviour.
///
/// Connection pool assignment (round-robin):
///   conn 0: /page manifest → asset[0] → asset[k] → asset[2k] …
///   conn 1:                   asset[1] → asset[k+1] …
///   …
///   conn k-1:                 asset[k-1] → asset[2k-1] …
pub async fn run_pageload_probe(run_id: Uuid, seq: u32, cfg: &PageLoadConfig) -> RequestAttempt {
    let cpu_start = cpu_time::ProcessTime::now();
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    let target = &cfg.base_url;
    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad,
                ErrorCategory::Config,
                "Target URL has no host".into(),
            );
        }
    };
    let is_https = target.scheme() == "https";
    let port = target.port().unwrap_or(if is_https { 443 } else { 80 });
    let run_cfg = &cfg.run_cfg;

    // ── DNS resolution (shared across all connections) ────────────────────────
    let (server_addr, dns_result) = if run_cfg.dns_enabled {
        match dns_runner::resolve(&host, run_cfg.ipv4_only, run_cfg.ipv6_only).await {
            Ok((ips, r)) => {
                let ip = pick_ip(&ips, run_cfg.ipv4_only);
                debug!("DNS {} → {} ({:.1}ms)", host, ip, r.duration_ms);
                (SocketAddr::new(ip, port), Some(r))
            }
            Err(e) => {
                return error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad,
                    e.category,
                    e.message,
                );
            }
        }
    } else {
        match host.parse::<IpAddr>() {
            Ok(ip) => (SocketAddr::new(ip, port), None),
            Err(_) => {
                return error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad,
                    ErrorCategory::Config,
                    format!("dns_enabled=false but '{host}' is not a valid IP"),
                );
            }
        }
    };

    // ── Build asset URLs ──────────────────────────────────────────────────────
    let asset_urls = build_asset_urls(target, &cfg.asset_sizes);
    let n = asset_urls.len();
    let k = if n == 0 { 1 } else { n.min(6) };

    // Assign asset indices to connections in round-robin.
    let mut assignments: Vec<Vec<usize>> = vec![Vec::new(); k];
    for idx in 0..n {
        assignments[idx % k].push(idx);
    }

    // Manifest path — touches /page to record H1 timing; body not used.
    let representative_bytes = cfg.asset_sizes.first().copied().unwrap_or(10_240);
    let manifest_path = format!("/page?assets={n}&bytes={representative_bytes}");

    // ── TLS config (shared by all connections if HTTPS) ───────────────────────
    let tls_config = if is_https {
        match build_tls_config(
            &Protocol::Http1,
            run_cfg.insecure,
            run_cfg.ca_bundle.as_deref(),
        ) {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                return error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad,
                    ErrorCategory::Tls,
                    format!("TLS config error: {e}"),
                );
            }
        }
    } else {
        None
    };

    // ── Spawn k keep-alive connection tasks in parallel ───────────────────────
    let mut join_set: JoinSet<ConnResult> = JoinSet::new();

    for (conn_idx, conn_asset_indices) in assignments.into_iter().enumerate() {
        let asset_slice: Vec<(usize, url::Url)> = conn_asset_indices
            .iter()
            .map(|&i| (i, asset_urls[i].clone()))
            .collect();
        let is_manifest_conn = conn_idx == 0;
        let manifest_path_clone = if is_manifest_conn {
            Some(manifest_path.clone())
        } else {
            None
        };

        let host_clone = host.clone();
        let tls_config_clone = tls_config.clone();
        let timeout_ms = run_cfg.timeout_ms;

        join_set.spawn(async move {
            run_h1_keepalive_connection(
                server_addr,
                host_clone,
                is_https,
                tls_config_clone,
                timeout_ms,
                conn_idx,
                manifest_path_clone,
                asset_slice,
            )
            .await
        });
    }

    // Collect results, sorted by conn_idx.
    let mut conn_results: Vec<ConnResult> = Vec::with_capacity(k);
    while let Some(Ok(r)) = join_set.join_next().await {
        conn_results.push(r);
    }
    conn_results.sort_by_key(|r| r.conn_idx);

    let cpu_time_ms = Some(cpu_start.elapsed().as_secs_f64() * 1000.0);

    // ── Check conn 0 (manifest must succeed) ──────────────────────────────────
    let conn0 = match conn_results.first() {
        Some(r) => r,
        None => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad,
                ErrorCategory::Config,
                "No connection results".into(),
            );
        }
    };

    if let Some(ref err) = conn0.conn_error {
        return error_attempt_proto(
            attempt_id,
            run_id,
            seq,
            started_at,
            Protocol::PageLoad,
            ErrorCategory::Tcp,
            err.clone(),
        );
    }

    // ── Aggregate asset results across all connections ────────────────────────
    let mut asset_timings_ms = vec![0.0f64; n];
    let mut assets_fetched = 0usize;
    let mut total_bytes = 0usize;

    for cr in &conn_results {
        for (orig_idx, result) in &cr.asset_results {
            if let Some((status, bytes, elapsed)) = result {
                if *status < 500 {
                    assets_fetched += 1;
                    total_bytes += bytes;
                    if *orig_idx < asset_timings_ms.len() {
                        asset_timings_ms[*orig_idx] = *elapsed;
                    }
                }
            }
        }
    }

    // ── TLS accounting ────────────────────────────────────────────────────────
    let per_connection_tls_ms: Vec<f64> = conn_results.iter().map(|r| r.tls_ms).collect();
    let tls_setup_ms: f64 = per_connection_tls_ms.iter().sum();
    let total_ms = t_wall.elapsed().as_secs_f64() * 1000.0;
    let tls_overhead_ratio = if total_ms > 0.0 {
        tls_setup_ms / total_ms
    } else {
        0.0
    };

    let page_load = PageLoadResult {
        asset_count: n,
        assets_fetched,
        total_bytes,
        total_ms,
        ttfb_ms: conn0.manifest_ttfb_ms,
        connections_opened: k as u32,
        asset_timings_ms,
        started_at,
        tls_setup_ms,
        tls_overhead_ratio,
        per_connection_tls_ms,
        cpu_time_ms,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::PageLoad,
        sequence_num: seq,
        started_at,
        finished_at: Some(Utc::now()),
        success: n > 0 && assets_fetched == n,
        dns: dns_result,
        tcp: conn0.tcp_result.clone(),
        tls: conn0.tls_result.clone(),
        http: conn0.manifest_http.clone(),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: conn0.server_timing.clone(),
        udp_throughput: None,
        page_load: Some(page_load),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-connection H1.1 keep-alive worker
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_h1_keepalive_connection(
    server_addr: SocketAddr,
    host: String,
    is_https: bool,
    tls_config: Option<Arc<rustls::ClientConfig>>,
    timeout_ms: u64,
    conn_idx: usize,
    // Some(path) → this conn fetches the manifest at that path before assets.
    manifest_path: Option<String>,
    // Assets assigned to this connection: (original_asset_idx, url).
    asset_infos: Vec<(usize, url::Url)>,
) -> ConnResult {
    let timeout = std::time::Duration::from_millis(timeout_ms);

    // ── TCP connect ───────────────────────────────────────────────────────────
    let tcp_started_at = Utc::now();
    let t_tcp = Instant::now();
    let tcp_stream = match tokio::time::timeout(timeout, TcpStream::connect(server_addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return ConnResult {
                conn_idx,
                tls_ms: 0.0,
                tcp_result: None,
                tls_result: None,
                manifest_http: None,
                server_timing: None,
                manifest_ttfb_ms: 0.0,
                asset_results: Vec::new(),
                conn_error: Some(format!("TCP connect failed: {e}")),
            };
        }
        Err(_) => {
            return ConnResult {
                conn_idx,
                tls_ms: 0.0,
                tcp_result: None,
                tls_result: None,
                manifest_http: None,
                server_timing: None,
                manifest_ttfb_ms: 0.0,
                asset_results: Vec::new(),
                conn_error: Some(format!("TCP connect timed out after {timeout_ms}ms")),
            };
        }
    };
    let tcp_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let tcp_result_data = TcpResult {
        local_addr,
        remote_addr: server_addr.to_string(),
        connect_duration_ms: tcp_ms,
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

    // ── TLS (if HTTPS) or plain TCP → hyper H1.1 handshake ───────────────────
    // We use a macro-like pattern to avoid code duplication across TLS/plain paths.
    // Both paths produce the same `SendRequest<Full<Bytes>>` type.
    let tls_started_at = Utc::now();
    let t_tls = Instant::now();

    macro_rules! conn_error {
        ($msg:expr) => {
            return ConnResult {
                conn_idx,
                tls_ms: 0.0,
                tcp_result: if conn_idx == 0 {
                    Some(tcp_result_data)
                } else {
                    None
                },
                tls_result: None,
                manifest_http: None,
                server_timing: None,
                manifest_ttfb_ms: 0.0,
                asset_results: Vec::new(),
                conn_error: Some($msg),
            }
        };
    }

    let (mut send_req, tls_ms, tls_result_data): (
        http1::SendRequest<Full<Bytes>>,
        f64,
        Option<TlsResult>,
    ) = if is_https {
        let cfg = match tls_config {
            Some(c) => c,
            None => conn_error!("TLS config missing for HTTPS connection".into()),
        };
        let connector = TlsConnector::from(cfg);
        let server_name = match ServerName::try_from(host.clone()) {
            Ok(n) => n,
            Err(e) => conn_error!(format!("Invalid SNI: {e}")),
        };
        let tls_stream =
            match tokio::time::timeout(timeout, connector.connect(server_name, tcp_stream)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => conn_error!(format!("TLS handshake failed: {e}")),
                Err(_) => conn_error!(format!("TLS timed out after {timeout_ms}ms")),
            };
        let tls_ms = t_tls.elapsed().as_secs_f64() * 1000.0;

        // Extract TLS metadata before consuming the stream.
        let tls_result_data = if conn_idx == 0 {
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
            Some(TlsResult {
                protocol_version,
                cipher_suite,
                alpn_negotiated,
                cert_subject: None,
                cert_issuer: None,
                cert_expiry: None,
                handshake_duration_ms: tls_ms,
                started_at: tls_started_at,
                success: true,
                cert_chain: vec![],
                tls_backend: Some("rustls".into()),
            })
        } else {
            None
        };

        let io = TokioIo::new(tls_stream);
        let (sr, conn) = match http1::handshake(io).await {
            Ok(pair) => pair,
            Err(e) => conn_error!(format!("H1.1 handshake failed: {e}")),
        };
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                debug!("pageload H1.1 TLS connection error: {e}");
            }
        });
        (sr, tls_ms, tls_result_data)
    } else {
        let io = TokioIo::new(tcp_stream);
        let (sr, conn) = match http1::handshake(io).await {
            Ok(pair) => pair,
            Err(e) => conn_error!(format!("H1.1 handshake failed: {e}")),
        };
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                debug!("pageload H1.1 plain connection error: {e}");
            }
        });
        (sr, 0.0, None)
    };

    // ── Manifest request (conn 0 only) ────────────────────────────────────────
    let (manifest_http, server_timing, manifest_ttfb_ms) = if let Some(path) = manifest_path {
        let req = Request::builder()
            .method("GET")
            .uri(&path)
            .header("host", &host)
            .header("user-agent", "networker-tester/0.1")
            .header("accept", "*/*")
            .body(Full::new(Bytes::new()))
            .expect("valid manifest request");
        let manifest_send_at = Utc::now();
        let t0 = Instant::now();
        match tokio::time::timeout(timeout, send_req.send_request(req)).await {
            Ok(Ok(resp)) => {
                let ttfb_ms = t0.elapsed().as_secs_f64() * 1000.0;
                let status = resp.status().as_u16();
                let headers = resp.headers().clone();
                // Fully drain body before reusing connection.
                let body = resp.collect().await.ok().map(|b| b.to_bytes());
                let body_bytes = body.as_ref().map(|b| b.len()).unwrap_or(0);
                let total_ms = t0.elapsed().as_secs_f64() * 1000.0;

                let st = parse_server_timing_simple(&headers, manifest_send_at, ttfb_ms);
                let http = HttpResult {
                    negotiated_version: "HTTP/1.1".into(),
                    status_code: status,
                    headers_size_bytes: headers
                        .iter()
                        .map(|(k, v)| k.as_str().len() + v.len() + 4)
                        .sum(),
                    body_size_bytes: body_bytes,
                    ttfb_ms,
                    total_duration_ms: total_ms,
                    redirect_count: 0,
                    started_at: tls_started_at,
                    response_headers: headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect(),
                    payload_bytes: 0,
                    throughput_mbps: None,
                };
                (Some(http), st, ttfb_ms)
            }
            _ => (None, None, 0.0),
        }
    } else {
        (None, None, 0.0)
    };

    // ── Sequential asset fetches on this connection ───────────────────────────
    let mut asset_results: AssetResultVec = Vec::with_capacity(asset_infos.len());

    for (orig_idx, url) in &asset_infos {
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
        let t0 = Instant::now();
        match tokio::time::timeout(timeout, send_req.send_request(req)).await {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                // Must fully drain the response body to allow connection reuse.
                let body = resp.collect().await.ok().map(|b| b.to_bytes());
                let bytes = body.as_ref().map(|b| b.len()).unwrap_or(0);
                let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
                asset_results.push((*orig_idx, Some((status, bytes, elapsed))));
            }
            _ => {
                asset_results.push((*orig_idx, None));
            }
        }
    }

    ConnResult {
        conn_idx,
        tls_ms,
        tcp_result: if conn_idx == 0 {
            Some(tcp_result_data)
        } else {
            None
        },
        tls_result: tls_result_data,
        manifest_http,
        server_timing,
        manifest_ttfb_ms,
        asset_results,
        conn_error: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/2 page-load probe (all assets multiplexed on one connection)
// ─────────────────────────────────────────────────────────────────────────────

/// Establish one TLS+HTTP/2 connection to the target host and fetch all assets
/// concurrently via H2 stream multiplexing.
pub async fn run_pageload2_probe(run_id: Uuid, seq: u32, cfg: &PageLoadConfig) -> RequestAttempt {
    let cpu_start = cpu_time::ProcessTime::now();
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
    let tcp_result = TcpResult {
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
        TlsResult {
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
    let n = cfg.asset_sizes.len();
    let representative_bytes = cfg.asset_sizes.first().copied().unwrap_or(10_240);
    let manifest_path = format!("/page?assets={n}&bytes={representative_bytes}");
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
    let asset_urls = build_asset_urls(&cfg.base_url, &cfg.asset_sizes);
    let n = asset_urls.len();
    let run_cfg_ref = run_cfg;

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
                    std::time::Duration::from_millis(run_cfg_ref.timeout_ms),
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

    let cpu_time_ms = Some(cpu_start.elapsed().as_secs_f64() * 1000.0);
    let total_ms = t_wall.elapsed().as_secs_f64() * 1000.0;
    let tls_overhead_ratio = if total_ms > 0.0 {
        tls_duration_ms / total_ms
    } else {
        0.0
    };

    let page_load = PageLoadResult {
        asset_count: n,
        assets_fetched,
        total_bytes,
        total_ms,
        ttfb_ms,
        connections_opened: 1,
        asset_timings_ms: asset_timings,
        started_at,
        tls_setup_ms: tls_duration_ms,
        tls_overhead_ratio,
        per_connection_tls_ms: vec![tls_duration_ms],
        cpu_time_ms,
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

fn build_asset_urls(base: &url::Url, sizes: &[usize]) -> Vec<url::Url> {
    sizes
        .iter()
        .enumerate()
        .map(|(i, &bytes)| {
            let mut u = base.clone();
            u.set_path("/asset");
            u.set_query(Some(&format!("id={i}&bytes={bytes}")));
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
    error_attempt_proto(
        attempt_id,
        run_id,
        seq,
        started_at,
        Protocol::PageLoad2,
        category,
        message,
    )
}

fn error_attempt_proto(
    attempt_id: Uuid,
    run_id: Uuid,
    seq: u32,
    started_at: chrono::DateTime<Utc>,
    protocol: Protocol,
    category: ErrorCategory,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol,
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

// ─────────────────────────────────────────────────────────────────────────────
// HTTP/3 page-load probe (feature-gated — requires `--features http3`)
// ─────────────────────────────────────────────────────────────────────────────

/// Stub returned when the `http3` feature is disabled.
#[cfg(not(feature = "http3"))]
pub async fn run_pageload3_probe(run_id: Uuid, seq: u32, _cfg: &PageLoadConfig) -> RequestAttempt {
    error_attempt_proto(
        Uuid::new_v4(),
        run_id,
        seq,
        chrono::Utc::now(),
        Protocol::PageLoad3,
        ErrorCategory::Config,
        "HTTP/3 support not compiled in. Rebuild with --features http3".into(),
    )
}

/// Establish one QUIC+HTTP/3 connection and fetch all assets concurrently
/// via H3 stream multiplexing.
#[cfg(feature = "http3")]
pub async fn run_pageload3_probe(run_id: Uuid, seq: u32, cfg: &PageLoadConfig) -> RequestAttempt {
    use bytes::Buf;
    use h3_quinn::Connection as QuinnH3Connection;
    use quinn::{ClientConfig as QuinnClientConfig, Endpoint};
    use std::sync::Arc;

    let cpu_start = cpu_time::ProcessTime::now();
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    let target = &cfg.base_url;
    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Config,
                "Target URL has no host".into(),
            );
        }
    };
    if target.scheme() != "https" {
        return error_attempt_proto(
            attempt_id,
            run_id,
            seq,
            started_at,
            Protocol::PageLoad3,
            ErrorCategory::Config,
            "pageload3 requires an HTTPS target (HTTP/3 needs QUIC/TLS)".into(),
        );
    }
    let port = target.port().unwrap_or(443);
    let run_cfg = &cfg.run_cfg;

    // ── QUIC / TLS config ─────────────────────────────────────────────────────
    let mut tls_cfg = match build_tls_config(
        &Protocol::Http1,
        run_cfg.insecure,
        run_cfg.ca_bundle.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Tls,
                format!("TLS config error: {e}"),
            );
        }
    };
    tls_cfg.alpn_protocols = vec![b"h3".to_vec()];

    let quinn_tls = QuinnClientConfig::new(Arc::new(
        match quinn::crypto::rustls::QuicClientConfig::try_from(tls_cfg) {
            Ok(c) => c,
            Err(e) => {
                return error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    ErrorCategory::Tls,
                    format!("QUIC TLS config error: {e}"),
                );
            }
        },
    ));

    let mut endpoint = match Endpoint::client("0.0.0.0:0".parse().unwrap()) {
        Ok(e) => e,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Config,
                format!("QUIC endpoint creation failed: {e}"),
            );
        }
    };
    endpoint.set_default_client_config(quinn_tls);

    // ── DNS / address resolution ──────────────────────────────────────────────
    let addr_str = format!("{host}:{port}");
    let server_addr: std::net::SocketAddr = match addr_str.parse() {
        Ok(a) => a,
        Err(_) => match tokio::net::lookup_host(&addr_str).await {
            Ok(mut a) => match a.next() {
                Some(sa) => sa,
                None => {
                    return error_attempt_proto(
                        attempt_id,
                        run_id,
                        seq,
                        started_at,
                        Protocol::PageLoad3,
                        ErrorCategory::Dns,
                        format!("No addresses resolved for {host}"),
                    );
                }
            },
            Err(e) => {
                return error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    ErrorCategory::Dns,
                    format!("DNS error: {e}"),
                );
            }
        },
    };

    // ── QUIC handshake (includes TLS 1.3) ────────────────────────────────────
    let t_handshake = Instant::now();
    let connecting = match endpoint.connect(server_addr, &host) {
        Ok(c) => c,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Tcp,
                format!("QUIC connect error: {e}"),
            );
        }
    };
    let conn = match tokio::time::timeout(
        std::time::Duration::from_millis(run_cfg.timeout_ms),
        connecting,
    )
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Tcp,
                format!("QUIC connect: {e}"),
            );
        }
        Err(_) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Timeout,
                format!("QUIC handshake timed out after {}ms", run_cfg.timeout_ms),
            );
        }
    };
    let handshake_ms = t_handshake.elapsed().as_secs_f64() * 1000.0;

    // ── H3 client ─────────────────────────────────────────────────────────────
    let (mut driver, mut send_req) = match h3::client::new(QuinnH3Connection::new(conn)).await {
        Ok(pair) => pair,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Http,
                format!("H3 handshake: {e}"),
            );
        }
    };

    tokio::spawn(async move {
        let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    // ── Manifest request (measures ttfb on first request) ────────────────────
    let n = cfg.asset_sizes.len();
    let representative_bytes = cfg.asset_sizes.first().copied().unwrap_or(10_240);
    let manifest_path = format!("/page?assets={n}&bytes={representative_bytes}");
    let manifest_req = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}:{port}{manifest_path}"))
        .header("user-agent", "networker-tester/0.1 (h3-pageload)")
        .body(())
        .expect("valid request");

    let http_started_at = Utc::now();
    let t_sent = Instant::now();
    let mut manifest_stream = match send_req.send_request(manifest_req).await {
        Ok(s) => s,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Http,
                format!("Manifest send_request: {e}"),
            );
        }
    };
    manifest_stream.finish().await.ok();

    let manifest_resp = match manifest_stream.recv_response().await {
        Ok(r) => r,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                ErrorCategory::Http,
                format!("Manifest recv_response: {e}"),
            );
        }
    };
    let ttfb_ms = t_sent.elapsed().as_secs_f64() * 1000.0;
    let manifest_status = manifest_resp.status().as_u16();
    let manifest_headers = manifest_resp.headers().clone();

    let mut manifest_body_bytes = 0usize;
    while let Some(chunk) = manifest_stream.recv_data().await.ok().flatten() {
        manifest_body_bytes += chunk.remaining();
    }
    let manifest_total_ms = t_sent.elapsed().as_secs_f64() * 1000.0;

    let tls_result = TlsResult {
        protocol_version: "TLSv1.3 (QUIC)".into(),
        cipher_suite: "QUIC-embedded".into(),
        alpn_negotiated: Some("h3".into()),
        cert_subject: None,
        cert_issuer: None,
        cert_expiry: None,
        handshake_duration_ms: handshake_ms,
        started_at: http_started_at,
        success: true,
        cert_chain: vec![],
        tls_backend: Some("rustls+quinn".into()),
    };
    let manifest_http = crate::metrics::HttpResult {
        negotiated_version: "HTTP/3".into(),
        status_code: manifest_status,
        headers_size_bytes: manifest_headers
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len() + 4)
            .sum(),
        body_size_bytes: manifest_body_bytes,
        ttfb_ms,
        total_duration_ms: manifest_total_ms,
        redirect_count: 0,
        started_at: http_started_at,
        response_headers: manifest_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        payload_bytes: 0,
        throughput_mbps: None,
    };

    // ── Asset requests: open all N streams sequentially, then receive concurrently ──
    let asset_urls = build_asset_urls(&cfg.base_url, &cfg.asset_sizes);
    let n = asset_urls.len();
    let dur = std::time::Duration::from_millis(run_cfg.timeout_ms);

    // Phase 1: open N streams (fast — just sends HEADERS frame per stream)
    let mut streams = Vec::with_capacity(n);
    for (i, _) in asset_urls.iter().enumerate() {
        let bytes = cfg.asset_sizes.get(i).copied().unwrap_or(10_240);
        let path = format!("/asset?id={i}&bytes={bytes}");
        let req = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}:{port}{path}"))
            .header("user-agent", "networker-tester/0.1 (h3-pageload)")
            .body(())
            .expect("valid asset request");
        match tokio::time::timeout(dur, send_req.send_request(req)).await {
            Ok(Ok(mut s)) => {
                s.finish().await.ok();
                streams.push(Some(s));
            }
            _ => streams.push(None),
        }
    }

    // Phase 2: receive all responses concurrently
    let asset_futures: Vec<_> = streams
        .into_iter()
        .map(|maybe_stream| async move {
            let mut stream = maybe_stream?;
            let t0 = Instant::now();
            let resp = stream.recv_response().await.ok()?;
            let status = resp.status().as_u16();
            let mut body_bytes = 0usize;
            while let Some(chunk) = stream.recv_data().await.ok().flatten() {
                body_bytes += chunk.remaining();
            }
            let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
            Some((status, body_bytes, elapsed))
        })
        .collect();

    let asset_results = futures::future::join_all(asset_futures).await;

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

    let cpu_time_ms = Some(cpu_start.elapsed().as_secs_f64() * 1000.0);
    let total_ms = t_wall.elapsed().as_secs_f64() * 1000.0;
    let tls_overhead_ratio = if total_ms > 0.0 {
        handshake_ms / total_ms
    } else {
        0.0
    };

    let page_load = PageLoadResult {
        asset_count: n,
        assets_fetched,
        total_bytes,
        total_ms,
        ttfb_ms,
        connections_opened: 1,
        asset_timings_ms: asset_timings,
        started_at,
        tls_setup_ms: handshake_ms,
        tls_overhead_ratio,
        per_connection_tls_ms: vec![handshake_ms],
        cpu_time_ms,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::PageLoad3,
        sequence_num: seq,
        started_at,
        finished_at: Some(Utc::now()),
        success: assets_fetched == n,
        dns: None,
        tcp: None,
        tls: Some(tls_result),
        http: Some(manifest_http),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: Some(page_load),
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_preset_default_matches_legacy() {
        let sizes = resolve_preset("default").unwrap();
        assert_eq!(sizes.len(), 20);
        assert!(sizes.iter().all(|&s| s == 10_240));
    }

    #[test]
    fn resolve_preset_mixed_has_30_assets() {
        let sizes = resolve_preset("mixed").unwrap();
        assert_eq!(sizes.len(), 30);
    }

    #[test]
    fn resolve_preset_unknown_returns_err() {
        assert!(resolve_preset("bogus").is_err());
    }

    #[test]
    fn build_asset_urls_per_size() {
        let base: url::Url = "http://localhost:8080/".parse().unwrap();
        let sizes = vec![1024usize, 2048, 4096];
        let urls = build_asset_urls(&base, &sizes);
        assert_eq!(urls.len(), 3);
        assert!(urls[0].query().unwrap().contains("bytes=1024"));
        assert!(urls[1].query().unwrap().contains("bytes=2048"));
        assert!(urls[2].query().unwrap().contains("bytes=4096"));
    }
}
