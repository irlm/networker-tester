//! Working-conditions responsiveness probe (`responsiveness` mode) —
//! conformant with draft-ietf-ippm-responsiveness (verified against
//! draft-08).
//!
//! # Methodology (draft-08 §4, parameters verbatim)
//!
//! Per direction (download first, then upload — both stages measured):
//!
//! 1. **Load ramp to working conditions.** Start `INP = 1` HTTP/2
//!    load-generating connection against the endpoint's `/download`
//!    (`/upload`) route; every interval `ID = 1 s`, add `INC = 1` connection
//!    up to `MNP = 16` until goodput saturates. Saturation is declared when
//!    the standard deviation of the last `MAD = 4` moving-average goodput
//!    values is less than `SDT = 5 %` of the current moving average. After
//!    goodput saturation the same stability check is applied to per-interval
//!    responsiveness values; the direction ends when both are stable or the
//!    per-direction time cap is reached.
//! 2. **Probes DURING load** (up to `MPS = 100`/s allowed; we send 10/s,
//!    alternating kinds every 100 ms — same amount per kind, as the draft
//!    requires):
//!    - **foreign** probes on NEW connections — measure `tcp_f` (TCP
//!      handshake), `tls_f` (TLS handshake, HTTPS targets only) and `http_f`
//!      (GET of a 1-byte object) on the loaded link;
//!    - **self** probes ON a load-generating connection — a 1-byte GET
//!      multiplexed onto the loaded H2 flow (`http_l`). Because the probe
//!      shares the loaded flow's 5-tuple, flow-isolating AQMs (fq_codel,
//!      CAKE) cannot queue-jump it — this is the wrong-answer fix the m4
//!      audit demanded (§1.4: the sparse-flow blind spot).
//! 3. **Aggregation** (draft-08 "Final Calculations", quoted):
//!    `Foreign_Responsiveness = 60000 / ((TM(tcp_f)+TM(tls_f)+TM(http_f))/3)`
//!    (TCP-only variant `60000 / ((TM(tcp_f)+TM(http_f))/2)` for cleartext),
//!    `Loaded_Responsiveness = 60000 / TM(http_l)`, and
//!    `Responsiveness = (Foreign_Responsiveness + Loaded_Responsiveness) / 2`,
//!    where TM is the single-sided trimmed mean at `TMP = 95 %`, computed
//!    over the final `MAD` intervals of measurement data.
//!
//! # Deliberate deviations (documented, not hidden)
//!
//! - **Probe rate**: fixed 10/s (5/s per kind) instead of scaling toward
//!   `MPS = 100` with capacity — well inside the draft's ceiling and enough
//!   samples for the trimmed means on every link class.
//! - **Per-direction time cap**: 20 s (the draft does not fix a cap;
//!   `networkQuality` uses ~20 s per direction). If the cap hits before
//!   stability, `saturation_reached`/`responsiveness_stable` report `false`
//!   and the RPM is still computed from the final `MAD` intervals — flagged,
//!   never fabricated.
//! - **`http_f` includes the H2 preface/SETTINGS exchange** of the fresh
//!   connection (timed from H2 client handshake start to full 1-byte
//!   response) — it is connection-setup cost a real client pays.
//! - **Cleartext targets** use HTTP/2 with prior knowledge (h2c) for load and
//!   probes; `tls_f` is then absent and the TCP-only formula applies.
//!
//! Honesty guard: like `rpm`'s `load_ok`, if the load generator never moved a
//! byte the attempt FAILS instead of reporting an idle link's latency as
//! "under load".

use crate::metrics::{
    ErrorCategory, ErrorRecord, Protocol, RequestAttempt, ResponsivenessDirection,
    ResponsivenessResult,
};
use crate::runner::load_gen::{
    connect_h2, empty_body, host_header, load_download_once, load_upload_once, mean, stddev,
    H2Sender, H2Target, LoadDirection,
};
use crate::runner::throughput::ThroughputConfig;
use chrono::Utc;
use http_body_util::BodyExt;
use hyper::Request;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration (draft-08 parameter defaults)
// ─────────────────────────────────────────────────────────────────────────────

