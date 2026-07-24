//! Latency-under-load / bufferbloat probe (`rpm` mode, Apple-RPM-style).
//!
//! Phase 1 samples UDP echo RTT (endpoint port 9999) on an idle link — the
//! unloaded baseline. Phase 2 saturates the link with back-to-back HTTP
//! `/download` transfers from the networker-endpoint and, DURING the load,
//! fires UDP echo probes at a steady cadence (default every 100 ms).
//!
//! Reported metrics:
//! - unloaded RTT min/avg/p95 + jitter + loss
//! - loaded RTT min/avg/p95 + jitter + loss
//! - `rpm` = 60000 / loaded avg RTT ms (round-trips per minute, higher better)
//! - `bufferbloat_factor` = loaded avg / unloaded avg (1.0 ≈ no bufferbloat)
//!
//! The same wire format as the plain `udp` probe is used (big-endian
//! `[seq u32][timestamp_us i64]` + padding), and echoes are matched by their
//! embedded sequence id so late/reordered/duplicate echoes are credited to the
//! probe that sent them (trust audit V12 semantics).

use crate::metrics::{
    aggregate_udp_rtts, ErrorCategory, ErrorRecord, Protocol, RequestAttempt, RpmResult,
};
use crate::runner::throughput::{run_download_probe, ThroughputConfig};
use crate::runner::udp::UdpProbeConfig;
use chrono::Utc;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::debug;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RpmProbeConfig {
    /// UDP echo settings (host, port 9999, per-probe timeout, payload size).
    /// `probe_count` is the unloaded-phase probe count.
    pub udp: UdpProbeConfig,
    /// Download settings for the load generator (base URL, timeouts, TLS).
    pub throughput: ThroughputConfig,
    /// Bytes per load-generator download request. Downloads repeat
    /// back-to-back until the load window closes, so the link stays saturated
    /// even when one transfer finishes early (e.g. loopback).
    pub download_bytes: usize,
    /// Length of the loaded phase (ms).
    pub load_duration_ms: u64,
    /// Cadence of loaded-phase UDP echo probes (ms).
    pub probe_interval_ms: u64,
}

/// Default load-generator request size: 32 MiB per download.
pub const DEFAULT_RPM_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;
/// Default loaded-phase window: 5 s.
pub const DEFAULT_RPM_LOAD_DURATION_MS: u64 = 5_000;
/// Default loaded-phase probe cadence: 100 ms → ~50 samples per window.
pub const DEFAULT_RPM_PROBE_INTERVAL_MS: u64 = 100;

