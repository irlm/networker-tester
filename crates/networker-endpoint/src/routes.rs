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
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write as IoWrite;
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
    pub started_at: Instant,
    pub system_meta: SystemMeta,
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

fn detect_total_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb / 1024);
            }
        }
        None
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

/// Build the router.
///
/// `state.h3_port` — when `Some(port)`, every response includes
/// `Alt-Svc: h3=":port"; ma=86400` so that Chrome can discover H3 support
/// and upgrade to QUIC on subsequent navigations.  Pass `None` when H3 is not
/// compiled in (the `http3` feature is disabled).
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(landing_page))
        .route("/health", get(health))
        .route("/echo", post(echo).get(echo_get))
        .route("/download", get(download))
        .route("/upload", post(upload))
        .route("/delay", get(delay))
        .route("/headers", get(headers_echo))
        .route("/status/:code", get(status_code))
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
// Context-switch helpers (Unix only)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `(voluntary_csw, involuntary_csw)` for the server process.
#[cfg(unix)]
fn csw_snapshot() -> (i64, i64) {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    (u.ru_nvcsw, u.ru_nivcsw)
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
         </div>\n",
        http_port = state.http_port,
        https_port = state.https_port,
        h3_port_display = h3_port_display,
        udp_port = state.udp_port,
        udp_tp_port = state.udp_throughput_port,
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

/// GET /health → 200 JSON { "status": "ok", "timestamp": "..." }
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "timestamp": Utc::now().to_rfc3339(),
        "service": "networker-endpoint",
        "version": env!("CARGO_PKG_VERSION"),
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

#[derive(Deserialize)]
struct DownloadParams {
    bytes: Option<usize>,
}

/// GET /download?bytes=N – streams N zero bytes (max 2 GiB) in 64 KiB chunks.
/// Adds `Server-Timing: proc;dur=X, csw-v;dur=N, csw-i;dur=N` indicating
/// setup time and context switches.
async fn download(Query(p): Query<DownloadParams>) -> impl IntoResponse {
    let n = p.bytes.unwrap_or(1024).min(2 * 1024 * 1024 * 1024); // cap 2 GiB
    let t0 = Instant::now();
    #[cfg(unix)]
    let (csw_v0, csw_i0) = csw_snapshot();

    // Stream zero bytes in fixed-size chunks to avoid allocating the full
    // payload in memory at once (critical for multi-GiB downloads).
    const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB
    let full_chunks = n / CHUNK_SIZE;
    let remainder = n % CHUNK_SIZE;
    let zero_chunk = Bytes::from(vec![0u8; CHUNK_SIZE]);

    let body = Body::from_stream(futures::stream::iter(
        (0..full_chunks)
            .map(move |_| Ok::<_, std::io::Error>(zero_chunk.clone()))
            .chain(if remainder > 0 {
                vec![Ok(Bytes::from(vec![0u8; remainder]))].into_iter()
            } else {
                vec![].into_iter()
            }),
    ));

    let proc_ms = t0.elapsed().as_secs_f64() * 1000.0;
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
    let mut received_bytes: usize = 0;
    let mut body = req.into_body();
    while let Some(Ok(frame)) = body.frame().await {
        if let Ok(data) = frame.into_data() {
            received_bytes += data.len();
        }
    }
    let recv_ms = t0.elapsed().as_secs_f64() * 1000.0;
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
    Json(serde_json::json!({
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
    }))
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
    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", n.to_string())
        .body(Body::from(vec![0u8; n]))
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
    let first = FIRST_NAMES[rng.gen_range(0..FIRST_NAMES.len())];
    let last = LAST_NAMES[rng.gen_range(0..LAST_NAMES.len())];
    let domain = DOMAINS[rng.gen_range(0..DOMAINS.len())];
    let score: f64 = (rng.gen::<f64>() * 10000.0).round() / 100.0;
    let day = rng.gen_range(1u32..29);
    let month = rng.gen_range(1u32..13);
    let year = rng.gen_range(2018u32..2026);
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
/// Generate 100 users from deterministic PRNG, sort, paginate to 20.
async fn api_users(Query(p): Query<UsersParams>) -> impl IntoResponse {
    let t0 = Instant::now();
    let page = p.page.unwrap_or(1).max(1);
    let sort_field = p.sort.as_deref().unwrap_or("id");
    let ascending = p.order.as_deref().unwrap_or("asc") != "desc";

    let mut rng = StdRng::seed_from_u64(page);
    let mut users: Vec<serde_json::Value> = (0..100)
        .map(|i| gen_user(&mut rng, (page - 1) * 100 + i + 1))
        .collect();

    // Sort by requested field
    users.sort_by(|a, b| {
        let cmp = match sort_field {
            "name" => a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(a["name"].as_str().unwrap_or(""))
                .then(
                    a["name"]
                        .as_str()
                        .unwrap_or("")
                        .cmp(b["name"].as_str().unwrap_or("")),
                ),
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
    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
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
            format!("{:x}", hasher.finalize())
        })
        .collect();

    let mut reversed_values = body.values.unwrap_or_default();
    reversed_values.reverse();

    let result = serde_json::json!({
        "seed": body.seed.unwrap_or(0),
        "hashed_fields": hashed_fields,
        "reversed_values": reversed_values,
    });

    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
    (bench_headers(dur_ms), Json(result))
}

#[derive(Deserialize)]
struct AggregateParams {
    range: Option<String>,
}

/// GET /api/aggregate?range=start,end
/// Generate 10,000 time-series points, compute stats, group into 5 categories.
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

    let mut rng = StdRng::seed_from_u64(start);
    let mut values: Vec<f64> = (0..10_000).map(|_| rng.gen::<f64>() * 1000.0).collect();
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

    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
    (bench_headers(dur_ms), Json(result))
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    limit: Option<usize>,
}

/// GET /api/search?q=term&limit=N
/// Generate 1,000 items from PRNG, filter by regex, score by match position.
async fn api_search(Query(p): Query<SearchParams>) -> impl IntoResponse {
    let t0 = Instant::now();
    let query = p.q.as_deref().unwrap_or("test");
    let limit = p.limit.unwrap_or(20).min(100);

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

    let items: Vec<String> = (0..1_000)
        .map(|_| {
            let w1 = words[rng.gen_range(0..words.len())];
            let w2 = words[rng.gen_range(0..words.len())];
            let n: u32 = rng.gen_range(1..1000);
            format!("{w1}-{w2}-{n}")
        })
        .collect();

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

    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
    (bench_headers(dur_ms), Json(result))
}

/// POST /api/upload/process
/// Read body, compute CRC32 + SHA-256, compress with zlib.
async fn api_upload_process(req: Request) -> impl IntoResponse {
    let t0 = Instant::now();

    // Drain the body
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
    let sha = format!("{:x}", hasher.finalize());

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

    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
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
    let actual_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let result = serde_json::json!({
        "requested_ms": ms,
        "actual_ms": (actual_ms * 100.0).round() / 100.0,
    });

    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
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

    // Users: generate page=seed, hash the JSON
    let mut rng = StdRng::seed_from_u64(seed);
    let users: Vec<serde_json::Value> = (0..100)
        .map(|i| gen_user(&mut rng, (seed - 1) * 100 + i + 1))
        .collect();
    let users_json = serde_json::to_string(&users).unwrap_or_default();
    let users_hash = format!("{:x}", Sha256::digest(users_json.as_bytes()));

    // Aggregate: generate 10k points from seed, hash the stats
    let mut rng2 = StdRng::seed_from_u64(seed);
    let mut values: Vec<f64> = (0..10_000).map(|_| rng2.gen::<f64>() * 1000.0).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sum: f64 = values.iter().sum();
    let mean = sum / values.len() as f64;
    let agg_str = format!("{:.6}", mean);
    let aggregate_hash = format!("{:x}", Sha256::digest(agg_str.as_bytes()));

    // Transform: hash of SHA-256("test")
    let transform_check = format!("{:x}", Sha256::digest(b"test"));
    let transform_hash = format!("{:x}", Sha256::digest(transform_check.as_bytes()));

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
            let w1 = words[rng3.gen_range(0..words.len())];
            let w2 = words[rng3.gen_range(0..words.len())];
            let n: u32 = rng3.gen_range(1..1000);
            format!("{w1}-{w2}-{n}")
        })
        .collect();
    let search_json = serde_json::to_string(&items).unwrap_or_default();
    let search_hash = format!("{:x}", Sha256::digest(search_json.as_bytes()));

    let result = serde_json::json!({
        "seed": seed,
        "checksums": {
            "users": users_hash,
            "aggregate": aggregate_hash,
            "transform": transform_hash,
            "search": search_hash,
        },
    });

    let dur_ms = t0.elapsed().as_secs_f64() * 1000.0;
    (bench_headers(dur_ms), Json(result))
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
        build_router(AppState {
            h3_port: None,
            http_port: 8080,
            https_port: 8443,
            udp_port: 9999,
            udp_throughput_port: 9998,
            started_at: std::time::Instant::now(),
            system_meta: SystemMeta::collect(),
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
}