/// Interval duration ID (draft-08 default: 1 second).
pub const DEFAULT_INTERVAL_MS: u64 = 1_000;
/// Initial number of load-generating connections INP (draft-08 default: 1).
pub const DEFAULT_INITIAL_CONNECTIONS: u32 = 1;
/// Connections added per interval INC (draft-08 default: 1).
pub const DEFAULT_ADD_PER_INTERVAL: u32 = 1;
/// Maximum parallel load-generating connections MNP (draft-08 default: 16).
pub const DEFAULT_MAX_CONNECTIONS: u32 = 16;
/// Moving-average distance MAD (draft-08 default: 4 intervals).
pub const DEFAULT_MAD: usize = 4;
/// Standard-deviation tolerance SDT (draft-08 default: 5 %).
pub const DEFAULT_SDT: f64 = 0.05;
/// Trimmed-mean percentage TMP (draft-08 default: 95 %).
pub const DEFAULT_TMP: f64 = 0.95;
/// Per-direction wall-clock cap (deviation: draft fixes no cap; ~20 s is
/// `networkQuality` practice).
pub const DEFAULT_MAX_DURATION_MS: u64 = 20_000;
/// Probe cadence: one probe every 100 ms, alternating foreign/self →
/// 5/s per kind, 10/s total (≤ the draft's MPS = 100 ceiling).
pub const DEFAULT_PROBE_INTERVAL_MS: u64 = 100;
/// Bytes per load request (repeated back-to-back per connection).
pub const DEFAULT_LOAD_REQUEST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ResponsivenessConfig {
    /// Endpoint base URL (scheme selects h2-over-TLS vs h2c).
    pub base_url: url::Url,
    pub insecure: bool,
    pub ca_bundle: Option<String>,
    /// Per-connect / per-probe timeout (ms).
    pub timeout_ms: u64,
    /// Interval duration ID (ms).
    pub interval_ms: u64,
    pub initial_connections: u32,
    pub add_per_interval: u32,
    pub max_connections: u32,
    /// Moving-average distance MAD (intervals).
    pub mad: usize,
    /// Standard-deviation tolerance SDT (fraction, 0.05 = 5 %).
    pub sdt: f64,
    /// Trimmed-mean percentage TMP (fraction, 0.95 = 95 %).
    pub tmp: f64,
    /// Per-direction wall-clock cap (ms).
    pub max_duration_ms: u64,
    /// Probe cadence (ms per probe, kinds alternating).
    pub probe_interval_ms: u64,
    /// Bytes per load request.
    pub load_request_bytes: usize,
}

impl ResponsivenessConfig {
    /// Build from the resolved throughput config (same base URL + TLS trust
    /// settings every other endpoint mode uses), with draft-08 defaults.
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
            tmp: DEFAULT_TMP,
            max_duration_ms: DEFAULT_MAX_DURATION_MS,
            probe_interval_ms: DEFAULT_PROBE_INTERVAL_MS,
            load_request_bytes: DEFAULT_LOAD_REQUEST_BYTES,
        }
    }

    /// Connection parameters for the shared H2 load-generation machinery.
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

