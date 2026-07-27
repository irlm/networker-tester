//! Multi-connection throughput probe (`mthroughput` mode) — link capacity
//! the way adaptive speed tests measure it.
//!
//! # Why a second throughput mode
//!
//! The single-connection `download`/`upload` modes measure ONE TCP flow's
//! fair share (ndt7 methodology). Adaptive multi-connection tests (Ookla and
//! peers) open several parallel flows and report their aggregate — link
//! capacity. The two answer different questions and read consistently
//! different on high-latency/lossy paths, where a single cubic flow cannot
//! fill the pipe within a bounded transfer (SIGMETRICS '23 comparative
//! study). Reporting both, plus per-flow TCP attribution, makes the delta
//! explainable instead of confusing.
//!
//! # Methodology
//!
//! Per direction (download first, then upload — both stages measured):
//!
//! 1. **Ramp.** Start 1 HTTP/2 connection streaming the endpoint's
//!    `/download` (`/upload`) route; every 1 s interval add 1 connection (cap
//!    8) until the AGGREGATE goodput's moving average stabilizes — stddev of
//!    the last 4 moving-average values < 5% of the current average, the same
//!    criterion `responsiveness` uses (shared `load_gen` machinery).
//! 2. **Steady measure window.** Hold the connection count fixed for 4 more
//!    intervals. `capacity_mbps` and every per-connection figure come from
//!    this window only, so slow-start ramp is excluded from the headline.
//! 3. **Per-connection TCP attribution.** Each connection's fd is `dup(2)`'d
//!    before the TLS/hyper handover ([`SocketProbe`]); after the stage the
//!    kernel's post-transfer `tcp_info` is sampled per connection and the
//!    Wave-S busy/rwnd/sndbuf chronograph triad is folded into a verdict:
//!    rwnd-limited / sndbuf-limited / path-limited / unobserved. The triad is
//!    SEND-side, so it attests the data direction for the UPLOAD stage; on
//!    download the sender is the endpoint (kernel unreadable from here) and
//!    verdicts describe only the request/ACK flow — documented on the field,
//!    never silently conflated.
//!
//! # Time-boxing (documented deviation from the fixed-payload modes)
//!
//! Stages are TIME-boxed (15 s cap per direction; typically shorter once
//! saturation is declared), not payload-sized: `--payload-sizes` does not
//! apply. Each connection re-issues 32 MiB transfers back-to-back for as
//! long as its stage runs.
//!
//! Honesty guard: if the load moved 0 bytes the direction FAILS instead of
//! reporting a fabricated 0-capacity link (same class as `rpm`'s `load_ok`
//! and `responsiveness`' zero-byte guard).

use crate::metrics::{
    ErrorCategory, ErrorRecord, MthroughputConn, MthroughputDirection, MthroughputResult, Protocol,
    RequestAttempt, SocketStats,
};
use crate::runner::load_gen::{
    connect_h2, load_download_once, load_upload_once, mean, stddev, H2Target, LoadDirection,
};
use crate::runner::socket_info::SocketProbe;
use crate::runner::throughput::ThroughputConfig;
use chrono::Utc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Interval duration (ms) — same 1 s cadence as the responsiveness ramp.
pub const DEFAULT_INTERVAL_MS: u64 = 1_000;
/// Connections at stage start.
pub const DEFAULT_INITIAL_CONNECTIONS: u32 = 1;
/// Connections added per interval until saturation.
pub const DEFAULT_ADD_PER_INTERVAL: u32 = 1;
/// Connection cap. 8 parallel flows is the adaptive-speed-test norm and
/// bounds the worst-case ramp (8 s) inside the stage time box.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 8;
/// Moving-average distance (intervals) for the stability criterion AND the
/// steady measure window length.
pub const DEFAULT_MAD: usize = 4;
/// Standard-deviation tolerance for saturation (fraction of current average).
pub const DEFAULT_SDT: f64 = 0.05;
/// Per-direction wall-clock cap (ms) — the stage is time-boxed, not
/// payload-sized.
pub const DEFAULT_MAX_DURATION_MS: u64 = 15_000;
/// Bytes per load request (re-issued back-to-back per connection).
pub const DEFAULT_LOAD_REQUEST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MthroughputConfig {
    /// Endpoint base URL (scheme selects h2-over-TLS vs h2c).
    pub base_url: url::Url,
    pub insecure: bool,
    pub ca_bundle: Option<String>,
    /// Per-connect timeout (ms).
    pub timeout_ms: u64,
    /// Interval duration (ms).
    pub interval_ms: u64,
    pub initial_connections: u32,
    pub add_per_interval: u32,
    pub max_connections: u32,
    /// Moving-average distance (intervals); also the measure-window length.
    pub mad: usize,
    /// Standard-deviation tolerance (fraction, 0.05 = 5 %).
    pub sdt: f64,
    /// Per-direction wall-clock cap (ms).
    pub max_duration_ms: u64,
    /// Bytes per load request.
    pub load_request_bytes: usize,
}

