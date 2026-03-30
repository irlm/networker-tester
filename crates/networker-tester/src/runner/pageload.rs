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
use crate::runner::socket_info::SocketInfo;
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
// Shared connection types for --connection-reuse
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-established HTTP/2 connection for reuse across runs.
/// The `sender` is `Clone`; each probe clones it to send requests independently.
pub struct SharedH2Conn {
    pub sender: hyper::client::conn::http2::SendRequest<Full<Bytes>>,
    pub host: String,
    pub addr: std::net::SocketAddr,
    pub dns_result: Option<crate::metrics::DnsResult>,
    pub tcp_result: crate::metrics::TcpResult,
    pub tls_result: crate::metrics::TlsResult,
    pub tls_duration_ms: f64,
}

/// Pre-established HTTP/3 QUIC connection for reuse across runs.
/// `send_req` takes `&mut self`, so callers must use `tokio::sync::Mutex`.
#[cfg(feature = "http3")]
pub struct SharedH3Conn {
    pub send_req: h3::client::SendRequest<h3_quinn::OpenStreams, bytes::Bytes>,
    pub _endpoint: quinn::Endpoint, // must stay alive to keep QUIC connection open
    pub host: String,
    pub port: u16,
    pub handshake_ms: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Named presets
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a named preset to a vector of per-asset byte counts.
pub fn resolve_preset(name: &str) -> anyhow::Result<Vec<usize>> {
    match name.to_lowercase().as_str() {
        // Modeled after real-world page profiles.
        // Reference: microsoft.com ≈ 333 requests, 8 MB transferred, 19 MB resources.
        //
        // tiny   — simple landing / API docs page         (10 assets, ~100 KB)
        // small  — blog article or lightweight SPA        (25 assets, ~900 KB)
        // default — corporate homepage (first-party)      (50 assets, ~6 MB)
        // medium — heavy SPA / full enterprise page       (100 assets, ~10 MB)
        // large  — media-rich portal                      (200 assets, ~31 MB)
        // mixed  — realistic varied-size distribution     (50 assets, ~7 MB)
        "tiny" => {
            // 10 assets, ~100 KB total — simple landing page
            let mut v = vec![2_048; 4]; //   4 × 2 KB   (icons, tiny scripts)
            v.extend(vec![10_240; 3]); //   3 × 10 KB  (CSS, small JS)
            v.extend(vec![20_480; 3]); //   3 × 20 KB  (images, fonts)
            Ok(v)
        }
        "small" => {
            // 25 assets, ~900 KB total — blog / article page
            let mut v = vec![1_024; 5]; //   5 × 1 KB   (tracking, tiny scripts)
            v.extend(vec![5_120; 5]); //   5 × 5 KB   (icons, small CSS)
            v.extend(vec![20_480; 5]); //   5 × 20 KB  (fonts, images)
            v.extend(vec![51_200; 5]); //   5 × 50 KB  (JS bundles)
            v.extend(vec![102_400; 5]); //   5 × 100 KB (hero images)
            Ok(v)
        }
        "default" => {
            // 50 assets, ~6 MB total — corporate homepage (microsoft.com first-party)
            let mut v = vec![1_024; 10]; //  10 × 1 KB   (tracking pixels, beacons)
            v.extend(vec![5_120; 8]); //   8 × 5 KB   (icons, small CSS)
            v.extend(vec![20_480; 8]); //   8 × 20 KB  (fonts, stylesheets)
            v.extend(vec![51_200; 8]); //   8 × 50 KB  (JS modules, images)
            v.extend(vec![153_600; 6]); //   6 × 150 KB (hero images, large CSS)
            v.extend(vec![307_200; 5]); //   5 × 300 KB (large JS bundles)
            v.extend(vec![512_000; 3]); //   3 × 500 KB (main JS bundle, hi-res img)
            v.extend(vec![819_200; 2]); //   2 × 800 KB (large media)
            Ok(v)
        }
        "medium" => {
            // 100 assets, ~10 MB total — microsoft.com full page (transferred)
            let mut v = vec![1_024; 25]; //  25 × 1 KB   (tracking, analytics, pixels)
            v.extend(vec![5_120; 15]); //  15 × 5 KB   (icons, small scripts)
            v.extend(vec![20_480; 15]); //  15 × 20 KB  (CSS, fonts)
            v.extend(vec![51_200; 15]); //  15 × 50 KB  (JS modules, thumbnails)
            v.extend(vec![102_400; 10]); //  10 × 100 KB (images)
            v.extend(vec![204_800; 8]); //   8 × 200 KB (hero images)
            v.extend(vec![409_600; 6]); //   6 × 400 KB (large JS bundles)
            v.extend(vec![614_400; 4]); //   4 × 600 KB (main bundles)
            v.extend(vec![1_048_576; 2]); //  2 × 1 MB   (large media)
            Ok(v)
        }
        "large" => {
            // 200 assets, ~31 MB total — microsoft.com uncompressed resources
            let mut v = vec![1_024; 50]; //  50 × 1 KB   (tracking, analytics)
            v.extend(vec![5_120; 30]); //  30 × 5 KB   (icons, small scripts)
            v.extend(vec![20_480; 30]); //  30 × 20 KB  (CSS, fonts)
            v.extend(vec![51_200; 25]); //  25 × 50 KB  (JS modules, thumbnails)
            v.extend(vec![102_400; 20]); //  20 × 100 KB (product images)
            v.extend(vec![204_800; 15]); //  15 × 200 KB (hero images)
            v.extend(vec![409_600; 12]); //  12 × 400 KB (large JS bundles)
            v.extend(vec![614_400; 8]); //   8 × 600 KB (main bundles)
            v.extend(vec![1_048_576; 5]); //   5 × 1 MB   (large media)
            v.extend(vec![2_097_152; 5]); //   5 × 2 MB   (video, hi-res images)
            Ok(v)
        }
        "mixed" => {
            // 50 assets, ~7 MB total — realistic varied-size distribution
            let mut v = vec![512; 5]; //   5 × 0.5 KB (tracking pixels)
            v.extend(vec![2_048; 8]); //   8 × 2 KB   (small icons, beacons)
            v.extend(vec![8_192; 7]); //   7 × 8 KB   (CSS, small scripts)
            v.extend(vec![25_600; 7]); //   7 × 25 KB  (fonts, medium images)
            v.extend(vec![51_200; 6]); //   6 × 50 KB  (JS modules)
            v.extend(vec![102_400; 5]); //   5 × 100 KB (images)
            v.extend(vec![204_800; 4]); //   4 × 200 KB (large images)
            v.extend(vec![409_600; 4]); //   4 × 400 KB (JS bundles)
            v.extend(vec![614_400; 2]); //   2 × 600 KB (main bundle)
            v.extend(vec![1_048_576; 2]); //   2 × 1 MB   (large media)
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
/// HTTP/1.1 keep-alive connections — accurately mimicking browser behavior.
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

    // Use plain HTTP for pageload1 — matches browser1 which also uses HTTP to
    // force HTTP/1.1 (no ALPN).  This removes TLS overhead from the comparison.
    let target = rewrite_to_http(&cfg.base_url);
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
    let asset_urls = build_asset_urls(&target, &cfg.asset_sizes);
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
        connection_reused: false,
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
        browser: None,
        http_stack: None,
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
    let _ = tcp_stream.set_nodelay(true);
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let sock_info = SocketInfo::from_stream(&tcp_stream);
    let tcp_result_data = TcpResult {
        local_addr,
        remote_addr: server_addr.to_string(),
        connect_duration_ms: tcp_ms,
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
                resumed: None,
                handshake_kind: None,
                tls13_tickets_received: None,
                previous_handshake_duration_ms: None,
                previous_handshake_kind: None,
                previous_http_status_code: None,
                http_status_code: None,
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
                    goodput_mbps: None,
                    cpu_time_ms: None,
                    csw_voluntary: None,
                    csw_involuntary: None,
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
    let _ = tcp_stream.set_nodelay(true);
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let sock_info = SocketInfo::from_stream(&tcp_stream);
    let tcp_result = TcpResult {
        local_addr,
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
            resumed: None,
            handshake_kind: None,
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
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
        goodput_mbps: None,
        cpu_time_ms: None,
        csw_voluntary: None,
        csw_involuntary: None,
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
        connection_reused: false,
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
        browser: None,
        http_stack: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Rewrite an HTTPS URL to plain HTTP with the corresponding port.
///
/// Used by `pageload1` to match `browser1` behavior — both use plain HTTP
/// so the comparison is apples-to-apples without TLS overhead.
///
/// Port mapping: 8443 → 8080 (endpoint), 8444 → 8081 (nginx),
/// 8445 → 8082 (IIS); 443/default → 80 (omitted), other → kept as-is.
fn rewrite_to_http(base: &url::Url) -> url::Url {
    if base.scheme() != "https" {
        return base.clone();
    }
    let mut u = base.clone();
    let _ = u.set_scheme("http");
    let http_port: Option<u16> = match base.port_or_known_default() {
        Some(8443) => Some(8080), // endpoint
        Some(8444) => Some(8081), // nginx stack
        Some(8445) => Some(8082), // IIS stack
        Some(443) | None => None,
        Some(p) => Some(p),
    };
    let _ = u.set_port(http_port);
    u
}

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
        browser: None,
        http_stack: None,
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
        "HTTP/3 support was excluded at compile time (built with --no-default-features)".into(),
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
        resumed: None,
        handshake_kind: None,
        tls13_tickets_received: None,
        previous_handshake_duration_ms: None,
        previous_handshake_kind: None,
        previous_http_status_code: None,
        http_status_code: None,
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
        goodput_mbps: None,
        cpu_time_ms: None,
        csw_voluntary: None,
        csw_involuntary: None,
    };

    // ── Asset requests: send + receive all concurrently (like a real browser) ──
    let asset_urls = build_asset_urls(&cfg.base_url, &cfg.asset_sizes);
    let n = asset_urls.len();
    let dur = std::time::Duration::from_millis(run_cfg.timeout_ms);

    // Build concurrent futures: each opens a QUIC stream, sends the request,
    // and receives the response — all in parallel on the multiplexed connection.
    let asset_futures: Vec<_> = asset_urls
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let mut sr = send_req.clone();
            let host = host.clone();
            async move {
                let bytes = cfg.asset_sizes.get(i).copied().unwrap_or(10_240);
                let path = format!("/asset?id={i}&bytes={bytes}");
                let req = http::Request::builder()
                    .method("GET")
                    .uri(format!("https://{host}:{port}{path}"))
                    .header("user-agent", "networker-tester/0.1 (h3-pageload)")
                    .body(())
                    .expect("valid asset request");
                let t0 = Instant::now();
                let mut stream = match tokio::time::timeout(dur, sr.send_request(req)).await {
                    Ok(Ok(s)) => s,
                    _ => return None,
                };
                stream.finish().await.ok();
                let resp = stream.recv_response().await.ok()?;
                let status = resp.status().as_u16();
                let mut body_bytes = 0usize;
                while let Some(chunk) = stream.recv_data().await.ok().flatten() {
                    body_bytes += chunk.remaining();
                }
                let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
                Some((status, body_bytes, elapsed))
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
        connection_reused: false,
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
        browser: None,
        http_stack: None,
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
        srv_csw_voluntary: None,
        srv_csw_involuntary: None,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Connection-reuse: warmup + warm probes (--connection-reuse)
// ─────────────────────────────────────────────────────────────────────────────

/// Establish an HTTP/2 connection and run one warmup page-load (cold).
/// Returns the warmup RequestAttempt and the shared connection for reuse.
pub async fn warmup_pageload2(
    run_id: Uuid,
    seq: u32,
    cfg: &PageLoadConfig,
) -> (RequestAttempt, Option<SharedH2Conn>) {
    use crate::metrics::ErrorCategory;
    use crate::runner::socket_info::SocketInfo;

    let cpu_start = cpu_time::ProcessTime::now();
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    let target = &cfg.base_url;
    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Config,
                    "Target URL has no host".into(),
                ),
                None,
            );
        }
    };
    if target.scheme() != "https" {
        return (
            error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                ErrorCategory::Config,
                "pageload2 requires HTTPS".into(),
            ),
            None,
        );
    }
    let port = target.port().unwrap_or(443);
    let run_cfg = &cfg.run_cfg;

    // ── DNS ──
    let (addr, dns_result) = if run_cfg.dns_enabled {
        match dns_runner::resolve(&host, run_cfg.ipv4_only, run_cfg.ipv6_only).await {
            Ok((ips, r)) => {
                let ip = pick_ip(&ips, run_cfg.ipv4_only);
                (std::net::SocketAddr::new(ip, port), Some(r))
            }
            Err(e) => {
                return (
                    error_attempt(attempt_id, run_id, seq, started_at, e.category, e.message),
                    None,
                );
            }
        }
    } else {
        match host.parse::<std::net::IpAddr>() {
            Ok(ip) => (std::net::SocketAddr::new(ip, port), None),
            Err(_) => {
                return (
                    error_attempt(
                        attempt_id,
                        run_id,
                        seq,
                        started_at,
                        ErrorCategory::Config,
                        format!("dns_enabled=false but '{host}' is not a valid IP"),
                    ),
                    None,
                );
            }
        }
    };

    // ── TCP connect ──
    let tcp_started_at = Utc::now();
    let t_tcp = Instant::now();
    let tcp_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(run_cfg.timeout_ms),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Tcp,
                    e.to_string(),
                ),
                None,
            );
        }
        Err(_) => {
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Timeout,
                    format!("TCP timed out after {}ms", run_cfg.timeout_ms),
                ),
                None,
            );
        }
    };
    let tcp_duration_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let _ = tcp_stream.set_nodelay(true);
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let sock_info = SocketInfo::from_stream(&tcp_stream);
    let tcp_result = TcpResult {
        local_addr,
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

    // ── TLS handshake ──
    let tls_started_at = Utc::now();
    let t_tls = Instant::now();
    let tls_config = match build_tls_config(
        &Protocol::Http2,
        run_cfg.insecure,
        run_cfg.ca_bundle.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Tls,
                    e.to_string(),
                ),
                None,
            );
        }
    };
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = match ServerName::try_from(host.clone()) {
        Ok(n) => n,
        Err(e) => {
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Tls,
                    format!("Invalid SNI: {e}"),
                ),
                None,
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
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Tls,
                    e.to_string(),
                ),
                None,
            );
        }
        Err(_) => {
            return (
                error_attempt(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    ErrorCategory::Timeout,
                    format!("TLS timed out after {}ms", run_cfg.timeout_ms),
                ),
                None,
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
            resumed: None,
            handshake_kind: None,
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
        }
    };

    // ── HTTP/2 handshake ──
    let io = TokioIo::new(tls_stream);
    let (sender, conn) =
        match hyper::client::conn::http2::handshake::<_, _, Full<Bytes>>(TokioExecutor::new(), io)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                return (
                    error_attempt(
                        attempt_id,
                        run_id,
                        seq,
                        started_at,
                        ErrorCategory::Http,
                        format!("HTTP/2 handshake: {e}"),
                    ),
                    None,
                );
            }
        };
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("warmup H2 conn error: {e}");
        }
    });

    let shared = SharedH2Conn {
        sender: sender.clone(),
        host: host.clone(),
        addr,
        dns_result: dns_result.clone(),
        tcp_result: tcp_result.clone(),
        tls_result: tls_result.clone(),
        tls_duration_ms,
    };

    // ── Warmup fetch (same as a normal pageload2 fetch) ──
    let warmup = fetch_h2_pageload(
        attempt_id,
        run_id,
        seq,
        started_at,
        t_wall,
        cpu_start,
        cfg,
        sender,
        &host,
        dns_result,
        Some(tcp_result),
        Some(tls_result),
        tls_duration_ms,
        false, // warmup is a cold probe
    )
    .await;

    (warmup, Some(shared))
}

