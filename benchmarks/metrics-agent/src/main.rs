//! Lightweight metrics collection agent for AletheBench.
//!
//! Runs an HTTP server on port 9100 exposing system and per-process metrics
//! as JSON. Designed to run alongside the server under test with minimal
//! resource overhead.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sysinfo::{Networks, Pid, System};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct MetricsState {
    system: System,
    networks: Networks,
    /// Previous network snapshot for delta computation.
    prev_net_rx: u64,
    prev_net_tx: u64,
    /// Previous disk snapshot for delta computation.
    prev_disk_read: u64,
    prev_disk_write: u64,
}

type SharedState = Arc<RwLock<MetricsState>>;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize, serde::Deserialize)]
struct SystemMetrics {
    cpu_percent: f32,
    memory_rss_bytes: u64,
    memory_total_bytes: u64,
    disk_read_bytes_sec: u64,
    disk_write_bytes_sec: u64,
    net_rx_bytes_sec: u64,
    net_tx_bytes_sec: u64,
    uptime_secs: u64,
}

#[derive(Serialize, serde::Deserialize)]
struct ProcessMetrics {
    pid: u32,
    name: String,
    cpu_percent: f32,
    memory_rss_bytes: u64,
    memory_virtual_bytes: u64,
    thread_count: Option<u64>,
    open_fds: Option<u64>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn system_metrics(State(state): State<SharedState>) -> Json<SystemMetrics> {
    let s = state.read().await;
    let sys = &s.system;

    let memory_used: u64 = sys.used_memory();
    let memory_total: u64 = sys.total_memory();

    Json(SystemMetrics {
        cpu_percent: sys.global_cpu_usage(),
        memory_rss_bytes: memory_used,
        memory_total_bytes: memory_total,
        disk_read_bytes_sec: s.prev_disk_read,
        disk_write_bytes_sec: s.prev_disk_write,
        net_rx_bytes_sec: s.prev_net_rx,
        net_tx_bytes_sec: s.prev_net_tx,
        uptime_secs: System::uptime(),
    })
}

async fn process_metrics(
    State(state): State<SharedState>,
    Path(pid): Path<u32>,
) -> impl IntoResponse {
    let s = state.read().await;
    let sys = &s.system;

    let pid_key = Pid::from_u32(pid);
    match sys.process(pid_key) {
        Some(proc) => {
            let open_fds = read_open_fds(pid);
            let thread_count = read_thread_count(pid);

            Ok(Json(ProcessMetrics {
                pid,
                name: proc.name().to_string_lossy().into_owned(),
                cpu_percent: proc.cpu_usage(),
                memory_rss_bytes: proc.memory(),
                memory_virtual_bytes: proc.virtual_memory(),
                thread_count,
                open_fds,
            }))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            format!("process {pid} not found"),
        )),
    }
}

// ---------------------------------------------------------------------------
// /proc helpers (Linux only, graceful no-op elsewhere)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn read_open_fds(pid: u32) -> Option<u64> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .ok()
        .map(|entries| entries.count() as u64)
}

#[cfg(not(target_os = "linux"))]
fn read_open_fds(_pid: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn read_thread_count(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Threads:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_thread_count(_pid: u32) -> Option<u64> {
    None
}

// ---------------------------------------------------------------------------
// Background refresh task
// ---------------------------------------------------------------------------

async fn refresh_loop(state: SharedState) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;

        let mut s = state.write().await;

        // Refresh CPU, memory, processes, and disks.
        s.system.refresh_all();
        s.networks.refresh(true);

        // Compute network deltas (bytes received/transmitted since last tick).
        let (mut total_rx, mut total_tx) = (0u64, 0u64);
        for (_name, data) in s.networks.iter() {
            total_rx += data.received();
            total_tx += data.transmitted();
        }
        s.prev_net_rx = total_rx;
        s.prev_net_tx = total_tx;

        // Compute disk I/O deltas.
        let mut disk_read = 0u64;
        let mut disk_write = 0u64;
        for proc in s.system.processes().values() {
            let usage = proc.disk_usage();
            disk_read += usage.read_bytes;
            disk_write += usage.written_bytes;
        }
        s.prev_disk_read = disk_read;
        s.prev_disk_write = disk_write;
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("METRICS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9100);

    let mut sys = System::new_all();
    sys.refresh_all();

    let state: SharedState = Arc::new(RwLock::new(MetricsState {
        system: sys,
        networks: Networks::new_with_refreshed_list(),
        prev_net_rx: 0,
        prev_net_tx: 0,
        prev_disk_read: 0,
        prev_disk_write: 0,
    }));

    // Spawn background refresh.
    tokio::spawn(refresh_loop(state.clone()));

    let app = Router::new()
        .route("/metrics", get(system_metrics))
        .route("/metrics/process/{pid}", get(process_metrics))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind metrics port");

    eprintln!("metrics-agent listening on 0.0.0.0:{port}");
    axum::serve(listener, app).await.expect("server error");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state() -> SharedState {
        let mut sys = System::new_all();
        sys.refresh_all();
        // Double-refresh to ensure CPU usage is populated (sysinfo needs two
        // data points) and the current process is visible.
        std::thread::sleep(std::time::Duration::from_millis(200));
        sys.refresh_all();
        Arc::new(RwLock::new(MetricsState {
            system: sys,
            networks: Networks::new_with_refreshed_list(),
            prev_net_rx: 0,
            prev_net_tx: 0,
            prev_disk_read: 0,
            prev_disk_write: 0,
        }))
    }

    fn app(state: SharedState) -> Router {
        Router::new()
            .route("/metrics", get(system_metrics))
            .route("/metrics/process/{pid}", get(process_metrics))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_system_metrics_endpoint() {
        let state = test_state();
        let app = app(state);

        let req: Request<Body> = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let metrics: SystemMetrics = serde_json::from_slice(&body).unwrap();

        assert!(metrics.memory_total_bytes > 0);
        assert!(metrics.uptime_secs > 0);
    }

    #[tokio::test]
    async fn test_process_metrics_known_pid() {
        let state = test_state();

        // Pick any PID that sysinfo actually knows about.
        let known_pid = {
            let s = state.read().await;
            s.system
                .processes()
                .keys()
                .next()
                .map(|p| p.as_u32())
                .expect("sysinfo should see at least one process")
        };

        let app = app(state);

        let req: Request<Body> = Request::builder()
            .uri(format!("/metrics/process/{known_pid}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let metrics: ProcessMetrics = serde_json::from_slice(&body).unwrap();

        assert_eq!(metrics.pid, known_pid);
        assert!(metrics.memory_rss_bytes > 0);
    }

    #[tokio::test]
    async fn test_process_not_found() {
        let state = test_state();
        let app = app(state);

        // PID 999999999 should not exist.
        let req: Request<Body> = Request::builder()
            .uri("/metrics/process/999999999")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_open_fds_no_panic() {
        // Should return None on non-Linux or for nonexistent PID.
        let _ = read_open_fds(999_999_999);
    }

    #[test]
    fn test_thread_count_no_panic() {
        let _ = read_thread_count(999_999_999);
    }
}
