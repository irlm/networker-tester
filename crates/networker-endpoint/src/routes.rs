/// All HTTP route handlers for the diagnostics endpoint.
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Version},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use chrono::Utc;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use http_body_util::BodyExt;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write as IoWrite;
use std::sync::OnceLock;
use std::time::Instant;
use tokio::time::{sleep, Duration};
use tower_http::trace::TraceLayer;

// ─────────────────────────────────────────────────────────────────────────────
// Application state
// ─────────────────────────────────────────────────────────────────────────────

/// Shared state threaded through the Axum router and middleware.
#[derive(Debug, Clone)]
pub struct AppState {
    pub h3_port: Option<u16>,
    pub http_port: u16,
    pub https_port: u16,
    pub udp_port: u16,
    pub udp_throughput_port: u16,
    /// STAMP Session-Reflector port (RFC 8762).
    pub stamp_port: u16,
    pub started_at: Instant,
    pub system_meta: SystemMeta,
    /// Bearer token enforced by the auth middleware when set. Carried in
    /// state (production fills it from `BENCH_API_TOKEN`) instead of a
    /// process-global OnceLock so the with-token middleware branches are
    /// testable — the env-global version left all 7 of its mutants alive
    /// (tests can't safely vary a cached env read under a parallel runner).
    pub bench_token: Option<String>,
}

/// Non-sensitive system metadata exposed via GET /info.
#[derive(Debug, Clone, Serialize)]
pub struct SystemMeta {
    pub os: String,
    pub arch: String,
    pub cpu_cores: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_memory_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    pub hostname: String,
    /// Cloud region (auto-detected from cloud metadata at startup).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Public DNS hostname (auto-detected from cloud metadata).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_dns: Option<String>,
    /// Public IP address (auto-detected from cloud metadata).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
}

impl SystemMeta {
    pub fn collect() -> Self {
        let region = detect_cloud_region();
        let public_dns = detect_public_dns(&region);
        let public_ip = detect_public_ip(&region);
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            total_memory_mb: detect_total_memory_mb(),
            os_version: detect_os_version(),
            hostname: get_hostname(),
            region,
            public_dns,
            public_ip,
        }
    }
}

/// Detect public DNS hostname from cloud metadata.
fn detect_public_dns(region: &Option<String>) -> Option<String> {
    let region_str = region.as_deref().unwrap_or("");

    if region_str.starts_with("azure/") {
        // Azure: first try IMDS fqdnName endpoint
        if let Some(fqdn) = cloud_metadata_get_raw(
            "169.254.169.254:80",
            "169.254.169.254",
            "/metadata/instance/compute/fqdnName?api-version=2021-02-01&format=text",
            &[("Metadata", "true")],
        ) {
            if !fqdn.is_empty() {
                return Some(fqdn);
            }
        }
        // Fallback: hostname + region + cloudapp.azure.com
        let hostname = get_hostname();
        let azure_region = region_str.strip_prefix("azure/").unwrap_or("eastus");
        return Some(format!("{hostname}.{azure_region}.cloudapp.azure.com"));
    }

    if region_str.starts_with("aws/") {
        // AWS: http://169.254.169.254/latest/meta-data/public-hostname
        if let Some(dns) = cloud_metadata_get_raw(
            "169.254.169.254:80",
            "169.254.169.254",
            "/latest/meta-data/public-hostname",
            &[],
        ) {
            if !dns.is_empty() && !dns.contains(".internal") {
                return Some(dns);
            }
        }
        // Fallback: construct from public IP
        if let Some(ip) = cloud_metadata_get_raw(
            "169.254.169.254:80",
            "169.254.169.254",
            "/latest/meta-data/public-ipv4",
            &[],
        ) {
            let aws_region = region_str.strip_prefix("aws/").unwrap_or("us-east-1");
            let ip_dashed = ip.replace('.', "-");
            if aws_region == "us-east-1" {
                return Some(format!("ec2-{ip_dashed}.compute-1.amazonaws.com"));
            } else {
                return Some(format!(
                    "ec2-{ip_dashed}.{aws_region}.compute.amazonaws.com"
                ));
            }
        }
    }

    if region_str.starts_with("gcp/") {
        // GCP: hostname is typically the instance name
        let hostname = get_hostname();
        return Some(hostname);
    }

    None
}

/// Detect public IP address from cloud metadata.
fn detect_public_ip(region: &Option<String>) -> Option<String> {
    let region_str = region.as_deref().unwrap_or("");

    if region_str.starts_with("aws/") {
        // AWS: http://169.254.169.254/latest/meta-data/public-ipv4
        return cloud_metadata_get_raw(
            "169.254.169.254:80",
            "169.254.169.254",
            "/latest/meta-data/public-ipv4",
            &[],
        );
    }

    if region_str.starts_with("azure/") {
        // Azure: IMDS public IP
        return cloud_metadata_get_raw(
            "169.254.169.254:80",
            "169.254.169.254",
            "/metadata/instance/network/interface/0/ipv4/ipAddress/0/publicIpAddress?api-version=2021-02-01&format=text",
            &[("Metadata", "true")],
        );
    }

    if region_str.starts_with("gcp/") {
        // GCP: external IP from metadata
        return cloud_metadata_get_raw(
            "169.254.169.254:80",
            "metadata.google.internal",
            "/computeMetadata/v1/instance/network-interfaces/0/access-configs/0/external-ip",
            &[("Metadata-Flavor", "Google")],
        );
    }

    None
}

/// Attempt to detect cloud region from instance metadata APIs.
/// Tries Azure, then AWS, then GCP. Returns None if not on a cloud instance.
fn detect_cloud_region() -> Option<String> {
    // Azure: http://169.254.169.254/metadata/instance/compute/location
    if let Some(r) = cloud_metadata_get_raw(
        "169.254.169.254:80",
        "169.254.169.254",
        "/metadata/instance/compute/location?api-version=2021-02-01&format=text",
        &[("Metadata", "true")],
    ) {
        return Some(format!("azure/{r}"));
    }
    // AWS: http://169.254.169.254/latest/meta-data/placement/region
    if let Some(r) = cloud_metadata_get_raw(
        "169.254.169.254:80",
        "169.254.169.254",
        "/latest/meta-data/placement/region",
        &[],
    ) {
        return Some(format!("aws/{r}"));
    }
    // GCP: http://metadata.google.internal/computeMetadata/v1/instance/zone
    // Use the well-known IP (169.254.169.254) since DNS for metadata.google.internal
    // may not resolve outside GCE.
    if let Some(zone) = cloud_metadata_get_raw(
        "169.254.169.254:80",
        "metadata.google.internal",
        "/computeMetadata/v1/instance/zone",
        &[("Metadata-Flavor", "Google")],
    ) {
        // zone is "projects/123/zones/us-central1-a" — extract zone name
        let z = zone.rsplit('/').next().unwrap_or(&zone);
        // Derive region: strip trailing -[a-z]
        let region = z.rsplitn(2, '-').last().unwrap_or(z);
        return Some(format!("gcp/{region} ({z})"));
    }
    None
}