pub async fn run_responsiveness_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &ResponsivenessConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    // ── Download direction (the draft measures download first) ───────────────
    let download = run_direction(cfg, LoadDirection::Download).await;
    let download = match download {
        Ok(d) => d,
        Err(msg) => {
            return responsiveness_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                format!("Download stage: {msg}"),
            )
        }
    };

    // ── Upload direction — sequential, never fabricated on failure ───────────
    let (upload, upload_error) = match run_direction(cfg, LoadDirection::Upload).await {
        Ok(u) => (Some(u), None),
        Err(msg) => (None, Some(format!("Upload stage: {msg}"))),
    };

    let result = ResponsivenessResult {
        remote_addr: cfg.base_url.to_string(),
        rpm_download: download.rpm,
        rpm_upload: upload.as_ref().and_then(|u| u.rpm),
        capacity_down_mbps: download.capacity_mbps,
        capacity_up_mbps: upload.as_ref().and_then(|u| u.capacity_mbps),
        download,
        upload,
        upload_error,
        started_at,
    };

    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Responsiveness,
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
        responsiveness: Some(result),
        stamp: None,
        mthroughput: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Direction engine
// ─────────────────────────────────────────────────────────────────────────────

/// One probe latency sample.
struct ProbeSample {
    sent_at: Instant,
    foreign: bool,
    /// TCP connect time (foreign only).
    tcp_ms: Option<f64>,
    /// TLS handshake time (foreign + HTTPS only).
    tls_ms: Option<f64>,
    /// GET issue → full 1-byte response (both kinds; includes the fresh
    /// connection's H2 preface/SETTINGS for foreign probes — see module docs).
    http_ms: Option<f64>,
    ok: bool,
}

struct SharedState {
    /// Bytes moved by all load connections (download: body bytes received;
    /// upload: body bytes handed to the H2 layer under its flow control).
    /// `Arc` so upload bodies can hold the counter directly.
    bytes: Arc<AtomicU64>,
    /// Open load-connection senders — self probes multiplex onto these.
    senders: Mutex<Vec<H2Sender>>,
    probes: Mutex<Vec<ProbeSample>>,
}