/// Run a page-load probe reusing an existing HTTP/2 connection (warm).
pub async fn run_pageload2_warm(
    run_id: Uuid,
    seq: u32,
    cfg: &PageLoadConfig,
    conn: &SharedH2Conn,
) -> RequestAttempt {
    let cpu_start = cpu_time::ProcessTime::now();
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    fetch_h2_pageload(
        attempt_id,
        run_id,
        seq,
        started_at,
        t_wall,
        cpu_start,
        cfg,
        conn.sender.clone(),
        &conn.host,
        None,
        None,
        None,
        0.0,
        true, // warm = connection reused
    )
    .await
}

/// Common H2 page-load fetch logic shared by warmup and warm probes.
#[allow(clippy::too_many_arguments)]
async fn fetch_h2_pageload(
    attempt_id: Uuid,
    run_id: Uuid,
    seq: u32,
    started_at: chrono::DateTime<Utc>,
    t_wall: Instant,
    cpu_start: cpu_time::ProcessTime,
    cfg: &PageLoadConfig,
    sender: hyper::client::conn::http2::SendRequest<Full<Bytes>>,
    host: &str,
    dns_result: Option<crate::metrics::DnsResult>,
    tcp_result: Option<crate::metrics::TcpResult>,
    tls_result: Option<crate::metrics::TlsResult>,
    tls_duration_ms: f64,
    connection_reused: bool,
) -> RequestAttempt {
    let run_cfg = &cfg.run_cfg;

    // ── Manifest request ──
    let n = cfg.asset_sizes.len();
    let representative_bytes = cfg.asset_sizes.first().copied().unwrap_or(10_240);
    let manifest_path = format!("/page?assets={n}&bytes={representative_bytes}");
    let t_manifest = Instant::now();
    let manifest_req = Request::builder()
        .method("GET")
        .uri(&manifest_path)
        .header("host", host)
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
                crate::metrics::ErrorCategory::Http,
                format!("Manifest: {e}"),
            );
        }
        Err(_) => {
            return error_attempt(
                attempt_id,
                run_id,
                seq,
                started_at,
                crate::metrics::ErrorCategory::Timeout,
                "Manifest timed out".into(),
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
    let manifest_http = crate::metrics::HttpResult {
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
        started_at: manifest_send_at,
        response_headers: manifest_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        payload_bytes: 0,
        throughput_mbps: None,
        goodput_mbps: None,
        cpu_time_ms: None,
        csw_voluntary: None,
        csw_involuntary: None,
    };

    // ── Asset requests ──
    let asset_urls = build_asset_urls(&cfg.base_url, &cfg.asset_sizes);
    let n = asset_urls.len();

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
                .header("host", host)
                .header("user-agent", "networker-tester/0.1")
                .header("accept", "*/*")
                .body(Full::new(Bytes::new()))
                .expect("valid asset request");
            let mut s = sender.clone();
            let timeout_ms = run_cfg.timeout_ms;
            async move {
                let t0 = Instant::now();
                match tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
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
    let tls_overhead_ratio = if total_ms > 0.0 && !connection_reused {
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
        connections_opened: if connection_reused { 0 } else { 1 },
        asset_timings_ms: asset_timings,
        started_at,
        tls_setup_ms: tls_duration_ms,
        tls_overhead_ratio,
        per_connection_tls_ms: if connection_reused {
            vec![]
        } else {
            vec![tls_duration_ms]
        },
        cpu_time_ms,
        connection_reused,
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
        tcp: tcp_result,
        tls: tls_result,
        http: Some(manifest_http),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing,
        udp_throughput: None,
        page_load: Some(page_load),
        browser: None,
        http_stack: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connection-reuse: HTTP/3 (QUIC)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "http3"))]
pub async fn warmup_pageload3(
    run_id: Uuid,
    seq: u32,
    _cfg: &PageLoadConfig,
) -> (RequestAttempt, Option<()>) {
    (
        error_attempt_proto(
            Uuid::new_v4(),
            run_id,
            seq,
            Utc::now(),
            Protocol::PageLoad3,
            crate::metrics::ErrorCategory::Config,
            "HTTP/3 excluded at compile time".into(),
        ),
        None,
    )
}

#[cfg(not(feature = "http3"))]
pub async fn run_pageload3_warm(
    run_id: Uuid,
    seq: u32,
    _cfg: &PageLoadConfig,
    _conn: &tokio::sync::Mutex<()>,
) -> RequestAttempt {
    error_attempt_proto(
        Uuid::new_v4(),
        run_id,
        seq,
        Utc::now(),
        Protocol::PageLoad3,
        crate::metrics::ErrorCategory::Config,
        "HTTP/3 excluded at compile time".into(),
    )
}

#[cfg(feature = "http3")]
pub async fn warmup_pageload3(
    run_id: Uuid,
    seq: u32,
    cfg: &PageLoadConfig,
) -> (RequestAttempt, Option<SharedH3Conn>) {
    use h3_quinn::Connection as QuinnH3Connection;
    use quinn::{ClientConfig as QuinnClientConfig, Endpoint};

    let cpu_start = cpu_time::ProcessTime::now();
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    let target = &cfg.base_url;
    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Config,
                    "No host".into(),
                ),
                None,
            );
        }
    };
    if target.scheme() != "https" {
        return (
            error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                crate::metrics::ErrorCategory::Config,
                "pageload3 requires HTTPS".into(),
            ),
            None,
        );
    }
    let port = target.port().unwrap_or(443);
    let run_cfg = &cfg.run_cfg;

    // ── TLS / QUIC config ──
    let mut tls_cfg = match build_tls_config(
        &Protocol::Http1,
        run_cfg.insecure,
        run_cfg.ca_bundle.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Tls,
                    format!("TLS: {e}"),
                ),
                None,
            );
        }
    };
    tls_cfg.alpn_protocols = vec![b"h3".to_vec()];
    let quinn_tls = QuinnClientConfig::new(Arc::new(
        match quinn::crypto::rustls::QuicClientConfig::try_from(tls_cfg) {
            Ok(c) => c,
            Err(e) => {
                return (
                    error_attempt_proto(
                        attempt_id,
                        run_id,
                        seq,
                        started_at,
                        Protocol::PageLoad3,
                        crate::metrics::ErrorCategory::Tls,
                        format!("QUIC TLS: {e}"),
                    ),
                    None,
                );
            }
        },
    ));

    let mut endpoint = match Endpoint::client("0.0.0.0:0".parse().unwrap()) {
        Ok(e) => e,
        Err(e) => {
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Config,
                    format!("QUIC endpoint: {e}"),
                ),
                None,
            );
        }
    };
    endpoint.set_default_client_config(quinn_tls);

    // ── DNS ──
    let addr_str = format!("{host}:{port}");
    let server_addr: std::net::SocketAddr = match addr_str.parse() {
        Ok(a) => a,
        Err(_) => match tokio::net::lookup_host(&addr_str).await {
            Ok(mut a) => match a.next() {
                Some(sa) => sa,
                None => {
                    return (
                        error_attempt_proto(
                            attempt_id,
                            run_id,
                            seq,
                            started_at,
                            Protocol::PageLoad3,
                            crate::metrics::ErrorCategory::Dns,
                            format!("No addresses for {host}"),
                        ),
                        None,
                    );
                }
            },
            Err(e) => {
                return (
                    error_attempt_proto(
                        attempt_id,
                        run_id,
                        seq,
                        started_at,
                        Protocol::PageLoad3,
                        crate::metrics::ErrorCategory::Dns,
                        format!("DNS: {e}"),
                    ),
                    None,
                );
            }
        },
    };

    // ── QUIC handshake ──
    let t_handshake = Instant::now();
    let connecting = match endpoint.connect(server_addr, &host) {
        Ok(c) => c,
        Err(e) => {
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Tcp,
                    format!("QUIC connect: {e}"),
                ),
                None,
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
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Tcp,
                    format!("QUIC: {e}"),
                ),
                None,
            );
        }
        Err(_) => {
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Timeout,
                    format!("QUIC timed out after {}ms", run_cfg.timeout_ms),
                ),
                None,
            );
        }
    };
    let handshake_ms = t_handshake.elapsed().as_secs_f64() * 1000.0;

    // ── H3 client ──
    let (mut driver, send_req) = match h3::client::new(QuinnH3Connection::new(conn)).await {
        Ok(pair) => pair,
        Err(e) => {
            return (
                error_attempt_proto(
                    attempt_id,
                    run_id,
                    seq,
                    started_at,
                    Protocol::PageLoad3,
                    crate::metrics::ErrorCategory::Http,
                    format!("H3: {e}"),
                ),
                None,
            );
        }
    };
    tokio::spawn(async move {
        let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    // ── Warmup fetch ──
    let shared = SharedH3Conn {
        send_req,
        _endpoint: endpoint,
        host: host.clone(),
        port,
        handshake_ms,
    };
    let mutex = tokio::sync::Mutex::new(shared);
    let warmup = fetch_h3_pageload(
        attempt_id,
        run_id,
        seq,
        started_at,
        t_wall,
        cpu_start,
        cfg,
        &mutex,
        handshake_ms,
        false,
    )
    .await;

    let shared = mutex.into_inner();
    (warmup, Some(shared))
}

/// Run a page-load probe reusing an existing HTTP/3 QUIC connection (warm).
#[cfg(feature = "http3")]
pub async fn run_pageload3_warm(
    run_id: Uuid,
    seq: u32,
    cfg: &PageLoadConfig,
    conn: &tokio::sync::Mutex<SharedH3Conn>,
) -> RequestAttempt {
    let cpu_start = cpu_time::ProcessTime::now();
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();
    let t_wall = Instant::now();

    fetch_h3_pageload(
        attempt_id, run_id, seq, started_at, t_wall, cpu_start, cfg, conn, 0.0, true,
    )
    .await
}

/// Common H3 page-load fetch logic shared by warmup and warm probes.
#[cfg(feature = "http3")]
#[allow(clippy::too_many_arguments)]
async fn fetch_h3_pageload(
    attempt_id: Uuid,
    run_id: Uuid,
    seq: u32,
    started_at: chrono::DateTime<Utc>,
    t_wall: Instant,
    cpu_start: cpu_time::ProcessTime,
    cfg: &PageLoadConfig,
    conn_mutex: &tokio::sync::Mutex<SharedH3Conn>,
    handshake_ms: f64,
    connection_reused: bool,
) -> RequestAttempt {
    use bytes::Buf;

    let run_cfg = &cfg.run_cfg;
    let mut conn = conn_mutex.lock().await;
    let host = conn.host.clone();
    let port = conn.port;

    // ── Manifest request ──
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
    let mut manifest_stream = match conn.send_req.send_request(manifest_req).await {
        Ok(s) => s,
        Err(e) => {
            return error_attempt_proto(
                attempt_id,
                run_id,
                seq,
                started_at,
                Protocol::PageLoad3,
                crate::metrics::ErrorCategory::Http,
                format!("Manifest: {e}"),
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
                crate::metrics::ErrorCategory::Http,
                format!("Manifest recv: {e}"),
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

    let tls_result = crate::metrics::TlsResult {
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
        resumed: None,
        handshake_kind: None,
        tls13_tickets_received: None,
        previous_handshake_duration_ms: None,
        previous_handshake_kind: None,
        previous_http_status_code: None,
        http_status_code: None,
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
        goodput_mbps: None,
        cpu_time_ms: None,
        csw_voluntary: None,
        csw_involuntary: None,
    };

    // ── Asset requests ──
    let asset_urls = build_asset_urls(&cfg.base_url, &cfg.asset_sizes);
    let n = asset_urls.len();
    let dur = std::time::Duration::from_millis(run_cfg.timeout_ms);

    // Phase 1: open N streams
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
        match tokio::time::timeout(dur, conn.send_req.send_request(req)).await {
            Ok(Ok(mut s)) => {
                s.finish().await.ok();
                streams.push(Some(s));
            }
            _ => streams.push(None),
        }
    }

    // Release lock before receiving (concurrent receives don't need send_req)
    drop(conn);

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
    let tls_overhead_ratio = if total_ms > 0.0 && !connection_reused {
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
        connections_opened: if connection_reused { 0 } else { 1 },
        asset_timings_ms: asset_timings,
        started_at,
        tls_setup_ms: handshake_ms,
        tls_overhead_ratio,
        per_connection_tls_ms: if connection_reused {
            vec![]
        } else {
            vec![handshake_ms]
        },
        cpu_time_ms,
        connection_reused,
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
        tls: if connection_reused {
            None
        } else {
            Some(tls_result)
        },
        http: Some(manifest_http),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: Some(page_load),
        browser: None,
        http_stack: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Endpoint test helpers ────────────────────────────────────────────────

    #[cfg(test)]
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[cfg(test)]
    fn free_udp_port() -> u16 {
        std::net::UdpSocket::bind("0.0.0.0:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[cfg(test)]
    fn init_crypto() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[cfg(test)]
    struct TestEndpoint {
        http_port: u16,
        https_port: u16,
        _shutdown: tokio::sync::oneshot::Sender<()>,
    }

    #[cfg(test)]
    impl TestEndpoint {
        async fn start() -> Self {
            init_crypto();
            let http_port = free_port();
            let https_port = free_port();
            let udp_port = free_udp_port();
            let udp_throughput_port = free_udp_port();
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            let cfg = networker_endpoint::ServerConfig {
                http_port,
                https_port,
                udp_port,
                udp_throughput_port,
            };
            tokio::spawn(async move {
                networker_endpoint::run_with_shutdown(cfg, rx).await.ok();
            });
            // Wait for HTTP + HTTPS
            for port in [http_port, https_port] {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                        .await
                        .is_ok()
                    {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Endpoint port {port} did not start"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
            Self {
                http_port,
                https_port,
                _shutdown: tx,
            }
        }

        fn http_url(&self, path: &str) -> url::Url {
            format!("http://127.0.0.1:{}{path}", self.http_port)
                .parse()
                .unwrap()
        }

        fn https_url(&self, path: &str) -> url::Url {
            format!("https://127.0.0.1:{}{path}", self.https_port)
                .parse()
                .unwrap()
        }

        #[cfg(feature = "http3")]
        async fn wait_for_quic(&self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
                sock.connect(format!("127.0.0.1:{}", self.https_port))
                    .await
                    .unwrap();
                let _ = sock.send(&[0u8]).await;
                let mut buf = [0u8; 64];
                match tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    sock.recv(&mut buf),
                )
                .await
                {
                    Err(_timeout) => break, // no ICMP unreachable → Quinn is listening
                    Ok(Ok(_)) => break,     // Quinn sent data back → ready
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        // UDP port not bound yet — retry
                    }
                    Ok(Err(_)) => break, // unexpected; let probe handle it
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "QUIC server did not start"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }

    fn default_run_cfg() -> RunConfig {
        RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: true,
            ..Default::default()
        }
    }

    // ── Integration: run_pageload_probe (H1) ────────────────────────────────

    #[tokio::test]
    async fn pageload_h1_success() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                insecure: false,
                ..default_run_cfg()
            },
            base_url: ep.http_url("/health"),
            asset_sizes: vec![1024; 5],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 1, &cfg).await;
        assert!(a.success, "H1 failed: {:?}", a.error);
        assert_eq!(a.protocol, Protocol::PageLoad);
        let pl = a.page_load.unwrap();
        assert_eq!(pl.asset_count, 5);
        assert_eq!(pl.assets_fetched, 5);
        assert!(pl.total_bytes > 0);
        assert!(pl.total_ms > 0.0);
        assert!(pl.connections_opened >= 1);
        assert!(!pl.connection_reused);
    }

    #[tokio::test]
    /// pageload1 rewrites HTTPS URLs to plain HTTP (matching browser1 behavior).
    /// When given an HTTPS URL with the 8443 convention, it connects via HTTP:8080.
    async fn pageload_h1_rewrites_https_to_http() {
        let ep = TestEndpoint::start().await;
        // Use http_url directly — pageload1 always downgrades to HTTP.
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.http_url("/health"),
            asset_sizes: vec![512; 3],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(a.success, "H1 plain HTTP failed: {:?}", a.error);
        // No TLS when using plain HTTP
        assert!(a.tls.is_none());
        assert!(a.tcp.is_some());
        let pl = a.page_load.unwrap();
        assert_eq!(pl.tls_setup_ms, 0.0);
    }

    #[tokio::test]
    async fn pageload_h1_no_host_url() {
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: "data:text/html,hello".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad);
        let err = a.error.unwrap();
        assert_eq!(err.category, ErrorCategory::Config);
        assert!(err.message.contains("no host"));
    }

    #[tokio::test]
    async fn pageload_h1_dns_disabled_hostname_fails() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: false,
                ..default_run_cfg()
            },
            base_url: "http://example.com:9999/health".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        let err = a.error.unwrap();
        assert_eq!(err.category, ErrorCategory::Config);
        assert!(err.message.contains("dns_enabled=false"));
    }

    #[tokio::test]
    async fn pageload_h1_connection_refused() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                timeout_ms: 1000,
                ..default_run_cfg()
            },
            base_url: "http://127.0.0.1:1/health".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad);
    }

    #[tokio::test]
    async fn pageload_h1_empty_assets() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                insecure: false,
                ..default_run_cfg()
            },
            base_url: ep.http_url("/health"),
            asset_sizes: vec![],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        // 0 assets → success = (0 == 0) → depends on manifest fetch
        assert_eq!(a.protocol, Protocol::PageLoad);
        let pl = a.page_load.unwrap();
        assert_eq!(pl.asset_count, 0);
    }

    #[tokio::test]
    async fn pageload_h1_many_assets_uses_multiple_conns() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                insecure: false,
                ..default_run_cfg()
            },
            base_url: ep.http_url("/health"),
            asset_sizes: vec![512; 12], // 12 assets → should use up to 6 connections
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(a.success, "H1 12 assets failed: {:?}", a.error);
        let pl = a.page_load.unwrap();
        assert_eq!(pl.connections_opened, 6);
        assert_eq!(pl.assets_fetched, 12);
    }

    // ── Integration: run_pageload2_probe (H2) ───────────────────────────────

    #[tokio::test]
    async fn pageload_h2_success() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.https_url("/health"),
            asset_sizes: vec![1024; 5],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(a.success, "H2 failed: {:?}", a.error);
        assert_eq!(a.protocol, Protocol::PageLoad2);
        assert!(a.dns.is_none()); // dns_enabled=false
        assert!(a.tcp.is_some());
        assert!(a.tls.is_some());
        assert!(a.http.is_some());
        let pl = a.page_load.unwrap();
        assert_eq!(pl.asset_count, 5);
        assert_eq!(pl.assets_fetched, 5);
        assert_eq!(pl.connections_opened, 1);
        assert!(pl.tls_setup_ms > 0.0);
        assert!(pl.tls_overhead_ratio > 0.0);
        assert!(!pl.connection_reused);
    }

    #[tokio::test]
    async fn pageload_h2_requires_https() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.http_url("/health"), // HTTP, not HTTPS
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad2);
        let err = a.error.unwrap();
        assert_eq!(err.category, ErrorCategory::Config);
        assert!(err.message.contains("HTTPS"));
    }

    #[tokio::test]
    async fn pageload_h2_no_host_url() {
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: "data:text/html,hello".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        let err = a.error.unwrap();
        assert_eq!(err.category, ErrorCategory::Config);
    }

    #[tokio::test]
    async fn pageload_h2_dns_disabled_hostname_fails() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: false,
                ..default_run_cfg()
            },
            base_url: "https://example.com:9999/health".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        let err = a.error.unwrap();
        assert!(err.message.contains("dns_enabled=false"));
    }

    #[tokio::test]
    async fn pageload_h2_connection_refused() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                timeout_ms: 1000,
                ..default_run_cfg()
            },
            base_url: "https://127.0.0.1:1/health".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad2);
    }

    #[tokio::test]
    async fn pageload_h2_empty_assets() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.https_url("/health"),
            asset_sizes: vec![],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert_eq!(a.protocol, Protocol::PageLoad2);
        let pl = a.page_load.unwrap();
        assert_eq!(pl.asset_count, 0);
        assert_eq!(pl.assets_fetched, 0);
    }

    // ── Integration: run_pageload3_probe (H3) ───────────────────────────────

    #[cfg(all(feature = "http3", not(target_os = "windows")))]
    #[tokio::test]
    async fn pageload_h3_success() {
        let ep = TestEndpoint::start().await;
        ep.wait_for_quic().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.https_url("/health"),
            asset_sizes: vec![1024; 5],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(a.success, "H3 failed: {:?}", a.error);
        assert_eq!(a.protocol, Protocol::PageLoad3);
        let pl = a.page_load.unwrap();
        assert_eq!(pl.asset_count, 5);
        assert_eq!(pl.assets_fetched, 5);
        assert_eq!(pl.connections_opened, 1);
        assert!(!pl.connection_reused);
    }

    #[cfg(feature = "http3")]
    #[tokio::test]
    async fn pageload_h3_requires_https() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.http_url("/health"),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad3);
        let err = a.error.unwrap();
        assert!(err.message.contains("HTTPS"));
    }

    #[cfg(feature = "http3")]
    #[tokio::test]
    async fn pageload_h3_no_host_url() {
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: "data:text/html,hello".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad3);
    }

    // ── Integration: warmup_pageload2 + run_pageload2_warm ──────────────────

    #[tokio::test]
    async fn warmup_and_warm_h2() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.https_url("/health"),
            asset_sizes: vec![512; 3],
            preset_name: None,
        };
        let (warmup, shared) = warmup_pageload2(Uuid::new_v4(), 0, &cfg).await;
        assert!(warmup.success, "warmup failed: {:?}", warmup.error);
        assert_eq!(warmup.protocol, Protocol::PageLoad2);
        let wpl = warmup.page_load.unwrap();
        assert!(!wpl.connection_reused); // warmup is cold
        assert!(wpl.tls_setup_ms > 0.0);

        let shared = shared.expect("shared conn should be Some");
        let warm = run_pageload2_warm(Uuid::new_v4(), 1, &cfg, &shared).await;
        assert!(warm.success, "warm failed: {:?}", warm.error);
        assert_eq!(warm.protocol, Protocol::PageLoad2);
        let pl = warm.page_load.unwrap();
        assert!(pl.connection_reused);
        assert_eq!(pl.assets_fetched, 3);
        // Warm probe should skip DNS/TCP/TLS
        assert!(warm.dns.is_none());
        assert!(warm.tcp.is_none());
        assert!(warm.tls.is_none());
    }

    // ── Integration: warmup_pageload3 + run_pageload3_warm ──────────────────

    #[cfg(all(feature = "http3", not(target_os = "windows")))]
    #[tokio::test]
    async fn warmup_and_warm_h3() {
        let ep = TestEndpoint::start().await;
        ep.wait_for_quic().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.https_url("/health"),
            asset_sizes: vec![512; 3],
            preset_name: None,
        };
        let (warmup, shared) = warmup_pageload3(Uuid::new_v4(), 0, &cfg).await;
        assert!(warmup.success, "H3 warmup failed: {:?}", warmup.error);
        assert_eq!(warmup.protocol, Protocol::PageLoad3);
        let wpl = warmup.page_load.unwrap();
        assert!(!wpl.connection_reused);

        let shared = shared.expect("shared H3 conn should be Some");
        let shared_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(shared));
        let warm = run_pageload3_warm(Uuid::new_v4(), 1, &cfg, &shared_mutex).await;
        assert!(warm.success, "H3 warm failed: {:?}", warm.error);
        let pl = warm.page_load.unwrap();
        assert!(pl.connection_reused);
        assert_eq!(pl.assets_fetched, 3);
    }

    // ── Integration: DNS-enabled paths ─────────────────────────────────────

    #[tokio::test]
    async fn pageload_h1_with_dns_enabled() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: true,
                insecure: false,
                timeout_ms: 10_000,
                ..Default::default()
            },
            base_url: format!("http://localhost:{}/health", ep.http_port)
                .parse()
                .unwrap(),
            asset_sizes: vec![512; 2],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(a.success, "H1+DNS failed: {:?}", a.error);
        assert!(a.dns.is_some());
    }

    #[tokio::test]
    async fn pageload_h1_dns_resolution_failure() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: true,
                timeout_ms: 5_000,
                ..Default::default()
            },
            base_url: "http://this-host-does-not-exist-xyz.invalid:9999/health"
                .parse()
                .unwrap(),
            asset_sizes: vec![512],
            preset_name: None,
        };
        let a = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad);
        let err = a.error.unwrap();
        assert_eq!(err.category, ErrorCategory::Dns);
    }

    #[tokio::test]
    async fn pageload_h2_with_dns_enabled() {
        let ep = TestEndpoint::start().await;
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: true,
                insecure: true,
                timeout_ms: 10_000,
                ..Default::default()
            },
            base_url: format!("https://localhost:{}/health", ep.https_port)
                .parse()
                .unwrap(),
            asset_sizes: vec![512; 2],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(a.success, "H2+DNS failed: {:?}", a.error);
        assert!(a.dns.is_some());
    }

    #[tokio::test]
    async fn pageload_h2_dns_resolution_failure() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: true,
                timeout_ms: 5_000,
                insecure: true,
                ..Default::default()
            },
            base_url: "https://this-host-does-not-exist-xyz.invalid:9999/health"
                .parse()
                .unwrap(),
            asset_sizes: vec![512],
            preset_name: None,
        };
        let a = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad2);
        let err = a.error.unwrap();
        assert_eq!(err.category, ErrorCategory::Dns);
    }

    #[cfg(feature = "http3")]
    #[tokio::test]
    async fn pageload_h3_dns_resolution_failure() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                dns_enabled: true,
                timeout_ms: 5_000,
                insecure: true,
                ..Default::default()
            },
            base_url: "https://this-host-does-not-exist-xyz.invalid:9999/health"
                .parse()
                .unwrap(),
            asset_sizes: vec![512],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad3);
    }

    #[cfg(feature = "http3")]
    #[tokio::test]
    async fn pageload_h3_unresolvable_host() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                timeout_ms: 5_000,
                ..default_run_cfg()
            },
            base_url: "https://this-host-does-not-exist-xyz.invalid:9999/health"
                .parse()
                .unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad3);
    }

    #[cfg(feature = "http3")]
    #[tokio::test]
    async fn pageload_h3_connection_refused() {
        let cfg = PageLoadConfig {
            run_cfg: RunConfig {
                timeout_ms: 2000,
                ..default_run_cfg()
            },
            base_url: "https://127.0.0.1:1/health".parse().unwrap(),
            asset_sizes: vec![1024],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad3);
    }

    #[cfg(all(feature = "http3", not(target_os = "windows")))]
    #[tokio::test]
    async fn pageload_h3_empty_assets() {
        let ep = TestEndpoint::start().await;
        ep.wait_for_quic().await;
        let cfg = PageLoadConfig {
            run_cfg: default_run_cfg(),
            base_url: ep.https_url("/health"),
            asset_sizes: vec![],
            preset_name: None,
        };
        let a = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;
        assert_eq!(a.protocol, Protocol::PageLoad3);
        let pl = a.page_load.unwrap();
        assert_eq!(pl.asset_count, 0);
    }

    // ── Preset tests ────────────────────────────────────────────────────────

    #[test]
    fn resolve_preset_default() {
        let sizes = resolve_preset("default").unwrap();
        assert_eq!(sizes.len(), 50);
        let total: usize = sizes.iter().sum();
        // ~6 MB total
        assert!(
            total > 5_000_000 && total < 7_000_000,
            "default total={total}"
        );
    }

    #[test]
    fn resolve_preset_tiny() {
        let sizes = resolve_preset("tiny").unwrap();
        assert_eq!(sizes.len(), 10);
        let total: usize = sizes.iter().sum();
        // ~100 KB total
        assert!(total > 80_000 && total < 120_000, "tiny total={total}");
    }

    #[test]
    fn resolve_preset_small() {
        let sizes = resolve_preset("small").unwrap();
        assert_eq!(sizes.len(), 25);
        let total: usize = sizes.iter().sum();
        // ~900 KB total
        assert!(total > 800_000 && total < 1_000_000, "small total={total}");
    }

    #[test]
    fn resolve_preset_medium() {
        let sizes = resolve_preset("medium").unwrap();
        assert_eq!(sizes.len(), 100);
        let total: usize = sizes.iter().sum();
        // ~10 MB total
        assert!(
            total > 9_000_000 && total < 12_000_000,
            "medium total={total}"
        );
    }

    #[test]
    fn resolve_preset_large() {
        let sizes = resolve_preset("large").unwrap();
        assert_eq!(sizes.len(), 200);
        let total: usize = sizes.iter().sum();
        // ~31 MB total
        assert!(
            total > 28_000_000 && total < 35_000_000,
            "large total={total}"
        );
    }

    #[test]
    fn resolve_preset_mixed_has_50_assets() {
        let sizes = resolve_preset("mixed").unwrap();
        assert_eq!(sizes.len(), 50);
    }

    #[test]
    fn resolve_preset_mixed_total_size() {
        let sizes = resolve_preset("mixed").unwrap();
        let total: usize = sizes.iter().sum();
        // ~7 MB total
        assert!(
            total > 5_500_000 && total < 8_000_000,
            "mixed total={total}"
        );
    }

    #[test]
    fn resolve_preset_case_insensitive() {
        assert!(resolve_preset("TINY").is_ok());
        assert!(resolve_preset("Default").is_ok());
        assert!(resolve_preset("MIXED").is_ok());
    }

    #[test]
    fn resolve_preset_unknown_returns_err() {
        let err = resolve_preset("bogus").unwrap_err();
        assert!(
            err.to_string().contains("bogus"),
            "error should name the bad preset"
        );
    }

    #[test]
    fn resolve_preset_unknown_lists_valid_names() {
        let err = resolve_preset("oops").unwrap_err();
        let msg = err.to_string();
        for name in &["tiny", "small", "default", "medium", "large", "mixed"] {
            assert!(msg.contains(name), "error should list '{name}' as valid");
        }
    }

    // ── rewrite_to_http ────────────────────────────────────────────────────

    #[test]
    fn rewrite_to_http_8443_becomes_8080() {
        let url: url::Url = "https://10.0.0.1:8443/health".parse().unwrap();
        let r = rewrite_to_http(&url);
        assert_eq!(r.scheme(), "http");
        assert_eq!(r.port(), Some(8080));
        assert_eq!(r.path(), "/health");
    }

    #[test]
    fn rewrite_to_http_8444_becomes_8081() {
        let url: url::Url = "https://10.0.0.1:8444/health".parse().unwrap();
        let r = rewrite_to_http(&url);
        assert_eq!(r.scheme(), "http");
        assert_eq!(r.port(), Some(8081));
    }

    #[test]
    fn rewrite_to_http_8445_becomes_8082() {
        let url: url::Url = "https://10.0.0.1:8445/health".parse().unwrap();
        let r = rewrite_to_http(&url);
        assert_eq!(r.scheme(), "http");
        assert_eq!(r.port(), Some(8082));
    }

    #[test]
    fn rewrite_to_http_443_becomes_default_80() {
        let url: url::Url = "https://example.com/health".parse().unwrap();
        let r = rewrite_to_http(&url);
        assert_eq!(r.scheme(), "http");
        assert_eq!(r.port(), None); // default 80, omitted
    }

    #[test]
    fn rewrite_to_http_noop_for_http_url() {
        let url: url::Url = "http://10.0.0.1:8080/health".parse().unwrap();
        let r = rewrite_to_http(&url);
        assert_eq!(r.as_str(), url.as_str());
    }

    #[test]
    fn rewrite_to_http_custom_port_kept() {
        let url: url::Url = "https://10.0.0.1:9999/health".parse().unwrap();
        let r = rewrite_to_http(&url);
        assert_eq!(r.scheme(), "http");
        assert_eq!(r.port(), Some(9999));
    }

    // ── build_asset_urls ──────────────────────────────────────────────────────

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

    #[test]
    fn build_asset_urls_empty_returns_empty() {
        let base: url::Url = "http://localhost:8080/health".parse().unwrap();
        let urls = build_asset_urls(&base, &[]);
        assert!(urls.is_empty());
    }

    #[test]
    fn build_asset_urls_single_asset() {
        let base: url::Url = "http://localhost:8080/".parse().unwrap();
        let urls = build_asset_urls(&base, &[512]);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].path(), "/asset");
        let q = urls[0].query().unwrap();
        assert!(q.contains("id=0"));
        assert!(q.contains("bytes=512"));
    }

    #[test]
    fn build_asset_urls_path_is_always_asset() {
        let base: url::Url = "http://localhost:8080/health".parse().unwrap();
        let urls = build_asset_urls(&base, &[100, 200]);
        assert!(urls.iter().all(|u| u.path() == "/asset"));
    }

    #[test]
    fn build_asset_urls_ids_are_sequential() {
        let base: url::Url = "http://localhost:8080/".parse().unwrap();
        let urls = build_asset_urls(&base, &[1, 2, 3]);
        for (i, url) in urls.iter().enumerate() {
            assert!(url.query().unwrap().contains(&format!("id={i}")));
        }
    }

    // ── pick_ip ───────────────────────────────────────────────────────────────

    #[test]
    fn pick_ip_first_when_not_ipv4_only() {
        use std::net::IpAddr;
        let ips: Vec<IpAddr> = vec!["192.168.1.1".parse().unwrap(), "::1".parse().unwrap()];
        assert_eq!(pick_ip(&ips, false), ips[0]);
    }

    #[test]
    fn pick_ip_prefers_ipv4_when_ipv4_only() {
        use std::net::IpAddr;
        let ips: Vec<IpAddr> = vec!["::1".parse().unwrap(), "10.0.0.1".parse().unwrap()];
        let picked = pick_ip(&ips, true);
        assert!(picked.is_ipv4());
        assert_eq!(picked, "10.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn pick_ip_falls_back_to_first_when_no_ipv4() {
        use std::net::IpAddr;
        let ips: Vec<IpAddr> = vec!["::1".parse().unwrap(), "::2".parse().unwrap()];
        // ipv4_only=true but no IPv4 present — falls back to ips[0]
        assert_eq!(pick_ip(&ips, true), ips[0]);
    }

    // ── error_attempt_proto ───────────────────────────────────────────────────

    #[test]
    fn error_attempt_proto_sets_correct_fields() {
        let run_id = uuid::Uuid::new_v4();
        let attempt_id = uuid::Uuid::new_v4();
        let a = error_attempt_proto(
            attempt_id,
            run_id,
            3,
            chrono::Utc::now(),
            Protocol::PageLoad3,
            ErrorCategory::Timeout,
            "timed out".into(),
        );
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::PageLoad3);
        assert_eq!(a.sequence_num, 3);
        let err = a.error.unwrap();
        assert_eq!(err.message, "timed out");
        assert_eq!(err.category, ErrorCategory::Timeout);
    }

    #[test]
    fn error_attempt_defaults_to_pageload2_protocol() {
        let run_id = uuid::Uuid::new_v4();
        let a = error_attempt(
            uuid::Uuid::new_v4(),
            run_id,
            0,
            chrono::Utc::now(),
            ErrorCategory::Config,
            "config error".into(),
        );
        assert_eq!(a.protocol, Protocol::PageLoad2);
        assert!(!a.success);
    }

    // ── parse_server_timing_simple ────────────────────────────────────────────

    #[test]
    fn parse_server_timing_simple_no_headers_returns_none() {
        let headers = hyper::HeaderMap::new();
        let result = parse_server_timing_simple(&headers, chrono::Utc::now(), 10.0);
        assert!(result.is_none());
    }

    #[test]
    fn parse_server_timing_simple_version_header_returns_some() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("x-networker-server-version", "0.12.0".parse().unwrap());
        let result = parse_server_timing_simple(&headers, chrono::Utc::now(), 10.0);
        let st = result.expect("should be Some when version header present");
        assert_eq!(st.server_version.as_deref(), Some("0.12.0"));
        // No timestamp header → no clock skew
        assert!(st.clock_skew_ms.is_none());
        assert!(st.server_timestamp.is_none());
    }

    #[test]
    fn parse_server_timing_simple_clock_skew_formula() {
        // clock_skew_ms = (server_ts - client_send_at) - ttfb_ms / 2
        let client_send_at = chrono::Utc::now();
        // Server timestamp is 100ms ahead of client
        let server_ts = client_send_at + chrono::Duration::milliseconds(100);
        let ttfb_ms = 40.0;

        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "x-networker-server-timestamp",
            server_ts.to_rfc3339().parse().unwrap(),
        );

        let result = parse_server_timing_simple(&headers, client_send_at, ttfb_ms);
        let st = result.expect("should be Some with timestamp header");
        let skew = st.clock_skew_ms.expect("clock_skew_ms should be set");
        // Expected: 100ms - 40/2 = 80ms (with some tolerance for execution time)
        assert!(
            (skew - 80.0).abs() < 5.0,
            "clock skew was {skew:.3}ms, expected ~80ms"
        );
    }

    #[test]
    fn parse_server_timing_simple_invalid_timestamp_gives_none_skew() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "x-networker-server-timestamp",
            "not-a-valid-rfc3339-date".parse().unwrap(),
        );
        let result = parse_server_timing_simple(&headers, chrono::Utc::now(), 10.0);
        let st = result.expect("should be Some (has the timestamp header key)");
        // Invalid format → server_timestamp = None → clock_skew_ms = None
        assert!(st.server_timestamp.is_none());
        assert!(st.clock_skew_ms.is_none());
    }

    #[test]
    fn parse_server_timing_simple_only_server_timing_header() {
        // server-timing header alone (no x-networker-*) should also yield Some
        let mut headers = hyper::HeaderMap::new();
        headers.insert("server-timing", "recv;dur=5.0".parse().unwrap());
        let result = parse_server_timing_simple(&headers, chrono::Utc::now(), 10.0);
        // parse_server_timing_simple doesn't parse server-timing values, but it does
        // return Some because the header is present
        assert!(
            result.is_some(),
            "should be Some when server-timing header present"
        );
    }
}