/// Blocking HTTP GET to a metadata endpoint with a short timeout.
/// Called once at startup — blocking is acceptable.
///
/// `host_port` = "169.254.169.254:80" or "metadata.google.internal:80"
/// `path_query` = "/metadata/instance/compute/location?api-version=..."
fn cloud_metadata_get_raw(
    host_port: &str,
    host_header: &str,
    path_query: &str,
    headers: &[(&str, &str)],
) -> Option<String> {
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Duration;

    let mut req =
        format!("GET {path_query} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");

    let mut stream =
        TcpStream::connect_timeout(&host_port.parse().ok()?, Duration::from_millis(500)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(1000)))
        .ok()?;
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;

    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok();

    let first_line = resp.lines().next()?;
    if !first_line.contains("200") {
        return None;
    }
    let body = resp.split("\r\n\r\n").nth(1)?.trim().to_string();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Parse a `/proc/meminfo`-style buffer for `key` ("MemTotal:" /
/// "MemAvailable:") and convert its kB value to MiB. Extracted from the
/// per-OS detect shells so the parsing arithmetic is unit-testable (the
/// shells themselves read live system state and are excluded from mutation).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // linux shells are the non-test callers
fn parse_meminfo_mb(meminfo: &str, key: &str) -> Option<u64> {
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

fn detect_total_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        parse_meminfo_mb(&std::fs::read_to_string("/proc/meminfo").ok()?, "MemTotal:")
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(bytes / (1024 * 1024))
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("wmic")
            .args(["computersystem", "get", "TotalPhysicalMemory", "/value"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(val) = line.strip_prefix("TotalPhysicalMemory=") {
                let bytes: u64 = val.trim().parse().ok()?;
                return Some(bytes / (1024 * 1024));
            }
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Parse the leading 1-minute load average from a `/proc/loadavg`-style line
/// ("0.52 0.58 0.59 1/467 1234") or a macOS `sysctl -n vm.loadavg` line
/// ("{ 1.86 1.99 2.06 }"). Mirrors the tester's LoadSample parser.
fn parse_load_avg_1m(text: &str) -> Option<f64> {
    text.split_whitespace()
        .find(|tok| !tok.starts_with('{'))
        .and_then(|tok| tok.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
}

/// Live 1-minute load average, sampled per /info request. Same per-platform
/// honesty as the tester's LoadSample: Linux + macOS report it, Windows/other
/// have no cheap equivalent and stay `None` (field omitted from the JSON).
fn detect_load_avg_1m() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        parse_load_avg_1m(&std::fs::read_to_string("/proc/loadavg").ok()?)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()?;
        parse_load_avg_1m(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Live available (reclaimable) memory in MB, sampled per /info request.
/// Linux only (`MemAvailable` in /proc/meminfo); macOS/Windows have no cheap
/// equivalent so the field is omitted — honesty over fabrication.
fn detect_mem_available_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        parse_meminfo_mb(
            &std::fs::read_to_string("/proc/meminfo").ok()?,
            "MemAvailable:",
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn detect_os_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let release = std::fs::read_to_string("/etc/os-release").ok()?;
        for line in release.lines() {
            if let Some(val) = line.strip_prefix("PRETTY_NAME=") {
                return Some(val.trim_matches('"').to_string());
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if ver.is_empty() {
            None
        } else {
            Some(format!("macOS {ver}"))
        }
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .ok()?;
        let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if ver.is_empty() {
            None
        } else {
            Some(ver)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────────

/// Elapsed wall time in milliseconds — the unit every `Server-Timing`
/// `dur=` value is specified in. Extracted from 12 inline copies so the
/// seconds→ms conversion is pinned ONCE (each copy carried two live
/// mutants that would silently report durations 1000× off).
fn elapsed_ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

/// Build the router.
///
/// `state.h3_port` — when `Some(port)`, every response includes
/// `Alt-Svc: h3=":port"; ma=86400` so that Chrome can discover H3 support
/// and upgrade to QUIC on subsequent navigations.  Pass `None` when H3 is not
/// compiled in (the `http3` feature is disabled).
pub fn build_router(state: AppState) -> Router {
    // Eagerly resolve the shared benchmark dataset so a misconfigured
    // BENCH_DATA_PATH (or a corrupt on-disk dataset) is fatal at startup
    // instead of surfacing as silently different benchmark data at first
    // request (API-SPEC.md §2).
    load_bench_data();

    Router::new()
        .route("/", get(landing_page))
        .route("/health", get(health))
        .route("/echo", post(echo).get(echo_get))
        .route("/ws", get(ws_echo))
        .route("/download", get(download))
        .route("/download/{size}", get(download_path))
        .route("/upload", post(upload))
        .route("/delay", get(delay))
        .route("/headers", get(headers_echo))
        .route("/status/{code}", get(status_code))
        .route("/http-version", get(http_version))
        .route("/info", get(server_info))
        .route("/page", get(page_manifest))
        .route("/browser-page", get(browser_page))
        .route("/asset", get(asset_handler))
        // ── JSON API benchmark endpoints ──
        .route("/api/users", get(api_users))
        .route("/api/transform", post(api_transform))
        .route("/api/aggregate", get(api_aggregate))
        .route("/api/search", get(api_search))
        .route("/api/upload/process", post(api_upload_process))
        .route("/api/delayed", get(api_delayed))
        .route("/api/validate", get(api_validate))
        // Provide AppState to all handlers (converts Router<AppState> -> Router<()>).
        .with_state(state.clone())
        // Allow upload probes up to 2 GiB (matching the download cap) while
        // preventing unbounded memory consumption from malicious payloads.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024 * 1024))
        // Bearer token auth (state.bench_token; /health exempt).
        .layer(middleware::from_fn_with_state(
            state.clone(),
            bench_auth_middleware,
        ))
        // Add X-Networker-Server-Timestamp (and optionally Alt-Svc) to every response.
        .layer(middleware::from_fn_with_state(state, add_server_timestamp))
        // Log every request (method + URI) and response (status + latency).
        // Verbosity is controlled by RUST_LOG; defaults to INFO.
        .layer(TraceLayer::new_for_http())
}

// ─────────────────────────────────────────────────────────────────────────────
// Middleware
// ─────────────────────────────────────────────────────────────────────────────

/// Middleware that stamps every response with the server wall-clock time, version,
/// and (when `h3_port` is set) an `Alt-Svc` header advertising HTTP/3 support.
///
/// The `Alt-Svc` header is served on all responses regardless of scheme.
/// Chrome ignores it for plain-HTTP origins; it only upgrades to QUIC when
/// the header arrives over HTTPS — exactly the behavior we want.
async fn add_server_timestamp(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let ts = Utc::now().to_rfc3339();
    if let Ok(val) = HeaderValue::from_str(&ts) {
        response
            .headers_mut()
            .insert("x-networker-server-timestamp", val);
    }
    response.headers_mut().insert(
        "x-networker-server-version",
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
    // Advertise H3 so Chrome can upgrade to QUIC on the next request to this origin.
    if let Some(port) = state.h3_port {
        let alt_svc = format!("h3=\":{port}\"; ma=86400");
        if let Ok(val) = HeaderValue::from_str(&alt_svc) {
            response.headers_mut().insert("alt-svc", val);
        }
    }
    response
}

// ─────────────────────────────────────────────────────────────────────────────
// Bearer token auth middleware (BENCH_API_TOKEN)
// ─────────────────────────────────────────────────────────────────────────────

/// Middleware that enforces bearer-token authentication when
/// `state.bench_token` is set (production fills it from `BENCH_API_TOKEN`).
/// `/health` is always exempt so load-balancer probes keep working.
/// A `Server-Timing: auth;dur=X.X` metric is appended to every response.
async fn bench_auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let t0 = Instant::now();

    // /health is exempt — health checks must work without credentials.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    if let Some(expected) = state.bench_token.as_deref() {
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));

        match auth {
            Some(token) if token == expected => { /* valid */ }
            _ => {
                let dur_ms = elapsed_ms(t0);
                let mut resp = (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "unauthorized"})),
                )
                    .into_response();
                if let Ok(val) = HeaderValue::from_str(&format!("auth;dur={dur_ms:.1}")) {
                    resp.headers_mut().insert("server-timing", val);
                }
                return resp;
            }
        }
    }

    let dur_ms = elapsed_ms(t0);
    let mut resp = next.run(req).await;

    // Append auth timing to existing Server-Timing or create a new one.
    let auth_metric = format!("auth;dur={dur_ms:.1}");
    if let Some(existing) = resp.headers().get("server-timing").cloned() {
        let combined = format!("{}, {auth_metric}", existing.to_str().unwrap_or(""));
        if let Ok(val) = HeaderValue::from_str(&combined) {
            resp.headers_mut().insert("server-timing", val);
        }
    } else if let Ok(val) = HeaderValue::from_str(&auth_metric) {
        resp.headers_mut().insert("server-timing", val);
    }

    resp
}

// ─────────────────────────────────────────────────────────────────────────────
// Context-switch helpers (Unix only)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `(voluntary_csw, involuntary_csw)` for the server process.
#[cfg(unix)]
fn csw_snapshot() -> (i64, i64) {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    (u.ru_nvcsw, u.ru_nivcsw)
}

/// Process CPU time (user + system) in milliseconds. Delta across the upload
/// drain window feeds `Server-Timing: cpu;dur=X` — cpu/recv ratio ≈ how much
/// of the transfer window this process spent on-CPU, the server-side
/// "was the endpoint CPU-bound?" evidence for the infrastructure envelope.
#[cfg(unix)]
fn cpu_ms_snapshot() -> f64 {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let user_us = u.ru_utime.tv_sec as f64 * 1e6 + u.ru_utime.tv_usec as f64;
    let sys_us = u.ru_stime.tv_sec as f64 * 1e6 + u.ru_stime.tv_usec as f64;
    (user_us + sys_us) / 1000.0
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Landing page helpers
// ─────────────────────────────────────────────────────────────────────────────

fn format_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn get_hostname() -> String {
    // Unix: HOSTNAME env var
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    // Windows: COMPUTERNAME env var
    if let Ok(h) = std::env::var("COMPUTERNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    // Linux: /proc/sys/kernel/hostname
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            return h;
        }
    }
    // Fallback: run `hostname` command
    if let Ok(out) = std::process::Command::new("hostname").output() {
        if out.status.success() {
            let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !h.is_empty() {
                return h;
            }
        }
    }
    "unknown".to_string()
}

// Static CSS + HTML head shared by the landing page (raw string avoids escaping issues).
const LANDING_HTML_HEAD: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="icon" href="data:,">
<style>
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui,-apple-system,sans-serif;background:#0f1117;color:#e8e8e8;padding:2rem 2.5rem;max-width:940px}
h1{font-size:1.6rem;color:#fff;font-weight:700}
.meta{color:#7a9aaa;font-size:.85rem;margin:.3rem 0 1.2rem}
.status{display:inline-flex;align-items:center;gap:.4rem;background:#1b3a1b;color:#4caf50;border:1px solid #2e5a2e;padding:.25rem .8rem;border-radius:20px;font-size:.8rem;font-weight:600;margin-bottom:1.5rem}
.dot{width:7px;height:7px;background:#4caf50;border-radius:50%;animation:pulse 1.5s ease-in-out infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:1rem;margin-bottom:1.5rem}
.card{background:#1a1a2e;border:1px solid #2a2a40;border-radius:10px;padding:1rem 1.2rem}
.full{margin-bottom:1.5rem}
.card-title{font-size:.7rem;text-transform:uppercase;letter-spacing:.08em;color:#5a6a7a;font-weight:600;margin-bottom:.8rem}
.row{display:flex;justify-content:space-between;align-items:center;padding:.3rem 0;border-bottom:1px solid #1e1e30}
.row:last-child{border-bottom:none}
.lbl{color:#8a9aaa;font-size:.82rem}
.val{font-family:"SF Mono","Fira Mono",monospace;font-size:.82rem;color:#7ac0ff}
.proto-list{display:flex;flex-wrap:wrap;gap:.4rem}
.proto{background:#1a2a40;color:#7ac0ff;border:1px solid #2a3a50;border-radius:4px;padding:.2rem .5rem;font-size:.75rem;font-family:monospace}
table{width:100%;border-collapse:collapse}
th{font-size:.7rem;text-transform:uppercase;letter-spacing:.06em;color:#5a6a7a;padding:.4rem .6rem;border-bottom:1px solid #2a2a40;text-align:left}
td{padding:.4rem .6rem;border-bottom:1px solid #1e1e30;vertical-align:middle}
td:first-child{font-family:monospace;color:#7ac0ff;font-size:.82rem}
.method{font-family:monospace;color:#f0a050;font-size:.75rem}
.desc{color:#8a9aaa;font-size:.82rem}
tr:hover td{background:#1a1a28}
.footer{color:#3a4a5a;font-size:.75rem;margin-top:1.5rem}
.footer a{color:#4a7a9a;text-decoration:none}
.footer a:hover{color:#7ac0ff}
</style>
</head>
<body>
"##;

const LANDING_HTML_FOOT: &str = "</body></html>\n";

/// GET / → HTML status page showing server info, ports, and available endpoints.
async fn landing_page(State(state): State<AppState>) -> impl IntoResponse {
    let version = env!("CARGO_PKG_VERSION");
    let elapsed = state.started_at.elapsed().as_secs();
    let uptime = format_uptime(elapsed);
    let hostname = get_hostname();
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let started = Utc::now()
        .checked_sub_signed(chrono::Duration::seconds(elapsed as i64))
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let h3_port_display = state
        .h3_port
        .map(|p| p.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    let h3_proto = if state.h3_port.is_some() {
        r#"<span class="proto">HTTP/3</span>"#
    } else {
        ""
    };

    let mut out = String::with_capacity(8 * 1024);
    out.push_str(LANDING_HTML_HEAD);

    // Header + status badge
    out.push_str(&format!(
        "<h1>networker-endpoint</h1>\n\
         <div class=\"meta\">v{version} &middot; {hostname}</div>\n\
         <div class=\"status\"><span class=\"dot\"></span>running &nbsp; uptime {uptime}</div>\n"
    ));

    // Info grid
    out.push_str("<div class=\"grid\">\n");

    // Ports card
    out.push_str(&format!(
        "<div class=\"card\">\n\
           <div class=\"card-title\">Ports</div>\n\
           <div class=\"row\"><span class=\"lbl\">HTTP</span><span class=\"val\">:{http_port}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">HTTPS / H2</span><span class=\"val\">:{https_port}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">HTTP/3 QUIC</span><span class=\"val\">{h3_port_display}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">UDP echo</span><span class=\"val\">:{udp_port}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">UDP throughput</span><span class=\"val\">:{udp_tp_port}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">STAMP reflector</span><span class=\"val\">:{stamp_port}</span></div>\n\
         </div>\n",
        http_port = state.http_port,
        https_port = state.https_port,
        h3_port_display = h3_port_display,
        udp_port = state.udp_port,
        udp_tp_port = state.udp_throughput_port,
        stamp_port = state.stamp_port,
    ));

    // Protocols card
    out.push_str(&format!(
        "<div class=\"card\">\n\
           <div class=\"card-title\">Protocols</div>\n\
           <div class=\"proto-list\">\n\
             <span class=\"proto\">HTTP/1.1</span>\n\
             <span class=\"proto\">HTTP/2</span>\n\
             {h3_proto}\n\
             <span class=\"proto\">UDP</span>\n\
           </div>\n\
         </div>\n"
    ));

    // Server info card
    out.push_str(&format!(
        "<div class=\"card\">\n\
           <div class=\"card-title\">Server</div>\n\
           <div class=\"row\"><span class=\"lbl\">Version</span><span class=\"val\">{version}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">Started</span><span class=\"val\">{started}</span></div>\n\
           <div class=\"row\"><span class=\"lbl\">Now</span><span class=\"val\">{timestamp}</span></div>\n\
         </div>\n"
    ));

    out.push_str("</div>\n"); // end .grid

    // Endpoints table
    out.push_str(
        "<div class=\"card full\">\n\
           <div class=\"card-title\">Endpoints</div>\n\
           <table>\n\
             <thead><tr><th>Path</th><th>Method</th><th>Description</th></tr></thead>\n\
             <tbody>\n\
               <tr><td>/</td><td class=\"method\">GET</td><td class=\"desc\">This status page</td></tr>\n\
               <tr><td>/health</td><td class=\"method\">GET</td><td class=\"desc\">Health check — 200 + JSON</td></tr>\n\
               <tr><td>/info</td><td class=\"method\">GET</td><td class=\"desc\">Server capabilities as JSON</td></tr>\n\
               <tr><td>/echo</td><td class=\"method\">GET / POST</td><td class=\"desc\">Echo request body and headers</td></tr>\n\
               <tr><td>/ws</td><td class=\"method\">GET</td><td class=\"desc\">WebSocket echo — upgrades and echoes frames back</td></tr>\n\
               <tr><td>/download</td><td class=\"method\">GET</td><td class=\"desc\">Stream N zero bytes — ?bytes=N</td></tr>\n\
               <tr><td>/upload</td><td class=\"method\">POST</td><td class=\"desc\">Drain request body, return byte count</td></tr>\n\
               <tr><td>/delay</td><td class=\"method\">GET</td><td class=\"desc\">Delay response by N ms — ?ms=N (max 30 s)</td></tr>\n\
               <tr><td>/headers</td><td class=\"method\">GET</td><td class=\"desc\">Echo all request headers as JSON</td></tr>\n\
               <tr><td>/status/:code</td><td class=\"method\">GET</td><td class=\"desc\">Return specified HTTP status code</td></tr>\n\
               <tr><td>/http-version</td><td class=\"method\">GET</td><td class=\"desc\">Return HTTP version used by the client</td></tr>\n\
               <tr><td>/page</td><td class=\"method\">GET</td><td class=\"desc\">Page-load asset manifest — ?assets=N&amp;bytes=B</td></tr>\n\
               <tr><td>/browser-page</td><td class=\"method\">GET</td><td class=\"desc\">HTML page with img tags for browser probes</td></tr>\n\
               <tr><td>/asset</td><td class=\"method\">GET</td><td class=\"desc\">Single binary asset — ?id=X&amp;bytes=B</td></tr>\n\
               <tr><td>/api/users</td><td class=\"method\">GET</td><td class=\"desc\">Paginated users — ?page=N&amp;sort=field&amp;order=asc</td></tr>\n\
               <tr><td>/api/transform</td><td class=\"method\">POST</td><td class=\"desc\">SHA-256 hash fields, reverse values</td></tr>\n\
               <tr><td>/api/aggregate</td><td class=\"method\">GET</td><td class=\"desc\">Time-series stats — ?range=start,end</td></tr>\n\
               <tr><td>/api/search</td><td class=\"method\">GET</td><td class=\"desc\">Regex search — ?q=term&amp;limit=N</td></tr>\n\
               <tr><td>/api/upload/process</td><td class=\"method\">POST</td><td class=\"desc\">CRC32 + SHA-256 + zlib compress body</td></tr>\n\
               <tr><td>/api/delayed</td><td class=\"method\">GET</td><td class=\"desc\">Controlled delay — ?ms=N&amp;work=light</td></tr>\n\
               <tr><td>/api/validate</td><td class=\"method\">GET</td><td class=\"desc\">Endpoint output checksums — ?seed=N</td></tr>\n\
             </tbody>\n\
           </table>\n\
         </div>\n",
    );

    // Footer
    out.push_str(&format!(
        "<div class=\"footer\">\
           <a href=\"/health\">/health</a> &nbsp;&middot;&nbsp; \
           <a href=\"/info\">/info</a> \
           &nbsp;&middot;&nbsp; networker-endpoint v{version}\
         </div>\n"
    ));

    out.push_str(LANDING_HTML_FOOT);

    Response::builder()
        .status(200)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(out))
        .unwrap()
}

/// GET /health → 200 JSON { "status": "ok", "runtime": "rust", ... }
///
/// Constant-work per API-SPEC.md §5.1: the body is a compile-time constant so
/// every language's /health does identical (zero) per-request work. `runtime`
/// and `version` are required by the orchestrator contract (validator.rs).
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    // `services` is the target's capability self-report — LIVE truth about
    // what this instance actually runs (a port CAN be 0-disabled, e.g.
    // `--stamp-port 0` since v0.28.170), so launch flows can offer exactly
    // the tests this target supports instead of inferring them from static
    // config ("the target must return the tests supported", 2026-08-13).
    // Additive fields only: pre-existing consumers key on status/version.
    let port_or_null = |p: u16| -> serde_json::Value {
        if p == 0 {
            serde_json::Value::Null
        } else {
            serde_json::json!(p)
        }
    };
    Json(serde_json::json!({
        "status": "ok",
        "runtime": "rust",
        "service": "networker-endpoint",
        "version": env!("CARGO_PKG_VERSION"),
        "services": {
            // Always-on HTTP routes (same process as this handler).
            "download": true,
            "upload": true,
            "ws_echo": true,
            "page_assets": true,
            // Optional/disable-able listeners, by port; null = not running.
            "udp_echo": port_or_null(state.udp_port),
            "udp_throughput": port_or_null(state.udp_throughput_port),
            "stamp": port_or_null(state.stamp_port),
            "h3": state.h3_port.map_or(serde_json::Value::Null, |p| serde_json::json!(p)),
        },
    }))
}

/// GET /echo – returns empty body with request info
async fn echo_get(headers: HeaderMap) -> impl IntoResponse {
    let hdrs: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    Json(serde_json::json!({
        "method": "GET",
        "headers": hdrs,
        "body_bytes": 0,
    }))
}

/// POST /echo – echoes the request body back in the response
async fn echo(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let body_len = body.len();
    let hdrs: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    // Return the body + a JSON envelope in the headers
    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("x-echo-body-bytes", body_len.to_string())
        .header("x-echo-received-headers", hdrs.len().to_string());

    // If the body is small enough to be UTF-8 JSON, return it directly;
    // otherwise return raw bytes.
    if body_len <= 1_048_576 {
        resp.body(Body::from(body)).unwrap()
    } else {
        Response::builder()
            .status(413)
            .body(Body::from("Payload too large (> 1 MiB)"))
            .unwrap()
    }
}

/// GET /ws → WebSocket upgrade → echo every Text/Binary frame back unchanged.
///
/// Serves the tester's `websocket` probe mode: after the HTTP 101 upgrade the
/// probe sends N messages and measures per-message round-trip time, so the
/// handler must echo frames verbatim (payload bytes carry the probe's sequence
/// id + send timestamp). Ping frames are answered with Pong automatically by
/// axum/tungstenite; Close ends the session.
async fn ws_echo(ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_ws_echo)
}

async fn handle_ws_echo(mut socket: axum::extract::ws::WebSocket) {
    use axum::extract::ws::Message;
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(_) | Message::Binary(_) => {
                if socket.send(msg).await.is_err() {
                    break;
                }
            }
            // Ping is auto-ponged by the WS stack; Pong needs no reply.
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Close(_) => break,
        }
    }
}

#[derive(Deserialize)]
struct DownloadParams {
    bytes: Option<usize>,
}

/// Canonical download payload per API-SPEC.md §5.2: fill byte 0x42 ('B')
/// streamed in 8 KiB chunks, capped at 2 GiB. Fill byte and chunk size are
/// part of the measured workload, so they are pinned across all languages.
const DOWNLOAD_FILL: u8 = 0x42;
const DOWNLOAD_CHUNK: usize = 8 * 1024; // 8 KiB
const DOWNLOAD_CAP: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

/// GET /download?bytes=N — deprecated query-form alias kept for pre-0.28.28
/// testers. Canonical form is `GET /download/{size}`.
async fn download(Query(p): Query<DownloadParams>) -> impl IntoResponse {
    download_response(p.bytes.unwrap_or(1024))
}

/// GET /download/{size} — canonical path form (orchestrator contract).
async fn download_path(Path(size): Path<usize>) -> impl IntoResponse {
    download_response(size)
}

/// Streams N fill bytes (max 2 GiB) in 8 KiB chunks.
/// Adds `Server-Timing: proc;dur=X, csw-v;dur=N, csw-i;dur=N` indicating
/// setup time and context switches.
fn download_response(requested: usize) -> Response {
    let n = requested.min(DOWNLOAD_CAP);
    let t0 = Instant::now();
    #[cfg(unix)]
    let (csw_v0, csw_i0) = csw_snapshot();

    // Stream fill bytes in fixed-size chunks to avoid allocating the full
    // payload in memory at once (critical for multi-GiB downloads).
    let full_chunks = n / DOWNLOAD_CHUNK;
    let remainder = n % DOWNLOAD_CHUNK;
    let fill_chunk = Bytes::from(vec![DOWNLOAD_FILL; DOWNLOAD_CHUNK]);

    let body = Body::from_stream(futures::stream::iter(
        (0..full_chunks)
            .map(move |_| Ok::<_, std::io::Error>(fill_chunk.clone()))
            .chain(if remainder > 0 {
                vec![Ok(Bytes::from(vec![DOWNLOAD_FILL; remainder]))].into_iter()
            } else {
                vec![].into_iter()
            }),
    ));

    let proc_ms = elapsed_ms(t0);
    #[cfg(unix)]
    let csw_part = {
        let (csw_v1, csw_i1) = csw_snapshot();
        format!(
            ", csw-v;dur={}, csw-i;dur={}",
            csw_v1 - csw_v0,
            csw_i1 - csw_i0
        )
    };
    #[cfg(not(unix))]
    let csw_part = "";

    let timing = format!("proc;dur={proc_ms:.3}{csw_part}");
    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", n.to_string())
        .header("x-download-bytes", n.to_string())
        .header("server-timing", timing.as_str())
        .body(body)
        .unwrap()
}

#[derive(Serialize)]
struct UploadStats {
    received_bytes: usize,
    timestamp: String,
}

/// POST /upload – drains the request body without buffering it in memory,
/// then returns a JSON stats object with the byte count.
///
/// Adds `Server-Timing: recv;dur=X` (body drain time) and echoes
/// `X-Networker-Request-Id` from the request if present.
/// Adds `X-Networker-Received-Bytes` with the actual drained byte count so the
/// client can verify the upload was not silently truncated.
async fn upload(req: Request) -> impl IntoResponse {
    // Extract request metadata before consuming the body.
    let request_id = req
        .headers()
        .get("x-networker-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    let t0 = Instant::now();
    #[cfg(unix)]
    let (csw_v0, csw_i0) = csw_snapshot();
    #[cfg(unix)]
    let cpu_ms0 = cpu_ms_snapshot();
    let mut received_bytes: usize = 0;
    let mut body = req.into_body();
    while let Some(Ok(frame)) = body.frame().await {
        if let Ok(data) = frame.into_data() {
            received_bytes += data.len();
        }
    }
    let recv_ms = elapsed_ms(t0);
    #[cfg(unix)]
    let csw_part = {
        let (csw_v1, csw_i1) = csw_snapshot();
        // cpu;dur = process-CPU ms this process burned while draining the
        // body. Process-wide (concurrent requests share the counter), like
        // the csw fields — single-probe traffic is the intended reading.
        format!(
            ", csw-v;dur={}, csw-i;dur={}, cpu;dur={:.3}",
            csw_v1 - csw_v0,
            csw_i1 - csw_i0,
            cpu_ms_snapshot() - cpu_ms0
        )
    };
    #[cfg(not(unix))]
    let csw_part = "";

    let mut resp = Json(UploadStats {
        received_bytes,
        timestamp: Utc::now().to_rfc3339(),
    })
    .into_response();

    let timing = format!("recv;dur={recv_ms:.3}{csw_part}");
    if let Ok(v) = HeaderValue::from_str(&timing) {
        resp.headers_mut().insert("server-timing", v);
    }
    // Always echo the actual received byte count as a response header so the
    // client can detect upload truncation without parsing the JSON body.
    resp.headers_mut().insert(
        "x-networker-received-bytes",
        HeaderValue::from(received_bytes as u64),
    );
    if let Some(rid) = request_id {
        if let Ok(v) = HeaderValue::from_str(&rid) {
            resp.headers_mut().insert("x-networker-request-id", v);
        }
    }

    resp
}

#[derive(Deserialize)]
struct DelayParams {
    ms: Option<u64>,
}

/// GET /delay?ms=N – sleeps N ms (max 30 s) then returns 200
async fn delay(Query(p): Query<DelayParams>) -> impl IntoResponse {
    let ms = p.ms.unwrap_or(0).min(30_000);
    sleep(Duration::from_millis(ms)).await;
    Json(serde_json::json!({
        "delayed_ms": ms,
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

/// GET /headers – returns all received request headers as JSON
async fn headers_echo(headers: HeaderMap) -> impl IntoResponse {
    let map: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    Json(map)
}

/// GET /status/:code – returns the specified HTTP status code
async fn status_code(Path(code): Path<u16>) -> impl IntoResponse {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
    (
        status,
        Json(serde_json::json!({
            "status": code,
            "description": status.canonical_reason().unwrap_or("Unknown"),
        })),
    )
}

/// GET /http-version – returns the HTTP version used by the client
async fn http_version(req: Request) -> impl IntoResponse {
    let version = match req.version() {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "Unknown",
    };
    Json(serde_json::json!({
        "version": version,
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

/// GET /info – server capabilities and system metadata
async fn server_info(State(state): State<AppState>) -> impl IntoResponse {
    let uptime_secs = state.started_at.elapsed().as_secs();
    let mut body = serde_json::json!({
        "service": "networker-endpoint",
        "version": env!("CARGO_PKG_VERSION"),
        "protocols": if cfg!(feature = "http3") {
            serde_json::json!(["HTTP/1.1", "HTTP/2", "HTTP/3"])
        } else {
            serde_json::json!(["HTTP/1.1", "HTTP/2"])
        },
        "http3": cfg!(feature = "http3"),
        "endpoints": [
            "/health", "/echo", "/download", "/upload",
            "/delay", "/headers", "/status/:code", "/http-version", "/info",
            "/api/users", "/api/transform", "/api/aggregate", "/api/search",
            "/api/upload/process", "/api/delayed", "/api/validate"
        ],
        "system": &state.system_meta,
        "region": &state.system_meta.region,
        "uptime_secs": uptime_secs,
        "timestamp": Utc::now().to_rfc3339(),
    });
    // Live load sampled per request (unlike the startup-time system_meta).
    // Additive; absent on platforms that don't expose the value (honesty
    // over fabrication — mirrors the tester's LoadSample semantics).
    if let Some(load) = detect_load_avg_1m() {
        body["load_avg_1m"] = serde_json::json!(load);
    }
    if let Some(mem) = detect_mem_available_mb() {
        body["mem_available_mb"] = serde_json::json!(mem);
    }
    Json(body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Page-load simulation routes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PageParams {
    assets: Option<usize>,
    bytes: Option<usize>,
}

#[derive(Deserialize)]
struct AssetParams {
    #[allow(dead_code)]
    id: Option<u32>,
    bytes: Option<usize>,
}

/// GET /page?assets=N&bytes=B → JSON manifest listing N asset URLs.
async fn page_manifest(Query(p): Query<PageParams>) -> impl IntoResponse {
    let n = p.assets.unwrap_or(20).min(500);
    let b = p.bytes.unwrap_or(10_240);
    let assets: Vec<String> = (0..n).map(|i| format!("/asset?id={i}&bytes={b}")).collect();
    Json(serde_json::json!({
        "asset_count": n,
        "asset_bytes": b,
        "assets": assets,
    }))
}

/// GET /browser-page?assets=N&bytes=B → HTML page with N `<img>` tags pointing to /asset.
///
/// Each img src triggers a real HTTP fetch; the browser's `load` event fires only after
/// all images have settled (loaded or errored), making this suitable for measuring full
/// page-load time with a real browser (chromiumoxide / CDP).
async fn browser_page(Query(p): Query<PageParams>) -> impl IntoResponse {
    let n = p.assets.unwrap_or(20).min(500);
    let b = p.bytes.unwrap_or(10_240);

    let mut html = String::from(
        "<!DOCTYPE html>\n\
         <html><head><title>Networker Page Load Test</title><link rel=\"icon\" href=\"data:,\"></head>\n\
         <body>\n",
    );
    for i in 0..n {
        html.push_str(&format!(
            "<img src=\"/asset?id={i}&bytes={b}\" width=\"1\" height=\"1\" alt=\"\">\n"
        ));
    }
    html.push_str("</body></html>\n");

    Response::builder()
        .status(200)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}

/// GET /asset?id=X&bytes=B → B zero bytes, content-type: application/octet-stream.
async fn asset_handler(Query(p): Query<AssetParams>) -> impl IntoResponse {
    let n = p.bytes.unwrap_or(10_240).min(100 * 1024 * 1024); // cap 100 MiB

    // Stream the zero fill in fixed-size chunks (same approach as
    // `download_response`) instead of allocating the full payload — a few
    // concurrent 100 MiB /asset requests would otherwise spike memory.
    let full_chunks = n / DOWNLOAD_CHUNK;
    let remainder = n % DOWNLOAD_CHUNK;
    let fill_chunk = Bytes::from(vec![0u8; DOWNLOAD_CHUNK]);

    let body = Body::from_stream(futures::stream::iter(
        (0..full_chunks)
            .map(move |_| Ok::<_, std::io::Error>(fill_chunk.clone()))
            .chain(if remainder > 0 {
                vec![Ok(Bytes::from(vec![0u8; remainder]))].into_iter()
            } else {
                vec![].into_iter()
            }),
    ));

    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", n.to_string())
        .body(body)
        .unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON API benchmark endpoints
// ─────────────────────────────────────────────────────────────────────────────

/// Helper: build standard benchmark response headers.
fn bench_headers(dur_ms: f64) -> HeaderMap {
    let mut h = HeaderMap::new();
    let timing = format!("app;dur={dur_ms:.1}");
    h.insert("server-timing", HeaderValue::from_str(&timing).unwrap());
    h.insert(
        "cache-control",
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    h.insert("timing-allow-origin", HeaderValue::from_static("*"));
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    h
}

// ── Shared benchmark dataset (loaded once from bench-data.json) ─────────────

#[derive(Debug, serde::Deserialize)]
struct BenchData {
    users: Vec<serde_json::Value>,
    search_corpus: Vec<String>,
    timeseries: Vec<serde_json::Value>,
    #[allow(dead_code)]
    transform_inputs: Vec<serde_json::Value>,
    expected_checksums: serde_json::Map<String, serde_json::Value>,
}

static BENCH_DATA: OnceLock<Option<BenchData>> = OnceLock::new();

fn parse_bench_data(path: &str) -> Result<BenchData, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str::<BenchData>(&content).map_err(|e| e.to_string())
}

/// Load the shared benchmark dataset per API-SPEC.md §2.
///
/// Dataset-load failure is FATAL — a server that silently benchmarks
/// different input data poisons cross-language comparisons (audit F2):
/// - `BENCH_DATA_PATH` set but missing/unparsable → exit(1).
/// - A fallback path exists on disk but fails to parse → exit(1).
/// - No dataset found anywhere → PRNG fallback is tolerated only because
///   networker-endpoint doubles as the production diagnostic server;
///   benchmark deployments always ship the dataset.
fn load_bench_data() -> Option<&'static BenchData> {
    BENCH_DATA
        .get_or_init(|| {
            if let Ok(p) = std::env::var("BENCH_DATA_PATH") {
                if !p.is_empty() {
                    match parse_bench_data(&p) {
                        Ok(data) => {
                            tracing::info!("Loaded bench-data.json from {p}");
                            return Some(data);
                        }
                        Err(e) => {
                            eprintln!(
                                "FATAL: BENCH_DATA_PATH={p} could not be loaded: {e} \
                                 (dataset load failure must not fall back to PRNG data)"
                            );
                            std::process::exit(1);
                        }
                    }
                }
            }
            for p in [
                "/opt/bench/bench-data.json",
                "benchmarks/reference-apis/shared/bench-data.json",
            ] {
                if !std::path::Path::new(p).exists() {
                    continue;
                }
                match parse_bench_data(p) {
                    Ok(data) => {
                        tracing::info!("Loaded bench-data.json from {p}");
                        return Some(data);
                    }
                    Err(e) => {
                        eprintln!(
                            "FATAL: bench-data.json exists at {p} but could not be loaded: {e}"
                        );
                        std::process::exit(1);
                    }
                }
            }
            tracing::warn!(
                "bench-data.json not found, JSON API endpoints will use fallback PRNG data \
                 (benchmark runs must deploy the shared dataset)"
            );
            None
        })
        .as_ref()
}

// ── Deterministic name/email generators ─────────────────────────────────────

const FIRST_NAMES: &[&str] = &[
    "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hector", "Iris", "Jack", "Karen",
    "Leo", "Mona", "Nick", "Olivia", "Paul", "Quinn", "Rosa", "Steve", "Tina", "Uma", "Victor",
    "Wendy", "Xander", "Yuki", "Zane",
];
const LAST_NAMES: &[&str] = &[
    "Smith",
    "Johnson",
    "Williams",
    "Brown",
    "Jones",
    "Garcia",
    "Miller",
    "Davis",
    "Rodriguez",
    "Martinez",
    "Hernandez",
    "Lopez",
    "Gonzalez",
    "Wilson",
    "Anderson",
    "Thomas",
    "Taylor",
    "Moore",
    "Jackson",
    "Martin",
    "Lee",
    "Perez",
    "Thompson",
    "White",
    "Harris",
    "Sanchez",
];
const DOMAINS: &[&str] = &[
    "example.com",
    "test.org",
    "mail.net",
    "corp.io",
    "bench.dev",
];

fn gen_user(rng: &mut StdRng, id: u64) -> serde_json::Value {
    let first = FIRST_NAMES[rng.random_range(0..FIRST_NAMES.len())];
    let last = LAST_NAMES[rng.random_range(0..LAST_NAMES.len())];
    let domain = DOMAINS[rng.random_range(0..DOMAINS.len())];
    let score: f64 = (rng.random::<f64>() * 10000.0).round() / 100.0;
    let day = rng.random_range(1u32..29);
    let month = rng.random_range(1u32..13);
    let year = rng.random_range(2018u32..2026);
    serde_json::json!({
        "id": id,
        "name": format!("{first} {last}"),
        "email": format!("{}.{}@{}", first.to_lowercase(), last.to_lowercase(), domain),
        "score": score,
        "created_at": format!("{year:04}-{month:02}-{day:02}T00:00:00Z"),
    })
}

#[derive(Deserialize)]
struct UsersParams {
    page: Option<u64>,
    sort: Option<String>,
    order: Option<String>,
}

/// GET /api/users?page=N&sort=field&order=asc
/// Return users from shared bench-data.json, falling back to deterministic PRNG.
/// PRNG-fallback user page for dataset-less installs (plain network-mode
/// endpoints without bench-data.json). Pure given the page number — extracted
/// from the handler so its arithmetic is unit-testable regardless of whether
/// the test environment finds the shared dataset.
fn fallback_users(page: u64) -> Vec<serde_json::Value> {
    let mut rng = StdRng::seed_from_u64(page);
    (0..100)
        .map(|i| gen_user(&mut rng, (page - 1) * 100 + i + 1))
        .collect()
}

async fn api_users(Query(p): Query<UsersParams>) -> impl IntoResponse {
    let t0 = Instant::now();
    let page = p.page.unwrap_or(1).max(1);
    let sort_field = p.sort.as_deref().unwrap_or("id");
    let ascending = p.order.as_deref().unwrap_or("asc") != "desc";

    let mut users: Vec<serde_json::Value> = if let Some(data) = load_bench_data() {
        // Shared dataset: paginate the pre-generated user list
        let start = ((page - 1) * 100) as usize;
        let end = (start + 100).min(data.users.len());
        if start < data.users.len() {
            data.users[start..end].to_vec()
        } else {
            // Page beyond dataset — return empty
            Vec::new()
        }
    } else {
        fallback_users(page)
    };

    // Sort by requested field
    users.sort_by(|a, b| {
        let cmp = match sort_field {
            // (Previously compared a["name"] to ITSELF first — always Equal,
            // dead code found by the mutation map — before the real a-vs-b.)
            "name" => a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or("")),
            "email" => a["email"]
                .as_str()
                .unwrap_or("")
                .cmp(b["email"].as_str().unwrap_or("")),
            "score" => a["score"]
                .as_f64()
                .unwrap_or(0.0)
                .partial_cmp(&b["score"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal),
            "created_at" => a["created_at"]
                .as_str()
                .unwrap_or("")
                .cmp(b["created_at"].as_str().unwrap_or("")),
            _ => a["id"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&b["id"].as_u64().unwrap_or(0)),
        };
        if ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });

    let paginated: Vec<_> = users.into_iter().take(20).collect();
    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(paginated))
}

#[derive(Deserialize)]
struct TransformBody {
    seed: Option<u64>,
    fields: Option<Vec<String>>,
    values: Option<Vec<serde_json::Value>>,
}

/// POST /api/transform
/// SHA-256 hash each string field, reverse values array.
async fn api_transform(Json(body): Json<TransformBody>) -> impl IntoResponse {
    let t0 = Instant::now();

    let hashed_fields: Vec<String> = body
        .fields
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let mut hasher = Sha256::new();
            hasher.update(f.as_bytes());
            hex::encode(hasher.finalize())
        })
        .collect();

    let mut reversed_values = body.values.unwrap_or_default();
    reversed_values.reverse();

    let result = serde_json::json!({
        "seed": body.seed.unwrap_or(0),
        "hashed_fields": hashed_fields,
        "reversed_values": reversed_values,
    });

    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(result))
}

#[derive(Deserialize)]
struct AggregateParams {
    range: Option<String>,
}

/// GET /api/aggregate?range=start,end
/// Compute stats from shared bench-data.json timeseries, falling back to PRNG.
async fn api_aggregate(Query(p): Query<AggregateParams>) -> impl IntoResponse {
    let t0 = Instant::now();

    let (start, _end) = match p.range.as_deref() {
        Some(r) => {
            let parts: Vec<&str> = r.split(',').collect();
            let s: u64 = parts.first().and_then(|v| v.parse().ok()).unwrap_or(1);
            let e: u64 = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(100);
            (s, e)
        }
        None => (1u64, 100u64),
    };

    let mut values: Vec<f64> = if let Some(data) = load_bench_data() {
        data.timeseries
            .iter()
            .filter_map(|v| v["value"].as_f64())
            .collect()
    } else {
        let mut rng = StdRng::seed_from_u64(start);
        (0..10_000).map(|_| rng.random::<f64>() * 1000.0).collect()
    };
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let n = values.len() as f64;
    let sum: f64 = values.iter().sum();
    let mean = sum / n;
    let p50 = values[(values.len() as f64 * 0.50) as usize];
    let p95 = values[(values.len() as f64 * 0.95) as usize];
    let max = values.last().copied().unwrap_or(0.0);

    // Group into 5 categories by quintile
    let chunk_size = values.len() / 5;
    let categories: Vec<serde_json::Value> = (0..5)
        .map(|i| {
            let chunk = &values[i * chunk_size..(i + 1) * chunk_size];
            let cat_sum: f64 = chunk.iter().sum();
            let cat_mean = cat_sum / chunk.len() as f64;
            serde_json::json!({
                "category": format!("q{}", i + 1),
                "count": chunk.len(),
                "mean": (cat_mean * 100.0).round() / 100.0,
                "min": (chunk[0] * 100.0).round() / 100.0,
                "max": (chunk[chunk.len() - 1] * 100.0).round() / 100.0,
            })
        })
        .collect();

    let result = serde_json::json!({
        "total_points": 10_000,
        "mean": (mean * 100.0).round() / 100.0,
        "p50": (p50 * 100.0).round() / 100.0,
        "p95": (p95 * 100.0).round() / 100.0,
        "max": (max * 100.0).round() / 100.0,
        "categories": categories,
    });

    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(result))
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    limit: Option<usize>,
}

/// GET /api/search?q=term&limit=N
/// Search items from shared bench-data.json, falling back to PRNG generation.
async fn api_search(Query(p): Query<SearchParams>) -> impl IntoResponse {
    let t0 = Instant::now();
    let query = p.q.as_deref().unwrap_or("test");
    let limit = p.limit.unwrap_or(20).min(100);

    let items: Vec<String> = if let Some(data) = load_bench_data() {
        data.search_corpus.clone()
    } else {
        let mut rng = StdRng::seed_from_u64(42);
        let words: &[&str] = &[
            "network",
            "latency",
            "throughput",
            "bandwidth",
            "packet",
            "server",
            "client",
            "request",
            "response",
            "timeout",
            "connection",
            "socket",
            "protocol",
            "testing",
            "benchmark",
            "performance",
            "endpoint",
            "proxy",
            "firewall",
            "router",
            "switch",
            "gateway",
            "dns",
            "tls",
            "quic",
        ];
        (0..1_000)
            .map(|_| {
                let w1 = words[rng.random_range(0..words.len())];
                let w2 = words[rng.random_range(0..words.len())];
                let n: u32 = rng.random_range(1..1000);
                format!("{w1}-{w2}-{n}")
            })
            .collect()
    };

    // Apply regex filter; fall back to literal match on invalid regex
    let re = regex::Regex::new(query).ok();
    let mut scored: Vec<(usize, &str)> = items
        .iter()
        .filter_map(|item| {
            let matched = match &re {
                Some(r) => r.find(item).map(|m| m.start()),
                None => item.find(query),
            };
            matched.map(|pos| (pos, item.as_str()))
        })
        .collect();

    // Sort by match position (earlier = better), then alphabetically
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));

    let results: Vec<serde_json::Value> = scored
        .iter()
        .take(limit)
        .enumerate()
        .map(|(rank, (pos, item))| {
            serde_json::json!({
                "rank": rank + 1,
                "item": item,
                "match_position": pos,
            })
        })
        .collect();

    let result = serde_json::json!({
        "query": query,
        "total_matches": scored.len(),
        "returned": results.len(),
        "results": results,
    });

    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(result))
}

/// POST /api/upload/process
/// Read body, compute CRC32 + SHA-256, compress with zlib.
async fn api_upload_process(req: Request) -> impl IntoResponse {
    let t0 = Instant::now();

    // Drain the body (token auth is the protection now; the 2 GiB
    // DefaultBodyLimit layer still caps overall size).
    let mut body_data = Vec::new();
    let mut body = req.into_body();
    while let Some(Ok(frame)) = body.frame().await {
        if let Ok(data) = frame.into_data() {
            body_data.extend_from_slice(&data);
        }
    }

    let original_size = body_data.len();

    // CRC32
    let crc = crc32fast::hash(&body_data);

    // SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&body_data);
    let sha = hex::encode(hasher.finalize());

    // Zlib compress
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&body_data).unwrap_or(());
    let compressed = encoder.finish().unwrap_or_default();
    let compressed_size = compressed.len();

    let result = serde_json::json!({
        "original_size": original_size,
        "compressed_size": compressed_size,
        "crc32": format!("{crc:08x}"),
        "sha256": sha,
    });

    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(result))
}

#[derive(Deserialize)]
struct DelayedParams {
    ms: Option<u64>,
    #[allow(dead_code)]
    work: Option<String>,
}

/// GET /api/delayed?ms=N&work=light
/// Sleep N ms (clamped 1-100), return actual duration.
async fn api_delayed(Query(p): Query<DelayedParams>) -> impl IntoResponse {
    let t0 = Instant::now();
    let ms = p.ms.unwrap_or(10).clamp(1, 100);
    sleep(Duration::from_millis(ms)).await;
    let actual_ms = elapsed_ms(t0);

    let result = serde_json::json!({
        "requested_ms": ms,
        "actual_ms": (actual_ms * 100.0).round() / 100.0,
    });

    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(result))
}

#[derive(Deserialize)]
struct ValidateParams {
    seed: Option<u64>,
}

/// GET /api/validate?seed=42
/// Return checksums of all endpoint outputs for the given seed.
async fn api_validate(Query(p): Query<ValidateParams>) -> impl IntoResponse {
    let t0 = Instant::now();
    let seed = p.seed.unwrap_or(42);

    let result = if let Some(data) = load_bench_data() {
        // Use pre-computed checksums from shared dataset
        serde_json::json!({
            "seed": seed,
            "checksums": data.expected_checksums,
        })
    } else {
        fallback_validate_checksums(seed)
    };

    let dur_ms = elapsed_ms(t0);
    (bench_headers(dur_ms), Json(result))
}

/// PRNG-fallback checksum computation for dataset-less installs. Pure given
/// the seed — extracted from the handler so the hashing arithmetic is
/// unit-testable regardless of whether the test environment finds the shared
/// dataset (in-repo runs always do, which left this branch's 14 mutants
/// alive).
fn fallback_validate_checksums(seed: u64) -> serde_json::Value {
    {
        // Users: generate page=seed, hash the JSON
        let mut rng = StdRng::seed_from_u64(seed);
        let users: Vec<serde_json::Value> = (0..100)
            .map(|i| gen_user(&mut rng, (seed - 1) * 100 + i + 1))
            .collect();
        let users_json = serde_json::to_string(&users).unwrap_or_default();
        let users_hash = hex::encode(Sha256::digest(users_json.as_bytes()));

        // Aggregate: generate 10k points from seed, hash the stats
        let mut rng2 = StdRng::seed_from_u64(seed);
        let mut values: Vec<f64> = (0..10_000).map(|_| rng2.random::<f64>() * 1000.0).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let sum: f64 = values.iter().sum();
        let mean = sum / values.len() as f64;
        let agg_str = format!("{:.6}", mean);
        let aggregate_hash = hex::encode(Sha256::digest(agg_str.as_bytes()));

        // Transform: hash of SHA-256("test")
        let transform_check = hex::encode(Sha256::digest(b"test"));
        let transform_hash = hex::encode(Sha256::digest(transform_check.as_bytes()));

        // Search: hash the item list (seed=42 always)
        let mut rng3 = StdRng::seed_from_u64(42);
        let words: &[&str] = &[
            "network",
            "latency",
            "throughput",
            "bandwidth",
            "packet",
            "server",
            "client",
            "request",
            "response",
            "timeout",
            "connection",
            "socket",
            "protocol",
            "testing",
            "benchmark",
            "performance",
            "endpoint",
            "proxy",
            "firewall",
            "router",
            "switch",
            "gateway",
            "dns",
            "tls",
            "quic",
        ];
        let items: Vec<String> = (0..1_000)
            .map(|_| {
                let w1 = words[rng3.random_range(0..words.len())];
                let w2 = words[rng3.random_range(0..words.len())];
                let n: u32 = rng3.random_range(1..1000);
                format!("{w1}-{w2}-{n}")
            })
            .collect();
        let search_json = serde_json::to_string(&items).unwrap_or_default();
        let search_hash = hex::encode(Sha256::digest(search_json.as_bytes()));

        serde_json::json!({
            "seed": seed,
            "checksums": {
                "users": users_hash,
                "aggregate": aggregate_hash,
                "transform": transform_hash,
                "search": search_hash,
            },
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt; // for `oneshot`

    fn app() -> Router {
        app_with_token(None)
    }

    fn app_with_token(bench_token: Option<&str>) -> Router {
        build_router(AppState {
            h3_port: None,
            http_port: 8080,
            https_port: 8443,
            udp_port: 9999,
            udp_throughput_port: 9998,
            stamp_port: 9997,
            started_at: std::time::Instant::now(),
            system_meta: SystemMeta::collect(),
            bench_token: bench_token.map(str::to_owned),
        })
    }

    #[tokio::test]
    async fn landing_page_returns_html() {
        let resp = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("text/html"), "content-type must be text/html");
        let body = to_bytes(resp.into_body(), 32 * 1024).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("networker-endpoint"),
            "page must mention service name"
        );
        assert!(html.contains("/health"), "page must list /health endpoint");
        assert!(html.contains(":8080"), "page must show HTTP port");
    }

    #[tokio::test]
    async fn health_returns_200() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    /// /health self-reports the target's capabilities — running listeners by
    /// port, disabled ones as null — so launch flows offer exactly what this
    /// instance supports. A 0-configured port (e.g. `--stamp-port 0`,
    /// v0.28.170) must read as null, never as "port 0".
    #[tokio::test]
    async fn health_reports_live_service_capabilities() {
        let make = |stamp_port: u16| {
            build_router(AppState {
                h3_port: None,
                http_port: 8080,
                https_port: 8443,
                udp_port: 9999,
                udp_throughput_port: 9998,
                stamp_port,
                started_at: std::time::Instant::now(),
                system_meta: SystemMeta::collect(),
                bench_token: None,
            })
        };

        let body = |resp: Response| async move {
            let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };

        let enabled = body(
            make(9997)
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(enabled["status"], "ok");
        assert_eq!(enabled["services"]["stamp"], 9997);
        assert_eq!(enabled["services"]["udp_echo"], 9999);
        assert_eq!(enabled["services"]["udp_throughput"], 9998);
        assert_eq!(enabled["services"]["download"], true);
        assert_eq!(enabled["services"]["ws_echo"], true);
        assert!(
            enabled["services"]["h3"].is_null(),
            "no h3 listener configured"
        );

        let disabled = body(
            make(0)
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            disabled["services"]["stamp"].is_null(),
            "port 0 = disabled must self-report null, got {}",
            disabled["services"]["stamp"]
        );
    }

    #[tokio::test]
    async fn health_meets_orchestrator_contract_and_is_constant() {
        // API-SPEC.md §5.1: status/runtime/version required, body byte-constant.
        let mut bodies = Vec::new();
        for _ in 0..2 {
            let resp = app()
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            bodies.push(to_bytes(resp.into_body(), 4096).await.unwrap());
        }
        assert_eq!(bodies[0], bodies[1], "/health body must be constant-work");
        let v: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["runtime"], "rust");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn health_has_server_timestamp() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().contains_key("x-networker-server-timestamp"),
            "server timestamp header missing"
        );
    }

    #[tokio::test]
    async fn echo_returns_body() {
        let payload = b"hello world".as_ref();
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "text/plain")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], payload);
    }

    /// The /ws route exists and rejects a plain (non-upgrade) GET with a
    /// client error instead of 404 — the full upgrade + echo path is covered
    /// end-to-end by the tester's websocket integration test.
    #[tokio::test]
    async fn ws_route_rejects_non_upgrade_request() {
        let resp = app()
            .oneshot(Request::builder().uri("/ws").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(resp.status(), 404, "/ws route must be registered");
        assert!(
            resp.status().is_client_error(),
            "plain GET /ws should be a 4xx (missing upgrade headers), got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn download_returns_requested_bytes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download?bytes=256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body.len(), 256);
    }

    #[tokio::test]
    async fn download_path_form_returns_exact_fill_bytes() {
        // API-SPEC.md §5.2: canonical path form, 0x42 fill byte.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download/1024")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok()),
            Some("1024")
        );
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert_eq!(body.len(), 1024);
        assert!(
            body.iter().all(|&b| b == 0x42),
            "download fill byte must be 0x42"
        );
    }

    #[tokio::test]
    async fn download_path_form_rejects_non_integer() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn download_has_server_timing() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download?bytes=64")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().contains_key("server-timing"),
            "server-timing header missing from download"
        );
    }

    #[tokio::test]
    async fn upload_echoes_request_id() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .header("x-networker-request-id", "test-id-123")
                    .body(Body::from(b"data".as_ref()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("x-networker-request-id")
                .and_then(|v| v.to_str().ok()),
            Some("test-id-123"),
            "x-networker-request-id not echoed"
        );
        assert!(
            resp.headers().contains_key("server-timing"),
            "server-timing header missing from upload"
        );
    }

    #[tokio::test]
    async fn upload_returns_received_bytes_header() {
        let payload = b"hello world 12345";
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .body(Body::from(payload.as_ref()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let received: usize = resp
            .headers()
            .get("x-networker-received-bytes")
            .expect("x-networker-received-bytes header missing")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            received,
            payload.len(),
            "received-bytes header must match body size"
        );
    }

    #[tokio::test]
    async fn status_endpoint_returns_404() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/status/404")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn status_endpoint_returns_503() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/status/503")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn delay_endpoint_responds() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/delay?ms=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn http_version_responds() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/http-version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 512).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["version"].is_string());
    }

    #[tokio::test]
    async fn headers_endpoint_echoes_headers() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/headers")
                    .header("x-test-header", "networker")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["x-test-header"], "networker");
    }

    // ── JSON API benchmark endpoint tests ───────────────────────────────────

    #[tokio::test]
    async fn api_users_returns_20_items() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/users?page=1&sort=name&order=asc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(
            resp.headers().contains_key("server-timing"),
            "missing server-timing"
        );
        assert!(
            resp.headers().contains_key("cache-control"),
            "missing cache-control"
        );
        let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 20);
    }

    #[tokio::test]
    async fn api_users_is_deterministic() {
        let make_req = || {
            Request::builder()
                .uri("/api/users?page=5")
                .body(Body::empty())
                .unwrap()
        };
        let r1 = app().oneshot(make_req()).await.unwrap();
        let b1 = to_bytes(r1.into_body(), 64 * 1024).await.unwrap();
        let r2 = app().oneshot(make_req()).await.unwrap();
        let b2 = to_bytes(r2.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(b1, b2, "api/users must be deterministic for same seed");
    }

    #[tokio::test]
    async fn api_transform_hashes_and_reverses() {
        let body_json = serde_json::json!({
            "seed": 1,
            "fields": ["hello", "world"],
            "values": [1, 2, 3]
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/transform")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["reversed_values"], serde_json::json!([3, 2, 1]));
        assert_eq!(json["hashed_fields"].as_array().unwrap().len(), 2);
        // SHA-256 of "hello" is well-known
        assert_eq!(
            json["hashed_fields"][0].as_str().unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[tokio::test]
    async fn api_aggregate_returns_stats() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/aggregate?range=1,100")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_points"], 10_000);
        assert!(json["mean"].as_f64().is_some());
        assert!(json["p50"].as_f64().is_some());
        assert!(json["p95"].as_f64().is_some());
        assert_eq!(json["categories"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn api_search_returns_results() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/search?q=network&limit=5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["total_matches"].as_u64().unwrap() > 0);
        assert!(json["results"].as_array().unwrap().len() <= 5);
    }

    #[tokio::test]
    async fn api_upload_process_computes_hashes() {
        let payload = b"hello world benchmark test data";
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/upload/process")
                    .body(Body::from(payload.as_ref()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["original_size"], payload.len());
        assert!(json["compressed_size"].as_u64().unwrap() > 0);
        assert!(json["crc32"].as_str().unwrap().len() == 8);
        assert!(json["sha256"].as_str().unwrap().len() == 64);
    }

    #[tokio::test]
    async fn api_delayed_sleeps_at_least_requested() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/delayed?ms=10&work=light")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["requested_ms"], 10);
        assert!(json["actual_ms"].as_f64().unwrap() >= 9.0); // allow small timing slack
    }

    #[tokio::test]
    async fn api_delayed_clamps_to_100ms() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/delayed?ms=999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Clamped to 100
        assert_eq!(json["requested_ms"], 100);
    }

    #[tokio::test]
    async fn api_validate_returns_checksums() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/validate?seed=42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["seed"], 42);
        let checksums = &json["checksums"];
        assert!(checksums["users"].as_str().unwrap().len() == 64);
        assert!(checksums["aggregate"].as_str().unwrap().len() == 64);
        assert!(checksums["transform"].as_str().unwrap().len() == 64);
        assert!(checksums["search"].as_str().unwrap().len() == 64);
    }

    #[tokio::test]
    async fn api_validate_is_deterministic() {
        let make_req = || {
            Request::builder()
                .uri("/api/validate?seed=7")
                .body(Body::empty())
                .unwrap()
        };
        let r1 = app().oneshot(make_req()).await.unwrap();
        let b1 = to_bytes(r1.into_body(), 4 * 1024).await.unwrap();
        let r2 = app().oneshot(make_req()).await.unwrap();
        let b2 = to_bytes(r2.into_body(), 4 * 1024).await.unwrap();
        assert_eq!(b1, b2, "api/validate must be deterministic for same seed");
    }

    // ── Auth middleware tests ────────────────────────────────────────────────
    // BENCH_API_TOKEN is NOT set in the test process, so the middleware is
    // transparent. We verify that normal operation is unaffected and that the
    // auth;dur timing metric is appended.

    #[tokio::test]
    async fn api_auth_transparent_when_no_token() {
        // Without BENCH_API_TOKEN every endpoint should return 200 (auth disabled).
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/users?page=1&sort=name&order=asc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn api_auth_skips_health() {
        // /health must always succeed regardless of auth state.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn api_auth_timing_header() {
        // The auth middleware should append `auth;dur=` to Server-Timing.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/users?page=1&sort=name&order=asc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let st = resp
            .headers()
            .get("server-timing")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            st.contains("auth;dur="),
            "Server-Timing should contain auth metric, got: {st}"
        );
    }

    /// Verify all JSON API endpoints include required benchmark headers.
    #[tokio::test]
    async fn api_endpoints_include_benchmark_headers() {
        let endpoints = [
            "/api/users?page=1&sort=name&order=asc",
            "/api/aggregate?range=1,100",
            "/api/search?q=test&limit=5",
            "/api/delayed?ms=5&work=light",
            "/api/validate?seed=42",
        ];
        for uri in endpoints {
            let resp = app()
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), 200, "{uri} failed");
            let h = resp.headers();
            assert!(
                h.get("server-timing").is_some(),
                "{uri} missing Server-Timing"
            );
            assert!(
                h.get("cache-control")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .contains("no-store"),
                "{uri} missing Cache-Control: no-store"
            );
            assert!(
                h.get("timing-allow-origin").is_some(),
                "{uri} missing Timing-Allow-Origin"
            );
            assert!(
                h.get("access-control-allow-origin").is_some(),
                "{uri} missing Access-Control-Allow-Origin"
            );
        }
    }

    // ── page-load routes (previously untested — mutation-pilot gap) ──────────

    #[tokio::test]
    async fn page_manifest_lists_exactly_n_assets_with_requested_bytes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/page?assets=7&bytes=2048")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["asset_count"], 7);
        assert_eq!(v["asset_bytes"], 2048);
        let assets = v["assets"].as_array().unwrap();
        assert_eq!(assets.len(), 7, "manifest must list exactly N assets");
        assert_eq!(assets[0], "/asset?id=0&bytes=2048");
        assert_eq!(assets[6], "/asset?id=6&bytes=2048");
    }

    #[tokio::test]
    async fn page_manifest_clamps_assets_to_500() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/page?assets=9999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["asset_count"], 500, "asset count must clamp at 500");
    }

    #[tokio::test]
    async fn browser_page_renders_n_img_tags_pointing_at_assets() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/browser-page?assets=3&bytes=512")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/html"));
        let html = String::from_utf8(
            to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            html.matches("<img ").count(),
            3,
            "the browser load event depends on exactly N images"
        );
        assert!(html.contains("/asset?id=2&bytes=512"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[tokio::test]
    async fn asset_returns_exactly_the_requested_zero_bytes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/asset?id=1&bytes=3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/octet-stream")
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.len(), 3000, "asset must be exactly the requested size");
        assert!(body.iter().all(|&b| b == 0), "asset payload is a zero fill");
    }

    // ── pure helpers (previously untested — mutation-pilot gap) ──────────────

    // ── auth middleware with a token (previously untestable env-global) ──────

    #[tokio::test]
    async fn auth_rejects_missing_and_wrong_tokens_with_401() {
        for (desc, req) in [
            (
                "no header",
                Request::builder().uri("/info").body(Body::empty()).unwrap(),
            ),
            (
                "wrong token",
                Request::builder()
                    .uri("/info")
                    .header("authorization", "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            ),
            (
                "not a bearer",
                Request::builder()
                    .uri("/info")
                    .header("authorization", "Basic c3VwZXI=")
                    .body(Body::empty())
                    .unwrap(),
            ),
        ] {
            let resp = app_with_token(Some("s3cret")).oneshot(req).await.unwrap();
            assert_eq!(resp.status(), 401, "{desc} must be rejected");
            assert!(
                resp.headers()
                    .get("server-timing")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .contains("auth;dur="),
                "{desc}: 401s still carry the auth timing metric"
            );
        }
    }

    #[tokio::test]
    async fn auth_accepts_the_exact_token_and_appends_timing() {
        let resp = app_with_token(Some("s3cret"))
            .oneshot(
                Request::builder()
                    .uri("/info")
                    .header("authorization", "Bearer s3cret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(resp
            .headers()
            .get("server-timing")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("auth;dur="));
    }

    #[tokio::test]
    async fn auth_exempts_health_even_with_a_token_set() {
        let resp = app_with_token(Some("s3cret"))
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "LB probes must not need credentials");
    }

    // ── extracted pure logic (previously reachable only via env variance) ────

    #[test]
    fn parse_meminfo_extracts_the_requested_key_in_mib() {
        let meminfo =
            "MemTotal:       16309240 kB\nMemFree: 1029900 kB\nMemAvailable:    8210012 kB\n";
        assert_eq!(parse_meminfo_mb(meminfo, "MemTotal:"), Some(15926));
        assert_eq!(parse_meminfo_mb(meminfo, "MemAvailable:"), Some(8017));
        assert_eq!(parse_meminfo_mb(meminfo, "SwapTotal:"), None);
        assert_eq!(parse_meminfo_mb("MemTotal: garbage kB", "MemTotal:"), None);
        assert_eq!(parse_meminfo_mb("", "MemTotal:"), None);
    }

    #[test]
    fn fallback_users_is_deterministic_and_ids_follow_the_page() {
        let page1 = fallback_users(1);
        assert_eq!(page1.len(), 100);
        assert_eq!(page1[0]["id"], 1);
        assert_eq!(page1[99]["id"], 100);
        let page2 = fallback_users(2);
        assert_eq!(page2[0]["id"], 101, "page 2 continues the id sequence");
        // Deterministic per page; different across pages.
        assert_eq!(page1, fallback_users(1));
        assert_ne!(page1[0]["name"], page2[0]["name"]);
        // Shape sanity on a generated user (pins gen_user's stub mutant).
        let email = page1[0]["email"].as_str().unwrap();
        assert!(email.contains('@') && email.contains('.'));
        let score = page1[0]["score"].as_f64().unwrap();
        assert!(
            (0.0..=100.0).contains(&score),
            "score is 2-dp 0..100: {score}"
        );
    }

    #[test]
    fn elapsed_ms_converts_seconds_to_milliseconds() {
        // Every Server-Timing dur= value flows through this one conversion;
        // a *→/ mutation reports durations 1,000,000× off.
        let t0 = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let ms = elapsed_ms(t0);
        // Bounds tight enough to kill both operator mutations: `+` lands at
        // ~1000.02 (secs + 1000), `/` at ~0.00002 — both outside 10..500.
        assert!(
            (10.0..500.0).contains(&ms),
            "20ms sleep must read as ~20ms, got {ms}"
        );
    }

    #[tokio::test]
    async fn http_version_names_every_version_arm() {
        // oneshot lets us stamp any HTTP version on the request without a
        // real connection — deleting any match arm falls through to
        // "Unknown" and fails its case here.
        for (version, expect) in [
            (Version::HTTP_09, "HTTP/0.9"),
            (Version::HTTP_10, "HTTP/1.0"),
            (Version::HTTP_11, "HTTP/1.1"),
            (Version::HTTP_2, "HTTP/2"),
            (Version::HTTP_3, "HTTP/3"),
        ] {
            let mut req = Request::builder()
                .uri("/http-version")
                .body(Body::empty())
                .unwrap();
            *req.version_mut() = version;
            let resp = app().oneshot(req).await.unwrap();
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["version"], expect);
        }
    }

    #[test]
    fn gen_user_scores_span_the_two_decimal_0_to_100_range() {
        // Distribution asserts across a full page: an arithmetic mutation in
        // the score formula collapses the spread (all ≈100.0) or the scale.
        let users = fallback_users(1);
        let scores: Vec<f64> = users.iter().map(|u| u["score"].as_f64().unwrap()).collect();
        assert!(scores.iter().all(|s| (0.0..100.0).contains(s)));
        let (min, max) = scores
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        assert!(max - min > 10.0, "scores must spread, got [{min}, {max}]");
        assert!(
            scores.iter().any(|s| s.fract() != 0.0),
            "2-dp rounding must leave fractional scores"
        );
    }

    #[tokio::test]
    async fn fallback_aggregate_values_stay_in_the_0_to_1000_range() {
        // The lib-test env serves the PRNG fallback: 10k values in [0,1000).
        // A + mutation on the range scaling shifts everything to ~1000.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/aggregate")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let mean = v["mean"].as_f64().unwrap();
        assert!(
            (100.0..900.0).contains(&mean),
            "uniform [0,1000) mean must sit near 500, got {mean}"
        );
        assert!(v["max"].as_f64().unwrap() < 1000.0);
        assert!(v["p50"].as_f64().unwrap() < v["p95"].as_f64().unwrap());
    }

    #[test]
    fn fallback_validate_checksums_match_the_golden_values() {
        // GOLDEN values computed from the current implementation (seed=1):
        // any change to the hashed content — user-id arithmetic, value
        // scaling, float formatting — is a CONTRACT change and must show up
        // here deliberately, not drift silently.
        let v = fallback_validate_checksums(1);
        assert_eq!(
            v["checksums"]["users"],
            "c97ab1860ec7a71d790f2ed7b2d13d617b9dde34b28576e43aa64e9a55f52bcd"
        );
        assert_eq!(
            v["checksums"]["aggregate"],
            "e1a906250ac7ec3b83cbf773ddd2e1edde806a4af21c9d28c264d732affc81a8"
        );
        // seed=3: (seed-1)*100 is DEGENERATE at seed=1 (zero under several
        // arithmetic mutations), so a second golden pins the id offset term.
        let v3 = fallback_validate_checksums(3);
        assert_eq!(
            v3["checksums"]["users"],
            "caa760df5a392b4e757d7dee3725073e262e22426963237b2dc5286c9c1ab87e"
        );
    }

    #[tokio::test]
    async fn upload_streams_a_4mib_body_exactly() {
        // /upload drains the stream without buffering (DefaultBodyLimit never
        // engages there) — this pins the streaming byte count at real size.
        let payload = vec![0x41u8; 4 * 1024 * 1024];
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let received: usize = resp
            .headers()
            .get("x-networker-received-bytes")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(received, 4 * 1024 * 1024);
    }

    #[tokio::test]
    async fn body_limit_admits_a_4mib_json_body_on_the_buffering_extractor() {
        // DefaultBodyLimit (2 GiB) only fires through BUFFERING extractors.
        // /upload and /api/upload/process take the raw Request and stream/
        // read manually (verified by two surviving-mutant rounds) — the one
        // route that buffers through the limit is api_transform's
        // Json<TransformBody>. A cap-constant mutation shrinking the limit
        // to ~1-2 MiB turns this ~4 MiB JSON POST into a 413.
        let chunk = "x".repeat(1024);
        let values: Vec<String> = std::iter::repeat_n(chunk, 3900).collect();
        let body = serde_json::json!({"seed": 1, "fields": [], "values": values});
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/transform")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "~4 MiB must clear the 2 GiB body limit");
        let out = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reversed_values"].as_array().unwrap().len(), 3900);
    }

    #[tokio::test]
    async fn asset_and_download_sizes_are_exact_above_the_mutated_cap_range() {
        // 2 MiB asset + 4 MiB download: both far below the real caps
        // (100 MiB / 2 GiB) but above what cap-constant mutations shrink
        // them to (~1-2 MiB) — a clamped body fails the exact-length assert.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/asset?bytes=2097152")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.len(), 2_097_152);

        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download/4194304")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.len(), 4_194_304);
        assert!(body.iter().all(|&b| b == 0x42));
    }

    #[test]
    fn fallback_validate_checksums_is_seed_stable_and_seed_sensitive() {
        let a = fallback_validate_checksums(42);
        assert_eq!(a["seed"], 42);
        let sums = a["checksums"].as_object().unwrap();
        for key in ["users", "aggregate", "transform", "search"] {
            let h = sums[key].as_str().unwrap();
            assert_eq!(h.len(), 64, "{key} must be a hex SHA-256");
        }
        // Deterministic for a seed; users/aggregate move with the seed;
        // transform and search are seed-independent by contract.
        let b = fallback_validate_checksums(42);
        assert_eq!(a, b);
        let c = fallback_validate_checksums(7);
        assert_ne!(a["checksums"]["users"], c["checksums"]["users"]);
        assert_ne!(a["checksums"]["aggregate"], c["checksums"]["aggregate"]);
        assert_eq!(a["checksums"]["transform"], c["checksums"]["transform"]);
        assert_eq!(a["checksums"]["search"], c["checksums"]["search"]);
    }

    // ── api_users sort arms (deleting any arm must break ordering) ───────────

    async fn users_sorted_by(field: &str, order: &str) -> Vec<serde_json::Value> {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/users?page=1&sort={field}&order={order}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn api_users_sorts_by_every_field_in_both_orders() {
        for field in ["name", "email", "score", "created_at", "id"] {
            for order in ["asc", "desc"] {
                let users = users_sorted_by(field, order).await;
                assert_eq!(users.len(), 20);
                let in_order = users.windows(2).all(|w| {
                    let (a, b) = (&w[0][field], &w[1][field]);
                    let cmp = if field == "score" {
                        a.as_f64()
                            .unwrap()
                            .partial_cmp(&b.as_f64().unwrap())
                            .unwrap()
                    } else if field == "id" {
                        a.as_u64().unwrap().cmp(&b.as_u64().unwrap())
                    } else {
                        a.as_str().unwrap().cmp(b.as_str().unwrap())
                    };
                    if order == "asc" {
                        cmp != std::cmp::Ordering::Greater
                    } else {
                        cmp != std::cmp::Ordering::Less
                    }
                });
                assert!(in_order, "sort={field} order={order} is not ordered");
            }
        }
        // Distinct fields produce distinct leaders — a deleted sort arm falls
        // back to id-order and collapses this distinction.
        let by_name = users_sorted_by("name", "asc").await;
        let by_score = users_sorted_by("score", "asc").await;
        assert_ne!(
            by_name[0], by_score[0],
            "name-sorted and score-sorted pages must differ"
        );
    }

    /// The lib-test environment has NO dataset (only canonical_checksums.rs
    /// pins BENCH_DATA_PATH), so /api/users serves the PRNG fallback here:
    /// every page exists and ids continue across pages. The dataset branch's
    /// page-beyond-data-is-empty contract is asserted in
    /// tests/canonical_checksums.rs where the dataset is guaranteed present.
    #[tokio::test]
    async fn api_users_fallback_pages_continue_the_id_sequence() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/users?page=999&sort=id&order=asc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let users: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(users.len(), 20);
        assert_eq!(
            users[0]["id"], 99_801,
            "fallback page N starts at (N-1)*100 + 1"
        );
    }

    #[test]
    fn parse_load_avg_accepts_proc_and_sysctl_shapes() {
        // Linux /proc/loadavg: first token is the 1-minute average.
        assert_eq!(parse_load_avg_1m("0.52 0.58 0.59 1/467 12345"), Some(0.52));
        // macOS sysctl vm.loadavg: "{ 1.23 1.50 1.61 }" — the '{' token is
        // skipped, the first numeric token wins.
        assert_eq!(parse_load_avg_1m("{ 1.23 1.50 1.61 }"), Some(1.23));
        // Garbage, negatives and non-finite values are rejected, not zeroed.
        assert_eq!(parse_load_avg_1m("not a load"), None);
        assert_eq!(parse_load_avg_1m("-1.0 0.0 0.0"), None);
        assert_eq!(parse_load_avg_1m(""), None);
    }

    #[test]
    fn format_uptime_picks_the_right_granularity_per_magnitude() {
        assert_eq!(format_uptime(42), "42s");
        assert_eq!(format_uptime(60), "1m 0s");
        assert_eq!(format_uptime(3_723), "1h 2m 3s");
        // Days drop the seconds — 2d 3h 4m (and 5s discarded).
        assert_eq!(
            format_uptime(2 * 86_400 + 3 * 3_600 + 4 * 60 + 5),
            "2d 3h 4m"
        );
    }

    // ── /info contract ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn info_reports_identity_version_and_uptime() {
        let resp = app()
            .oneshot(Request::builder().uri("/info").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["service"], "networker-endpoint");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert!(v["uptime_secs"].is_u64());
        let endpoints = v["endpoints"].as_array().unwrap();
        assert!(
            endpoints.iter().any(|e| e == "/api/validate"),
            "/info must advertise the api surface"
        );
    }

    mod parser_props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Both extracted parsers are total: arbitrary text (including
            /// multi-byte unicode) never panics.
            #[test]
            fn parsers_never_panic(s in ".{0,128}") {
                let _ = parse_load_avg_1m(&s);
                let _ = parse_meminfo_mb(&s, "MemTotal:");
            }

            /// Generated /proc/loadavg lines round-trip: the leading value
            /// comes back within float-formatting tolerance.
            #[test]
            fn loadavg_round_trips(v in 0.0f64..1000.0) {
                let line = format!("{v:.2} 0.58 0.59 1/467 12345");
                let parsed = parse_load_avg_1m(&line).unwrap();
                prop_assert!((parsed - v).abs() < 0.005 + v * 1e-9);
            }

            /// Generated meminfo buffers round-trip: kB → MiB is exact
            /// integer division, key order and unrelated lines irrelevant.
            #[test]
            fn meminfo_round_trips(
                kb in 0u64..=u64::MAX / 2,
                noise in prop::sample::select(vec!["", "MemFree: 12 kB\n", "SwapTotal: 0 kB\n"]),
            ) {
                let buf = format!("{noise}MemTotal: {kb} kB\nMemAvailable: 1 kB\n");
                prop_assert_eq!(parse_meminfo_mb(&buf, "MemTotal:"), Some(kb / 1024));
            }
        }
    }
}