async fn run_direction(
    cfg: &ResponsivenessConfig,
    direction: LoadDirection,
) -> Result<ResponsivenessDirection, String> {
    let state = Arc::new(SharedState {
        bytes: Arc::new(AtomicU64::new(0)),
        senders: Mutex::new(Vec::new()),
        probes: Mutex::new(Vec::new()),
    });
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut conn_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // ── Initial INP load connections ─────────────────────────────────────────
    let mut conn_count = 0u32;
    for _ in 0..cfg.initial_connections.max(1) {
        match spawn_load_connection(cfg, direction, &state, stop_rx.clone()).await {
            Ok(task) => {
                conn_tasks.push(task);
                conn_count += 1;
            }
            Err(e) => {
                stop_and_reap(&stop_tx, &mut conn_tasks).await;
                return Err(format!("initial load connection failed: {e}"));
            }
        }
    }

    // ── Probe scheduler (foreign/self alternating) ───────────────────────────
    let probe_task = {
        let cfg = cfg.clone();
        let state = state.clone();
        let mut stop = stop_rx.clone();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(Duration::from_millis(cfg.probe_interval_ms.max(10)));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Cap in-flight probes so a stalled link cannot pile up tasks.
            let gate = Arc::new(tokio::sync::Semaphore::new(8));
            let mut foreign = true;
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = stop.changed() => break,
                }
                if *stop.borrow() {
                    break;
                }
                let Ok(permit) = gate.clone().try_acquire_owned() else {
                    continue; // all slots busy — skip, never queue up
                };
                let cfg = cfg.clone();
                let state = state.clone();
                let is_foreign = foreign;
                foreign = !foreign;
                tokio::spawn(async move {
                    let _permit = permit;
                    let sample = if is_foreign {
                        foreign_probe(&cfg).await
                    } else {
                        self_probe(&cfg, &state).await
                    };
                    if let Some(sample) = sample {
                        state.probes.lock().await.push(sample);
                    }
                });
            }
        })
    };

    // ── Interval controller ──────────────────────────────────────────────────
    let started = Instant::now();
    let interval = Duration::from_millis(cfg.interval_ms.max(50));
    let interval_s = interval.as_secs_f64();
    let cap = Duration::from_millis(cfg.max_duration_ms.max(cfg.interval_ms));
    let mad = cfg.mad.max(2);

    let mut goodputs: Vec<f64> = Vec::new(); // bytes/s per interval
    let mut moving_avgs: Vec<f64> = Vec::new();
    let mut resp_values: Vec<f64> = Vec::new(); // per-interval responsiveness
    let mut last_total = 0u64;
    let mut saturation_reached = false;
    let mut responsiveness_stable = false;
    let mut saturated_connections = conn_count;
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // first tick fires immediately — consume it

    loop {
        tick.tick().await;

        let total = state.bytes.load(Ordering::Relaxed);
        let delta = total.saturating_sub(last_total);
        last_total = total;
        goodputs.push(delta as f64 / interval_s);
        let ma_window = &goodputs[goodputs.len().saturating_sub(mad)..];
        let current_ma = mean(ma_window);
        moving_avgs.push(current_ma);

        if !saturation_reached {
            // Draft: "If the standard deviation of the past MAD average
            // goodput values is less than SDT of the current_average,
            // declare goodput saturation."
            if moving_avgs.len() >= mad
                && current_ma > 0.0
                && stddev(&moving_avgs[moving_avgs.len() - mad..]) < cfg.sdt * current_ma
            {
                saturation_reached = true;
                saturated_connections = conn_count;
            } else if conn_count < cfg.max_connections {
                // Draft: add INC connections per interval until saturation.
                for _ in 0..cfg.add_per_interval {
                    if conn_count >= cfg.max_connections {
                        break;
                    }
                    match spawn_load_connection(cfg, direction, &state, stop_rx.clone()).await {
                        Ok(task) => {
                            conn_tasks.push(task);
                            conn_count += 1;
                        }
                        Err(e) => {
                            debug!("responsiveness: ramp connection failed: {e}");
                            break;
                        }
                    }
                }
                saturated_connections = conn_count;
            }
        } else {
            // Draft: after goodput saturation, apply the same stability check
            // to per-interval responsiveness values.
            let window_start = started
                .elapsed()
                .saturating_sub(interval * (mad as u32))
                .max(Duration::ZERO);
            let samples = state.probes.lock().await;
            if let Some(r) = responsiveness_from_samples(&samples, cfg.tmp, |s| {
                s.sent_at.duration_since(started) >= window_start
            }) {
                resp_values.push(r);
            }
            drop(samples);
            if resp_values.len() >= mad {
                let cur = resp_values[resp_values.len() - 1];
                if cur > 0.0 && stddev(&resp_values[resp_values.len() - mad..]) < cfg.sdt * cur {
                    responsiveness_stable = true;
                    break;
                }
            }
        }

        if started.elapsed() >= cap {
            break;
        }
    }

    let load_duration = started.elapsed();
    let intervals = goodputs.len() as u32;

    // ── Stop everything ──────────────────────────────────────────────────────
    stop_and_reap(&stop_tx, &mut conn_tasks).await;
    probe_task.abort();
    let _ = probe_task.await;

    // ── Aggregate (final MAD intervals of measurement data, per the draft) ───
    let bytes_transferred = state.bytes.load(Ordering::Relaxed);
    if bytes_transferred == 0 {
        // Same honesty class as rpm's load_ok guard: latency on an idle link
        // must not be published as "under load".
        return Err(format!(
            "load generator moved 0 bytes over {} connection(s) — refusing to report \
             responsiveness for an unloaded link",
            conn_count
        ));
    }
    let final_window_start = load_duration.saturating_sub(interval * (mad as u32));
    let samples = state.probes.lock().await;
    let in_final = |s: &ProbeSample| s.sent_at.duration_since(started) >= final_window_start;

    let tm_of = |f: &dyn Fn(&ProbeSample) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = samples
            .iter()
            .filter(|s| s.ok && in_final(s))
            .filter_map(f)
            .collect();
        trimmed_mean(&vals, cfg.tmp)
    };
    let foreign_tcp_tm_ms = tm_of(&|s| if s.foreign { s.tcp_ms } else { None });
    let foreign_tls_tm_ms = tm_of(&|s| if s.foreign { s.tls_ms } else { None });
    let foreign_http_tm_ms = tm_of(&|s| if s.foreign { s.http_ms } else { None });
    let self_http_tm_ms = tm_of(&|s| if !s.foreign { s.http_ms } else { None });

    let foreign_rpm =
        foreign_responsiveness(foreign_tcp_tm_ms, foreign_tls_tm_ms, foreign_http_tm_ms);
    let self_rpm = self_http_tm_ms.filter(|v| *v > 0.0).map(|v| 60_000.0 / v);
    // Draft: Responsiveness = (Foreign + Loaded) / 2 — both terms required.
    let rpm = match (foreign_rpm, self_rpm) {
        (Some(f), Some(s)) => Some((f + s) / 2.0),
        _ => None,
    };

    let foreign_probes_sent = samples.iter().filter(|s| s.foreign).count() as u32;
    let foreign_probes_ok = samples.iter().filter(|s| s.foreign && s.ok).count() as u32;
    let self_probes_sent = samples.iter().filter(|s| !s.foreign).count() as u32;
    let self_probes_ok = samples.iter().filter(|s| !s.foreign && s.ok).count() as u32;
    drop(samples);

    let capacity_mbps = moving_avgs
        .last()
        .copied()
        .filter(|v| *v > 0.0)
        .map(|bytes_per_s| bytes_per_s / 1e6);

    Ok(ResponsivenessDirection {
        saturation_reached,
        responsiveness_stable,
        saturated_connections,
        intervals,
        load_duration_ms: load_duration.as_secs_f64() * 1000.0,
        bytes_transferred,
        capacity_mbps,
        rpm,
        foreign_rpm,
        self_rpm,
        foreign_tcp_tm_ms,
        foreign_tls_tm_ms,
        foreign_http_tm_ms,
        self_http_tm_ms,
        foreign_probes_sent,
        foreign_probes_ok,
        self_probes_sent,
        self_probes_ok,
    })
}