impl RpmProbeConfig {
    /// Build the rpm config from the already-resolved udp + throughput configs
    /// (the same objects every other probe mode receives from the CLI layer).
    pub fn from_parts(udp: UdpProbeConfig, throughput: ThroughputConfig) -> Self {
        Self {
            udp,
            throughput,
            download_bytes: DEFAULT_RPM_DOWNLOAD_BYTES,
            load_duration_ms: DEFAULT_RPM_LOAD_DURATION_MS,
            probe_interval_ms: DEFAULT_RPM_PROBE_INTERVAL_MS,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_rpm_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &RpmProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    // ── Resolve the UDP echo target ──────────────────────────────────────────
    let target = format!("{}:{}", cfg.udp.target_host, cfg.udp.target_port);
    let target_addr: SocketAddr = match resolve(&target).await {
        Ok(a) => a,
        Err(msg) => return rpm_failed(run_id, attempt_id, sequence_num, started_at, msg),
    };

    // ── Phase 1: unloaded UDP echo RTTs ──────────────────────────────────────
    let unloaded = match echo_rtts(
        target_addr,
        cfg.udp.probe_count,
        cfg.udp.payload_size,
        Pacing::BackToBack {
            timeout_ms: cfg.udp.timeout_ms,
        },
    )
    .await
    {
        Ok(rtts) => rtts,
        Err(msg) => return rpm_failed(run_id, attempt_id, sequence_num, started_at, msg),
    };
    let unloaded_stats = aggregate_udp_rtts(&unloaded);
    let unloaded_success = unloaded.iter().filter(|r| r.is_some()).count() as u32;
    if unloaded_success == 0 {
        return rpm_failed(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            format!("Unloaded phase: all {} UDP echo probes lost (is the endpoint's UDP echo server on {target} reachable?)", cfg.udp.probe_count),
        );
    }

    // ── Phase 2: sustained download + paced UDP echo probes ──────────────────
    let load = Arc::new(LoadStats::default());
    let load_handle = {
        let load = load.clone();
        let throughput = cfg.throughput.clone();
        let bytes = cfg.download_bytes;
        let window = Duration::from_millis(cfg.load_duration_ms);
        tokio::spawn(async move {
            let started = Instant::now();
            // Back-to-back downloads keep the link saturated for the whole
            // window even when a single transfer finishes early. The final
            // in-flight transfer is aborted when the handle is dropped.
            while started.elapsed() < window {
                load.downloads_started.fetch_add(1, Ordering::Relaxed);
                let attempt = run_download_probe(run_id, u32::MAX, bytes, &throughput).await;
                if attempt.success {
                    let delivered = attempt
                        .http
                        .as_ref()
                        .map(|h| h.body_size_bytes as u64)
                        .unwrap_or(0);
                    load.downloads_completed.fetch_add(1, Ordering::Relaxed);
                    load.bytes.fetch_add(delivered, Ordering::Relaxed);
                    if let Some(mbps) = attempt.http.as_ref().and_then(|h| h.throughput_mbps) {
                        // Store mean incrementally: sum in micro-MB/s units.
                        load.throughput_sum_micro
                            .fetch_add((mbps * 1e6) as u64, Ordering::Relaxed);
                        load.throughput_samples.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    load.downloads_failed.fetch_add(1, Ordering::Relaxed);
                    debug!(
                        "rpm load download failed: {}",
                        attempt
                            .error
                            .as_ref()
                            .map(|e| e.message.clone())
                            .unwrap_or_else(|| "unknown".into())
                    );
                    // Brief backoff so a hard-down /download route cannot spin.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        })
    };

    let load_started = Instant::now();
    let loaded_probe_count =
        (cfg.load_duration_ms / cfg.probe_interval_ms.max(1)).clamp(1, 10_000) as u32;
    let loaded = echo_rtts(
        target_addr,
        loaded_probe_count,
        cfg.udp.payload_size,
        Pacing::Paced {
            interval_ms: cfg.probe_interval_ms.max(1),
            grace_ms: cfg.udp.timeout_ms.min(1_000),
        },
    )
    .await;
    let load_duration_ms = load_started.elapsed().as_secs_f64() * 1000.0;

    // The load window and the paced probe window are the same length; abort
    // whatever transfer is still in flight so the probe returns promptly.
    load_handle.abort();
    let _ = load_handle.await;

    let loaded = match loaded {
        Ok(rtts) => rtts,
        Err(msg) => return rpm_failed(run_id, attempt_id, sequence_num, started_at, msg),
    };
    let loaded_stats = aggregate_udp_rtts(&loaded);
    let loaded_success = loaded.iter().filter(|r| r.is_some()).count() as u32;

    // The probe measures latency UNDER LOAD — if the load generator never got
    // a single byte moving, the "loaded" numbers would silently be a second
    // idle baseline. Fail loudly instead of reporting a fake factor of ~1.0.
    let downloads_started = load.downloads_started.load(Ordering::Relaxed);
    let downloads_completed = load.downloads_completed.load(Ordering::Relaxed);
    let downloads_failed = load.downloads_failed.load(Ordering::Relaxed);
    let load_ok = downloads_completed > 0 || downloads_started > downloads_failed;
    let throughput_samples = load.throughput_samples.load(Ordering::Relaxed);
    let load_throughput_mbps = if throughput_samples > 0 {
        Some(
            load.throughput_sum_micro.load(Ordering::Relaxed) as f64
                / 1e6
                / throughput_samples as f64,
        )
    } else {
        None
    };

    let rpm = (loaded_success > 0 && loaded_stats.avg > 0.0).then(|| 60_000.0 / loaded_stats.avg);
    let bufferbloat_factor =
        (loaded_success > 0 && unloaded_success > 0 && unloaded_stats.avg > 0.0)
            .then(|| loaded_stats.avg / unloaded_stats.avg);

    let result = RpmResult {
        remote_addr: target_addr.to_string(),
        unloaded_probe_count: cfg.udp.probe_count,
        unloaded_success_count: unloaded_success,
        unloaded_loss_percent: unloaded_stats.loss_percent,
        unloaded_rtt_min_ms: unloaded_stats.min,
        unloaded_rtt_avg_ms: unloaded_stats.avg,
        unloaded_rtt_p95_ms: unloaded_stats.p95,
        unloaded_jitter_ms: unloaded_stats.jitter,
        loaded_probe_count,
        loaded_success_count: loaded_success,
        loaded_loss_percent: loaded_stats.loss_percent,
        loaded_rtt_min_ms: loaded_stats.min,
        loaded_rtt_avg_ms: loaded_stats.avg,
        loaded_rtt_p95_ms: loaded_stats.p95,
        loaded_jitter_ms: loaded_stats.jitter,
        rpm,
        bufferbloat_factor,
        load_duration_ms,
        load_bytes_transferred: load.bytes.load(Ordering::Relaxed),
        load_downloads_completed: downloads_completed,
        load_throughput_mbps,
        started_at,
    };

    let error = if !load_ok {
        Some(ErrorRecord {
            category: ErrorCategory::Http,
            message: format!(
                "Load generator failed: 0/{downloads_started} downloads completed — \
                 loaded RTTs were measured on an idle link and are not reported as under-load"
            ),
            detail: None,
            occurred_at: Utc::now(),
        })
    } else if loaded_success == 0 {
        Some(ErrorRecord {
            category: ErrorCategory::Udp,
            message: format!("Loaded phase: all {loaded_probe_count} UDP echo probes lost"),
            detail: None,
            occurred_at: Utc::now(),
        })
    } else {
        None
    };
    let success = error.is_none();

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Rpm,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: Some(result),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct LoadStats {
    downloads_started: AtomicU32,
    downloads_completed: AtomicU32,
    downloads_failed: AtomicU32,
    bytes: AtomicU64,
    /// Sum of per-download throughput readings in micro-MB/s (integer so it
    /// fits an atomic; ÷1e6 on read).
    throughput_sum_micro: AtomicU64,
    throughput_samples: AtomicU32,
}

async fn resolve(target: &str) -> Result<SocketAddr, String> {
    if let Ok(a) = target.parse() {
        return Ok(a);
    }
    match tokio::net::lookup_host(target).await {
        Ok(mut addrs) => addrs
            .next()
            .ok_or_else(|| format!("No address resolved for {target}")),
        Err(e) => Err(format!("DNS error for {target}: {e}")),
    }
}

enum Pacing {
    /// Send each probe as soon as the previous echo arrives (or times out) —
    /// the plain `udp` probe behavior; used for the unloaded baseline.
    BackToBack { timeout_ms: u64 },
    /// Send probes on a fixed cadence regardless of echo arrival; used during
    /// the loaded phase so samples track queue growth over the whole window.
    /// After the last send, wait up to `grace_ms` for outstanding echoes.
    Paced { interval_ms: u64, grace_ms: u64 },
}

/// Send `count` seq-stamped echo probes and return per-probe RTTs
/// (None = lost). Echoes are matched by embedded sequence id.
async fn echo_rtts(
    target_addr: SocketAddr,
    count: u32,
    payload_size: usize,
    pacing: Pacing,
) -> Result<Vec<Option<f64>>, String> {
    let bind_addr: SocketAddr = if target_addr.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let socket = UdpSocket::bind(bind_addr)
        .await
        .map_err(|e| format!("UDP bind failed: {e}"))?;
    socket
        .connect(target_addr)
        .await
        .map_err(|e| format!("UDP connect failed: {e}"))?;

    let mut send_times: Vec<Option<Instant>> = vec![None; count as usize];
    let mut probe_rtts: Vec<Option<f64>> = vec![None; count as usize];

    let payload_size = payload_size.max(12); // seq (4) + timestamp (8)
    let mut send_buf = vec![0u8; payload_size];

    for seq in 0..count {
        let now_us = Utc::now().timestamp_micros();
        send_buf[..4].copy_from_slice(&seq.to_be_bytes());
        send_buf[4..12].copy_from_slice(&now_us.to_be_bytes());

        let sent_at = Instant::now();
        if socket.send(&send_buf).await.is_ok() {
            send_times[seq as usize] = Some(sent_at);
        }
        match pacing {
            Pacing::BackToBack { timeout_ms } => {
                // Wait for this probe's echo (crediting any outstanding ones).
                let deadline = sent_at + Duration::from_millis(timeout_ms);
                recv_echoes(&socket, deadline, Some(seq), &send_times, &mut probe_rtts).await;
            }
            Pacing::Paced { interval_ms, .. } => {
                // Drain echoes for exactly one cadence interval, then send the
                // next probe whether or not this one came back yet.
                let deadline = sent_at + Duration::from_millis(interval_ms);
                recv_echoes(&socket, deadline, None, &send_times, &mut probe_rtts).await;
            }
        }
    }

    // Grace drain: paced probes near the window's end may still have echoes in
    // flight (loaded RTTs can exceed the cadence interval).
    if let Pacing::Paced { grace_ms, .. } = pacing {
        if probe_rtts.iter().any(|r| r.is_none()) {
            let deadline = Instant::now() + Duration::from_millis(grace_ms);
            recv_echoes(&socket, deadline, None, &send_times, &mut probe_rtts).await;
        }
    }

    Ok(probe_rtts)
}

/// Receive echoes until `deadline` (or, when `until_seq` is set, until that
/// probe's echo has been credited). Every received datagram is matched by its
/// embedded sequence id against `send_times` — late, reordered, or duplicated
/// echoes are credited to the probe that actually sent them (trust audit V12).
async fn recv_echoes(
    socket: &UdpSocket,
    deadline: Instant,
    until_seq: Option<u32>,
    send_times: &[Option<Instant>],
    probe_rtts: &mut [Option<f64>],
) {
    let mut recv_buf = vec![0u8; 4096];
    loop {
        if let Some(seq) = until_seq {
            if probe_rtts[seq as usize].is_some() {
                return;
            }
        } else if probe_rtts.iter().all(|r| r.is_some()) {
            return; // grace/interval drain: nothing outstanding
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        match tokio::time::timeout(deadline - now, socket.recv(&mut recv_buf)).await {
            Ok(Ok(n)) if n >= 4 => {
                let echo_seq = u32::from_be_bytes(recv_buf[..4].try_into().unwrap());
                let idx = echo_seq as usize;
                if idx < probe_rtts.len() {
                    if let (Some(sent_at), None) = (send_times[idx], probe_rtts[idx]) {
                        probe_rtts[idx] = Some(sent_at.elapsed().as_secs_f64() * 1000.0);
                    }
                    // else: duplicate or not-yet-sent seq — ignore.
                }
            }
            Ok(Ok(_)) => {}       // runt datagram — ignore
            Ok(Err(_)) => return, // socket error — give up on this window
            Err(_) => return,     // deadline reached
        }
    }
}

fn rpm_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Rpm,
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
            category: ErrorCategory::Udp,
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
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_echo_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        server.set_nonblocking(true).unwrap();
        let server = UdpSocket::from_std(server).unwrap();
        let handle = tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while let Ok((n, from)) = server.recv_from(&mut buf).await {
                let _ = server.send_to(&buf[..n], from).await;
            }
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn paced_echo_rtts_credits_all_probes() {
        let (addr, server) = spawn_echo_server();
        let rtts = echo_rtts(
            addr,
            5,
            64,
            Pacing::Paced {
                interval_ms: 20,
                grace_ms: 500,
            },
        )
        .await
        .expect("echo_rtts should not error");
        server.abort();
        assert_eq!(rtts.len(), 5);
        assert!(
            rtts.iter().all(|r| r.is_some()),
            "loopback paced probes must all be credited: {rtts:?}"
        );
    }

    #[tokio::test]
    async fn back_to_back_echo_rtts_credits_all_probes() {
        let (addr, server) = spawn_echo_server();
        let rtts = echo_rtts(addr, 5, 64, Pacing::BackToBack { timeout_ms: 2000 })
            .await
            .expect("echo_rtts should not error");
        server.abort();
        assert!(rtts.iter().all(|r| r.is_some()), "got {rtts:?}");
    }

    #[tokio::test]
    async fn paced_echo_rtts_no_server_all_lost() {
        let addr: SocketAddr = "127.0.0.1:19877".parse().unwrap(); // nothing listening
        let rtts = echo_rtts(
            addr,
            3,
            64,
            Pacing::Paced {
                interval_ms: 20,
                grace_ms: 100,
            },
        )
        .await
        .expect("echo_rtts should not error");
        assert!(rtts.iter().all(|r| r.is_none()), "got {rtts:?}");
    }

    #[tokio::test]
    async fn rpm_probe_fails_cleanly_when_udp_unreachable() {
        // No echo server AND no download endpoint: the unloaded phase loses
        // every probe, so the attempt must fail with a Udp-category error and
        // carry no RpmResult.
        let cfg = RpmProbeConfig {
            udp: UdpProbeConfig {
                target_host: "127.0.0.1".into(),
                target_port: 19878, // nothing listening
                probe_count: 3,
                timeout_ms: 100,
                payload_size: 64,
            },
            throughput: ThroughputConfig {
                run_cfg: crate::runner::http::RunConfig {
                    dns_enabled: false,
                    timeout_ms: 1_000,
                    ..Default::default()
                },
                base_url: url::Url::parse("http://127.0.0.1:19879/").unwrap(),
            },
            download_bytes: 1024,
            load_duration_ms: 300,
            probe_interval_ms: 50,
        };
        let attempt = run_rpm_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!attempt.success);
        assert_eq!(attempt.protocol, Protocol::Rpm);
        assert!(attempt.rpm.is_none());
        let err = attempt.error.expect("error must be set");
        assert_eq!(err.category, ErrorCategory::Udp);
        assert!(err.message.contains("Unloaded phase"), "{}", err.message);
    }

    #[test]
    fn from_parts_uses_documented_defaults() {
        let udp = UdpProbeConfig::default();
        let throughput = ThroughputConfig {
            run_cfg: crate::runner::http::RunConfig::default(),
            base_url: url::Url::parse("http://127.0.0.1:8080/").unwrap(),
        };
        let cfg = RpmProbeConfig::from_parts(udp, throughput);
        assert_eq!(cfg.download_bytes, DEFAULT_RPM_DOWNLOAD_BYTES);
        assert_eq!(cfg.load_duration_ms, DEFAULT_RPM_LOAD_DURATION_MS);
        assert_eq!(cfg.probe_interval_ms, DEFAULT_RPM_PROBE_INTERVAL_MS);
    }
}