impl MthroughputConfig {
    /// Build from the resolved throughput config (same base URL + TLS trust
    /// settings every other endpoint mode uses).
    pub fn from_parts(throughput: &ThroughputConfig) -> Self {
        Self {
            base_url: throughput.base_url.clone(),
            insecure: throughput.run_cfg.insecure,
            ca_bundle: throughput.run_cfg.ca_bundle.clone(),
            timeout_ms: throughput.run_cfg.timeout_ms,
            interval_ms: DEFAULT_INTERVAL_MS,
            initial_connections: DEFAULT_INITIAL_CONNECTIONS,
            add_per_interval: DEFAULT_ADD_PER_INTERVAL,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            mad: DEFAULT_MAD,
            sdt: DEFAULT_SDT,
            max_duration_ms: DEFAULT_MAX_DURATION_MS,
            load_request_bytes: DEFAULT_LOAD_REQUEST_BYTES,
        }
    }

    fn h2_target(&self) -> H2Target {
        H2Target {
            base_url: self.base_url.clone(),
            insecure: self.insecure,
            ca_bundle: self.ca_bundle.clone(),
            timeout_ms: self.timeout_ms,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_mthroughput_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &MthroughputConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    // ── Download stage ───────────────────────────────────────────────────────
    let download = match run_direction(cfg, LoadDirection::Download).await {
        Ok(d) => d,
        Err(msg) => {
            return mthroughput_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                format!("Download stage: {msg}"),
            )
        }
    };

    // ── Upload stage — sequential, never fabricated on failure ───────────────
    let (upload, upload_error) = match run_direction(cfg, LoadDirection::Upload).await {
        Ok(u) => (Some(u), None),
        Err(msg) => (None, Some(format!("Upload stage: {msg}"))),
    };

    let result = MthroughputResult {
        remote_addr: cfg.base_url.to_string(),
        capacity_down_mbps: download.capacity_mbps,
        capacity_up_mbps: upload.as_ref().and_then(|u| u.capacity_mbps),
        conns_down: download.connections,
        conns_up: upload.as_ref().map(|u| u.connections),
        fair_share_spread_down_pct: download.fair_share_spread_pct,
        fair_share_spread_up_pct: upload.as_ref().and_then(|u| u.fair_share_spread_pct),
        download,
        upload,
        upload_error,
        started_at,
    };

    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Mthroughput,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: true,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
        ping: None,
        path: None,
        dualstack: None,
        websocket: None,
        pmtud: None,
        responsiveness: None,
        stamp: None,
        mthroughput: Some(result),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Direction engine
// ─────────────────────────────────────────────────────────────────────────────

/// One load connection: its own byte counter (per-flow goodput) and the
/// dup'd socket handle for post-transfer kernel stats.
struct Conn {
    bytes: Arc<AtomicU64>,
    probe: Option<SocketProbe>,
    task: tokio::task::JoinHandle<()>,
}

async fn run_direction(
    cfg: &MthroughputConfig,
    direction: LoadDirection,
) -> Result<MthroughputDirection, String> {
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut conns: Vec<Conn> = Vec::new();

    // ── Initial connections ──────────────────────────────────────────────────
    for _ in 0..cfg.initial_connections.max(1) {
        match spawn_load_connection(cfg, direction, stop_rx.clone()).await {
            Ok(conn) => conns.push(conn),
            Err(e) => {
                stop_and_reap(&stop_tx, &mut conns).await;
                return Err(format!("initial load connection failed: {e}"));
            }
        }
    }

    // ── Ramp to aggregate saturation, then a fixed steady measure window ─────
    let started = Instant::now();
    let interval = Duration::from_millis(cfg.interval_ms.max(50));
    let interval_s = interval.as_secs_f64();
    let cap = Duration::from_millis(cfg.max_duration_ms.max(cfg.interval_ms));
    let mad = cfg.mad.max(2);

    let mut goodputs: Vec<f64> = Vec::new(); // aggregate bytes/s per interval
    let mut moving_avgs: Vec<f64> = Vec::new();
    // Per-tick snapshot of each connection's cumulative counter — the basis
    // for the per-connection measure-window goodput.
    let mut snapshots: Vec<Vec<u64>> = Vec::new();
    let mut last_total = 0u64;
    let mut saturation_reached = false;
    let mut ramp_duration = Duration::ZERO;
    let mut measure_ticks_left: Option<usize> = None;
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // first tick fires immediately — consume it

    loop {
        tick.tick().await;

        let per_conn_totals: Vec<u64> = conns
            .iter()
            .map(|c| c.bytes.load(Ordering::Relaxed))
            .collect();
        let total: u64 = per_conn_totals.iter().sum();
        snapshots.push(per_conn_totals);
        let delta = total.saturating_sub(last_total);
        last_total = total;
        goodputs.push(delta as f64 / interval_s);
        let ma_window = &goodputs[goodputs.len().saturating_sub(mad)..];
        let current_ma = mean(ma_window);
        moving_avgs.push(current_ma);

        if let Some(left) = measure_ticks_left {
            // Steady measure window: connection count is frozen.
            if left <= 1 {
                break;
            }
            measure_ticks_left = Some(left - 1);
        } else if moving_avgs.len() >= mad
            && current_ma > 0.0
            && stddev(&moving_avgs[moving_avgs.len() - mad..]) < cfg.sdt * current_ma
        {
            // Aggregate goodput stabilized — freeze the connection count and
            // measure a clean window at it.
            saturation_reached = true;
            ramp_duration = started.elapsed();
            measure_ticks_left = Some(mad);
        } else if (conns.len() as u32) < cfg.max_connections {
            for _ in 0..cfg.add_per_interval {
                if conns.len() as u32 >= cfg.max_connections {
                    break;
                }
                match spawn_load_connection(cfg, direction, stop_rx.clone()).await {
                    Ok(conn) => conns.push(conn),
                    Err(e) => {
                        debug!("mthroughput: ramp connection failed: {e}");
                        break;
                    }
                }
            }
        }

        if started.elapsed() >= cap {
            break;
        }
    }

    let load_duration = started.elapsed();
    if !saturation_reached {
        ramp_duration = load_duration;
    }
    let intervals = goodputs.len() as u32;

    // ── Stop the load, then sample each connection's post-transfer stats ─────
    stop_and_reap(&stop_tx, &mut conns).await;

    let bytes_transferred: u64 = conns.iter().map(|c| c.bytes.load(Ordering::Relaxed)).sum();
    if bytes_transferred == 0 {
        return Err(format!(
            "load moved 0 bytes over {} connection(s) — refusing to report a \
             0-capacity link as a measurement",
            conns.len()
        ));
    }

    // Measure window: the last `mad` snapshots when saturation was reached
    // (the frozen steady window), otherwise whatever final window exists —
    // flagged by `saturation_reached: false`, never silently upgraded.
    let window_ticks = mad.min(snapshots.len().saturating_sub(1));
    let window_secs = window_ticks as f64 * interval_s;
    let (capacity_mbps, per_conn_mbps) = if window_ticks == 0 || window_secs <= 0.0 {
        (None, Vec::new())
    } else {
        let end = &snapshots[snapshots.len() - 1];
        let start = &snapshots[snapshots.len() - 1 - window_ticks];
        let per_conn: Vec<f64> = (0..conns.len())
            .map(|i| {
                let e = end.get(i).copied().unwrap_or(0);
                // Connections spawned after the window start have no start
                // snapshot column — they moved everything inside the window.
                let s = start.get(i).copied().unwrap_or(0);
                e.saturating_sub(s) as f64 / window_secs / 1e6
            })
            .collect();
        let aggregate: f64 = per_conn.iter().sum();
        ((aggregate > 0.0).then_some(aggregate), per_conn)
    };

    // Fair-share spread over the per-connection window goodputs.
    let (per_conn_min, per_conn_max, per_conn_mean, spread_pct) = if per_conn_mbps.is_empty() {
        (None, None, None, None)
    } else {
        let min = per_conn_mbps.iter().copied().fold(f64::INFINITY, f64::min);
        let max = per_conn_mbps
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let mean_v = mean(&per_conn_mbps);
        let spread = (mean_v > 0.0).then(|| (max - min) / mean_v * 100.0);
        (Some(min), Some(max), Some(mean_v), spread)
    };

    // Per-connection TCP attribution from the dup'd sockets.
    let mut per_conn = Vec::with_capacity(conns.len());
    let mut rwnd_limited_conns = 0u32;
    let mut sndbuf_limited_conns = 0u32;
    let mut path_limited_conns = 0u32;
    let mut unobserved_conns = 0u32;
    for (i, conn) in conns.iter().enumerate() {
        let stats = conn.probe.as_ref().and_then(|p| p.stats_for_result());
        let verdict = limited_verdict(stats.as_ref());
        match verdict {
            Verdict::RwndLimited(_) => rwnd_limited_conns += 1,
            Verdict::SndbufLimited(_) => sndbuf_limited_conns += 1,
            Verdict::PathLimited => path_limited_conns += 1,
            Verdict::Unobserved => unobserved_conns += 1,
        }
        per_conn.push(MthroughputConn {
            conn: i as u32,
            mbps: per_conn_mbps.get(i).copied().unwrap_or(0.0),
            verdict: verdict.to_string(),
            retrans: stats.as_ref().and_then(|s| s.total_retrans),
        });
    }

    Ok(MthroughputDirection {
        saturation_reached,
        connections: conns.len() as u32,
        intervals,
        ramp_duration_ms: ramp_duration.as_secs_f64() * 1000.0,
        measure_duration_ms: window_secs * 1000.0,
        load_duration_ms: load_duration.as_secs_f64() * 1000.0,
        bytes_transferred,
        capacity_mbps,
        per_conn_min_mbps: per_conn_min,
        per_conn_max_mbps: per_conn_max,
        per_conn_mean_mbps: per_conn_mean,
        fair_share_spread_pct: spread_pct,
        rwnd_limited_conns,
        sndbuf_limited_conns,
        path_limited_conns,
        unobserved_conns,
        per_conn,
    })
}

/// Open one H2 connection (dup'ing its fd for post-transfer stats) and spawn
/// its load loop, counting into the connection's OWN byte counter.
async fn spawn_load_connection(
    cfg: &MthroughputConfig,
    direction: LoadDirection,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<Conn, String> {
    let conn = connect_h2(&cfg.h2_target()).await?;
    let sender = conn.sender;
    let probe = conn.socket_probe;
    let bytes = Arc::new(AtomicU64::new(0));

    let target = cfg.h2_target();
    let load_request_bytes = cfg.load_request_bytes;
    let task_bytes = bytes.clone();
    let mut stop = stop;
    let task = tokio::spawn(async move {
        loop {
            if *stop.borrow() {
                return;
            }
            let xfer = async {
                match direction {
                    LoadDirection::Download => {
                        load_download_once(&target, sender.clone(), &task_bytes, load_request_bytes)
                            .await
                    }
                    LoadDirection::Upload => {
                        load_upload_once(&target, sender.clone(), &task_bytes, load_request_bytes)
                            .await
                    }
                }
            };
            tokio::select! {
                r = xfer => {
                    if let Err(e) = r {
                        debug!("mthroughput load transfer error: {e}");
                        // Brief backoff so a hard-down route cannot spin.
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
                _ = stop.changed() => return,
            }
        }
    });

    Ok(Conn { bytes, probe, task })
}

async fn stop_and_reap(stop_tx: &tokio::sync::watch::Sender<bool>, conns: &mut [Conn]) {
    let _ = stop_tx.send(true);
    for c in conns.iter_mut() {
        c.task.abort();
        let _ = (&mut c.task).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-connection attribution (pure, unit-tested)
// ─────────────────────────────────────────────────────────────────────────────

/// A connection's throughput-attribution verdict from the Wave-S
/// busy/rwnd/sndbuf chronograph triad. Send-side semantics — see the
/// [`MthroughputConn::verdict`] docs for the download-direction caveat.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Verdict {
    /// ≥ 5% of busy time limited by the peer's receive window.
    RwndLimited(u8),
    /// ≥ 5% of busy time limited by the local send buffer.
    SndbufLimited(u8),
    /// Triad present, neither share noteworthy — the path (congestion
    /// control vs available bandwidth) was the constraint.
    PathLimited,
    /// No triad data: non-Linux platform or pre-4.10 kernel. Honest absence,
    /// never folded into "path-limited".
    Unobserved,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::RwndLimited(pct) => write!(f, "rwnd-limited {pct}%"),
            Verdict::SndbufLimited(pct) => write!(f, "sndbuf-limited {pct}%"),
            Verdict::PathLimited => write!(f, "path-limited"),
            Verdict::Unobserved => write!(f, "unobserved"),
        }
    }
}

/// Same noteworthiness threshold as the run-level attribution note in
/// `summary.rs` (5% of busy time).
const NOTEWORTHY_PCT: f64 = 5.0;

/// Fold a connection's post-transfer socket stats into a verdict. The larger
/// of the two limited shares wins when both clear the threshold.
fn limited_verdict(stats: Option<&SocketStats>) -> Verdict {
    let Some(s) = stats else {
        return Verdict::Unobserved;
    };
    let Some(busy) = s.busy_time_us.filter(|b| *b > 0) else {
        return Verdict::Unobserved;
    };
    let rwnd_pct = s.rwnd_limited_us.unwrap_or(0) as f64 / busy as f64 * 100.0;
    let sndbuf_pct = s.sndbuf_limited_us.unwrap_or(0) as f64 / busy as f64 * 100.0;
    if rwnd_pct >= NOTEWORTHY_PCT && rwnd_pct >= sndbuf_pct {
        Verdict::RwndLimited(rwnd_pct.round().min(100.0) as u8)
    } else if sndbuf_pct >= NOTEWORTHY_PCT {
        Verdict::SndbufLimited(sndbuf_pct.round().min(100.0) as u8)
    } else {
        Verdict::PathLimited
    }
}

fn mthroughput_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Mthroughput,
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
            category: ErrorCategory::Http,
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
        rpm: None,
        ping: None,
        path: None,
        dualstack: None,
        websocket: None,
        pmtud: None,
        responsiveness: None,
        stamp: None,
        mthroughput: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(busy: Option<u64>, rwnd: Option<u64>, sndbuf: Option<u64>) -> SocketStats {
        SocketStats {
            busy_time_us: busy,
            rwnd_limited_us: rwnd,
            sndbuf_limited_us: sndbuf,
            ..Default::default()
        }
    }

    #[test]
    fn verdict_unobserved_without_stats_or_triad() {
        assert_eq!(limited_verdict(None), Verdict::Unobserved);
        // Stats present but no busy-time chronograph (macOS/Windows/old
        // kernel) — must be honest absence, not "path-limited".
        let s = stats(None, Some(1), Some(1));
        assert_eq!(limited_verdict(Some(&s)), Verdict::Unobserved);
        let s = stats(Some(0), None, None);
        assert_eq!(limited_verdict(Some(&s)), Verdict::Unobserved);
    }

    #[test]
    fn verdict_rwnd_limited_when_share_noteworthy() {
        let s = stats(Some(1_000_000), Some(840_000), Some(10_000));
        assert_eq!(limited_verdict(Some(&s)), Verdict::RwndLimited(84));
        assert_eq!(limited_verdict(Some(&s)).to_string(), "rwnd-limited 84%");
    }

    #[test]
    fn verdict_sndbuf_limited_when_share_noteworthy() {
        let s = stats(Some(1_000_000), Some(10_000), Some(250_000));
        assert_eq!(limited_verdict(Some(&s)), Verdict::SndbufLimited(25));
        assert_eq!(limited_verdict(Some(&s)).to_string(), "sndbuf-limited 25%");
    }

    #[test]
    fn verdict_larger_limited_share_wins_when_both_noteworthy() {
        let s = stats(Some(1_000_000), Some(300_000), Some(200_000));
        assert_eq!(limited_verdict(Some(&s)), Verdict::RwndLimited(30));
        let s = stats(Some(1_000_000), Some(200_000), Some(300_000));
        assert_eq!(limited_verdict(Some(&s)), Verdict::SndbufLimited(30));
    }

    #[test]
    fn verdict_path_limited_when_neither_share_noteworthy() {
        // 2% rwnd, 1% sndbuf — the normal path/CPU-limited transfer.
        let s = stats(Some(1_000_000), Some(20_000), Some(10_000));
        assert_eq!(limited_verdict(Some(&s)), Verdict::PathLimited);
        assert_eq!(limited_verdict(Some(&s)).to_string(), "path-limited");
    }

    #[test]
    fn config_defaults_are_time_boxed_not_payload_sized() {
        let t = ThroughputConfig {
            run_cfg: crate::runner::http::RunConfig::default(),
            base_url: url::Url::parse("http://127.0.0.1:8080/").unwrap(),
        };
        let cfg = MthroughputConfig::from_parts(&t);
        assert_eq!(cfg.interval_ms, 1_000);
        assert_eq!(cfg.initial_connections, 1);
        assert_eq!(cfg.add_per_interval, 1);
        assert_eq!(cfg.max_connections, 8);
        assert_eq!(cfg.mad, 4);
        assert!((cfg.sdt - 0.05).abs() < 1e-12);
        assert_eq!(cfg.max_duration_ms, 15_000);
        // Worst case ramp (8 conns @ 1/s) + measure window (4 s) fits the cap.
        assert!(
            (cfg.max_connections as u64 * cfg.interval_ms) + (cfg.mad as u64 * cfg.interval_ms)
                <= cfg.max_duration_ms
        );
    }
}