async fn stop_and_reap(
    stop_tx: &tokio::sync::watch::Sender<bool>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let _ = stop_tx.send(true);
    for t in tasks.drain(..) {
        t.abort();
        let _ = t.await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Load connections
// ─────────────────────────────────────────────────────────────────────────────

/// Open one H2 connection via the shared load-gen machinery, register its
/// sender for self probes, and spawn its load loop.
async fn spawn_load_connection(
    cfg: &ResponsivenessConfig,
    direction: LoadDirection,
    state: &Arc<SharedState>,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let conn = connect_h2(&cfg.h2_target()).await?;
    let sender = conn.sender;
    state.senders.lock().await.push(sender.clone());

    let target = cfg.h2_target();
    let load_request_bytes = cfg.load_request_bytes;
    let state = state.clone();
    let mut stop = stop;
    Ok(tokio::spawn(async move {
        loop {
            if *stop.borrow() {
                return;
            }
            let xfer = async {
                match direction {
                    LoadDirection::Download => {
                        load_download_once(
                            &target,
                            sender.clone(),
                            &state.bytes,
                            load_request_bytes,
                        )
                        .await
                    }
                    LoadDirection::Upload => {
                        load_upload_once(&target, sender.clone(), &state.bytes, load_request_bytes)
                            .await
                    }
                }
            };
            tokio::select! {
                r = xfer => {
                    if let Err(e) = r {
                        debug!("responsiveness load transfer error: {e}");
                        // Brief backoff so a hard-down route cannot spin.
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
                _ = stop.changed() => return,
            }
        }
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Probes
// ─────────────────────────────────────────────────────────────────────────────

/// Foreign probe: fresh connection — `tcp_f` + `tls_f` (HTTPS) + `http_f`
/// (1-byte GET; includes the fresh connection's H2 settings exchange).
async fn foreign_probe(cfg: &ResponsivenessConfig) -> Option<ProbeSample> {
    let sent_at = Instant::now();
    let timeout = Duration::from_millis(cfg.timeout_ms.max(1));

    let connected = connect_h2(&cfg.h2_target()).await;
    match connected {
        Ok(conn) => {
            let t_http = Instant::now();
            let ok = tokio::time::timeout(timeout, one_byte_get(cfg, conn.sender))
                .await
                .ok()
                .and_then(|r| r.ok())
                .is_some();
            let http_ms = ok.then(|| t_http.elapsed().as_secs_f64() * 1000.0);
            Some(ProbeSample {
                sent_at,
                foreign: true,
                tcp_ms: Some(conn.tcp_ms),
                tls_ms: conn.tls_ms,
                http_ms,
                ok,
            })
        }
        Err(e) => {
            debug!("responsiveness foreign probe failed: {e}");
            Some(ProbeSample {
                sent_at,
                foreign: true,
                tcp_ms: None,
                tls_ms: None,
                http_ms: None,
                ok: false,
            })
        }
    }
}

/// Self probe: 1-byte GET multiplexed on a load-generating connection.
async fn self_probe(cfg: &ResponsivenessConfig, state: &SharedState) -> Option<ProbeSample> {
    let sender = {
        let senders = state.senders.lock().await;
        if senders.is_empty() {
            return None; // no load connection yet — nothing to probe ON
        }
        // Rotate across connections so all loaded flows get sampled.
        let idx = state.bytes.load(Ordering::Relaxed) as usize % senders.len();
        senders[idx].clone()
    };
    let sent_at = Instant::now();
    let timeout = Duration::from_millis(cfg.timeout_ms.max(1));
    let t_http = Instant::now();
    let ok = tokio::time::timeout(timeout, one_byte_get(cfg, sender))
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_some();
    let http_ms = ok.then(|| t_http.elapsed().as_secs_f64() * 1000.0);
    Some(ProbeSample {
        sent_at,
        foreign: false,
        tcp_ms: None,
        tls_ms: None,
        http_ms,
        ok,
    })
}

/// GET the draft's 1-byte object (`/download?bytes=1`) and drain the response.
async fn one_byte_get(cfg: &ResponsivenessConfig, mut sender: H2Sender) -> Result<(), String> {
    let host = host_header(&cfg.base_url);
    let req = Request::builder()
        .method("GET")
        .uri("/download?bytes=1")
        .header("host", &host)
        .header("user-agent", "networker-tester/responsiveness")
        .body(empty_body())
        .map_err(|e| e.to_string())?;
    let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("probe GET returned {}", resp.status()));
    }
    let mut body = resp.into_body();
    while let Some(frame) = body.frame().await {
        frame.map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Math helpers (draft-08 formulas; mean/stddev shared via load_gen)
// ─────────────────────────────────────────────────────────────────────────────

/// Single-sided trimmed mean at `tmp` (draft-08 TM): discard the samples
/// above the `tmp` percentile (the top 5 % for TMP = 95 %), mean the rest.
/// `None` when there are no samples — never a fabricated 0.
fn trimmed_mean(samples: &[f64], tmp: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let keep = ((sorted.len() as f64) * tmp.clamp(0.0, 1.0)).floor() as usize;
    let keep = keep.clamp(1, sorted.len());
    Some(mean(&sorted[..keep]))
}

/// Draft-08 foreign term: `60000 / ((TM(tcp_f)+TM(tls_f)+TM(http_f))/3)`,
/// or the TCP-only variant `60000 / ((TM(tcp_f)+TM(http_f))/2)` when there
/// is no TLS (cleartext target).
fn foreign_responsiveness(
    tcp_tm: Option<f64>,
    tls_tm: Option<f64>,
    http_tm: Option<f64>,
) -> Option<f64> {
    let (tcp, http) = (tcp_tm?, http_tm?);
    let denom = match tls_tm {
        Some(tls) => (tcp + tls + http) / 3.0,
        None => (tcp + http) / 2.0,
    };
    (denom > 0.0).then(|| 60_000.0 / denom)
}

/// Per-interval responsiveness over the samples selected by `pred` — used
/// for the draft's second (responsiveness) stability criterion.
fn responsiveness_from_samples(
    samples: &[ProbeSample],
    tmp: f64,
    pred: impl Fn(&ProbeSample) -> bool,
) -> Option<f64> {
    let sel: Vec<&ProbeSample> = samples.iter().filter(|s| s.ok && pred(s)).collect();
    let tm = |f: &dyn Fn(&ProbeSample) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = sel.iter().filter_map(|s| f(s)).collect();
        trimmed_mean(&vals, tmp)
    };
    let foreign = foreign_responsiveness(
        tm(&|s| if s.foreign { s.tcp_ms } else { None }),
        tm(&|s| if s.foreign { s.tls_ms } else { None }),
        tm(&|s| if s.foreign { s.http_ms } else { None }),
    )?;
    let self_tm = tm(&|s| if !s.foreign { s.http_ms } else { None })?;
    if self_tm <= 0.0 {
        return None;
    }
    Some((foreign + 60_000.0 / self_tm) / 2.0)
}

fn responsiveness_failed(
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
        protocol: Protocol::Responsiveness,
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

    #[test]
    fn trimmed_mean_discards_top_tail_only() {
        // 20 samples: 19×10ms + 1×1000ms outlier. TMP=95% keeps the lowest
        // 19 → the outlier is discarded, mean = 10.
        let mut v = vec![10.0; 19];
        v.push(1000.0);
        let tm = trimmed_mean(&v, 0.95).unwrap();
        assert!((tm - 10.0).abs() < 1e-9, "got {tm}");
    }

    #[test]
    fn trimmed_mean_small_n_keeps_at_least_one() {
        assert_eq!(trimmed_mean(&[42.0], 0.95), Some(42.0));
        assert_eq!(trimmed_mean(&[], 0.95), None);
    }

    #[test]
    fn foreign_responsiveness_tls_uses_three_way_mean() {
        // TM(tcp)=10, TM(tls)=20, TM(http)=30 → denom 20 → 3000 RPM.
        let r = foreign_responsiveness(Some(10.0), Some(20.0), Some(30.0)).unwrap();
        assert!((r - 3000.0).abs() < 1e-9, "got {r}");
    }

    #[test]
    fn foreign_responsiveness_cleartext_uses_two_way_mean() {
        // No TLS: denom = (10+30)/2 = 20 → 3000 RPM (draft's TCP-only case).
        let r = foreign_responsiveness(Some(10.0), None, Some(30.0)).unwrap();
        assert!((r - 3000.0).abs() < 1e-9, "got {r}");
    }

    #[test]
    fn foreign_responsiveness_requires_tcp_and_http() {
        assert!(foreign_responsiveness(None, None, Some(30.0)).is_none());
        assert!(foreign_responsiveness(Some(10.0), None, None).is_none());
    }

    #[test]
    fn config_defaults_match_draft08_parameters() {
        let t = ThroughputConfig {
            run_cfg: crate::runner::http::RunConfig::default(),
            base_url: url::Url::parse("http://127.0.0.1:8080/").unwrap(),
        };
        let cfg = ResponsivenessConfig::from_parts(&t);
        assert_eq!(cfg.interval_ms, 1_000); // ID
        assert_eq!(cfg.initial_connections, 1); // INP
        assert_eq!(cfg.add_per_interval, 1); // INC
        assert_eq!(cfg.max_connections, 16); // MNP
        assert_eq!(cfg.mad, 4); // MAD
        assert!((cfg.sdt - 0.05).abs() < 1e-12); // SDT
        assert!((cfg.tmp - 0.95).abs() < 1e-12); // TMP
    }
}
