//! STAMP Session-Sender (RFC 8762, unauthenticated mode) — `stamp` mode.
//!
//! Sends 44-byte Session-Sender test packets at a fixed cadence (periodic
//! stream per RFC 3432 — send times are NOT conditioned on echo arrival) to
//! the networker-endpoint's Session-Reflector (default UDP port 9997) and
//! computes, from the reflected timestamps and sequence numbers:
//!
//! - **Processing-corrected RTT**: `(T4 − T1) − (T3 − T2)` (RFC 8762 §4.4's
//!   arithmetic) — the reflector's own processing time is subtracted, and
//!   because T1/T4 come from the sender clock and T2/T3 from the reflector
//!   clock, NO clock synchronization is needed. T4−T1 is taken from the
//!   sender's monotonic clock (immune to wall-clock steps).
//! - **Per-direction delay variation**: forward readings `T2 − T1` and
//!   return readings `T4 − T3` each embed a constant (over the train) clock
//!   offset, which cancels in consecutive differences — so IPDV (RFC 3393,
//!   consecutive-by-sequence selection) is exact per direction without sync.
//! - **Directional loss** (RFC 8762 §4.2 stateful reflector sequence): the
//!   highest reflector sequence observed says how many probes REACHED the
//!   reflector → `sent − (max_reflector_seq + 1)` is forward loss and
//!   `(max_reflector_seq + 1) − replies` is reverse loss. (If the reply
//!   carrying the highest reflector sequence is itself lost on the return
//!   path, that probe is attributed to the return direction — the standard
//!   tail ambiguity of the method; totals always reconcile.)
//!
//! Absolute one-way delay is NOT derivable from this exchange alone; when the
//! run performed its SNTP clock-sync query, `target_runner` fills the
//! `owd_*_est_ms` fields from the raw readings — explicitly labeled an
//! estimate with the offset's ±(delay/2) uncertainty attached.
//!
//! Timestamps are NTP 64-bit (RFC 8762's default format; the PTP option is
//! not used). Loss threshold: every probe gets its full `--udp-timeout`
//! (same Tmax discipline as the `udp`/`rpm` probes — no censoring).

use crate::metrics::{
    aggregate_udp_rtts, ErrorCategory, ErrorRecord, Protocol, RequestAttempt, StampResult,
};
use chrono::Utc;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Unauthenticated STAMP test packets are exactly 44 bytes (RFC 8762 §4.2).
pub const STAMP_PACKET_LEN: usize = 44;
/// Default Session-Reflector port on the networker-endpoint.
pub const DEFAULT_STAMP_PORT: u16 = 9997;
/// Default probe count: 50 → enough samples for honest p95s on RTT and IPDV
/// (the project's `MIN_SAMPLES_P95 = 20` gate) in a 2.5 s train.
pub const DEFAULT_STAMP_PROBES: u32 = 50;
/// Default cadence: one probe every 50 ms (periodic per RFC 3432).
pub const DEFAULT_STAMP_INTERVAL_MS: u64 = 50;

/// Minimum |IPDV| deltas before a p95 is reported (mirrors the project's
/// `MIN_SAMPLES_P95` honesty gate — a p95 of 10 samples is just the max).
const MIN_IPDV_P95_SAMPLES: usize = 20;

const NTP_UNIX_OFFSET_SECS: u64 = 2_208_988_800;

#[derive(Debug, Clone)]
pub struct StampProbeConfig {
    pub target_host: String,
    pub target_port: u16,
    pub probe_count: u32,
    /// Send cadence (ms).
    pub interval_ms: u64,
    /// Per-probe loss threshold Tmax (ms) — same semantics as `--udp-timeout`.
    pub timeout_ms: u64,
}

impl Default for StampProbeConfig {
    fn default() -> Self {
        Self {
            target_host: "127.0.0.1".into(),
            target_port: DEFAULT_STAMP_PORT,
            probe_count: DEFAULT_STAMP_PROBES,
            interval_ms: DEFAULT_STAMP_INTERVAL_MS,
            timeout_ms: 5_000,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NTP 64-bit timestamp helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Current wall clock as an NTP 64-bit timestamp (seconds, fraction).
fn ntp_now() -> (u32, u32) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = (now.as_secs() + NTP_UNIX_OFFSET_SECS) as u32;
    let frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs, frac as u32)
}

/// NTP timestamp → seconds as f64 (0.5 µs resolution at current epochs —
/// far below the ms-scale quantities measured here).
fn ntp_to_secs(secs: u32, frac: u32) -> f64 {
    secs as f64 + (frac as f64) / 4_294_967_296.0
}

fn read_ntp(buf: &[u8], off: usize) -> f64 {
    let secs = u32::from_be_bytes(buf[off..off + 4].try_into().unwrap());
    let frac = u32::from_be_bytes(buf[off + 4..off + 8].try_into().unwrap());
    ntp_to_secs(secs, frac)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// One credited reply's derived readings.
#[derive(Debug, Clone, Copy)]
struct Reply {
    /// Processing-corrected RTT (ms): mono(T4−T1) − (T3−T2).
    rtt_corrected_ms: f64,
    /// Reflector processing time T3−T2 (ms).
    processing_ms: f64,
    /// Raw forward reading T2−T1 (ms) — includes clock offset.
    fwd_raw_ms: f64,
    /// Raw return reading T4−T3 (ms) — includes clock offset (opposite sign).
    rev_raw_ms: f64,
    reflector_seq: u32,
}

pub async fn run_stamp_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &StampProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let target = format!("{}:{}", cfg.target_host, cfg.target_port);
    let target_addr: SocketAddr = match resolve(&target).await {
        Ok(a) => a,
        Err(msg) => return stamp_failed(run_id, attempt_id, sequence_num, started_at, msg),
    };

    let replies = match probe_train(target_addr, cfg).await {
        Ok(r) => r,
        Err(msg) => return stamp_failed(run_id, attempt_id, sequence_num, started_at, msg),
    };

    let probes_sent = cfg.probe_count;
    let received: Vec<(u32, Reply)> = replies
        .iter()
        .enumerate()
        .filter_map(|(seq, r)| r.map(|r| (seq as u32, r)))
        .collect();
    let replies_received = received.len() as u32;

    if replies_received == 0 {
        return stamp_failed(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            format!(
                "All {probes_sent} STAMP probes lost (is the endpoint's Session-Reflector \
                 on {target} reachable? It listens on UDP port 9997 by default)"
            ),
        );
    }

    // ── Processing-corrected RTT aggregate (per-probe timeline preserved) ────
    let probe_rtts_ms: Vec<Option<f64>> = replies
        .iter()
        .map(|r| r.map(|r| r.rtt_corrected_ms))
        .collect();
    let stats = aggregate_udp_rtts(&probe_rtts_ms);

    // ── Directional loss (RFC 8762 §4.2) ─────────────────────────────────────
    let max_reflector_seq = received.iter().map(|(_, r)| r.reflector_seq).max();
    let (loss_sent_percent, loss_return_percent) = match max_reflector_seq {
        Some(max_seq) => {
            let reached = (max_seq as u64 + 1).min(probes_sent as u64) as u32;
            let fwd_lost = probes_sent.saturating_sub(reached);
            let rev_lost = reached.saturating_sub(replies_received);
            (
                Some(fwd_lost as f64 / probes_sent as f64 * 100.0),
                Some(rev_lost as f64 / reached.max(1) as f64 * 100.0),
            )
        }
        None => (None, None),
    };

    // ── Per-direction delay variation (offsets cancel per direction) ─────────
    let fwd_readings: Vec<f64> = received.iter().map(|(_, r)| r.fwd_raw_ms).collect();
    let rev_readings: Vec<f64> = received.iter().map(|(_, r)| r.rev_raw_ms).collect();
    let (near_ipdv_mean_ms, near_ipdv_p95_ms) = ipdv_stats(&fwd_readings);
    let (far_ipdv_mean_ms, far_ipdv_p95_ms) = ipdv_stats(&rev_readings);

    let mean = |v: &[f64]| -> Option<f64> {
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    let processing: Vec<f64> = received.iter().map(|(_, r)| r.processing_ms).collect();

    let result = StampResult {
        remote_addr: target_addr.to_string(),
        probes_sent,
        replies_received,
        loss_percent: stats.loss_percent,
        loss_sent_percent,
        loss_return_percent,
        rtt_min_ms: stats.min,
        rtt_avg_ms: stats.avg,
        rtt_p95_ms: stats.p95,
        jitter_ms: stats.jitter,
        near_ipdv_mean_ms,
        near_ipdv_p95_ms,
        far_ipdv_mean_ms,
        far_ipdv_p95_ms,
        reflector_processing_avg_us: mean(&processing).map(|ms| ms * 1000.0),
        reflector_seq_max: max_reflector_seq,
        near_owd_raw_avg_ms: mean(&fwd_readings),
        far_owd_raw_avg_ms: mean(&rev_readings),
        // Filled by target_runner when the run has an SNTP clock-sync result.
        owd_forward_est_ms: None,
        owd_return_est_ms: None,
        owd_uncertainty_ms: None,
        probe_rtts_ms,
        interval_ms: cfg.interval_ms,
        started_at,
    };

    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Stamp,
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
        stamp: Some(result),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Probe train
// ─────────────────────────────────────────────────────────────────────────────

/// Send `cfg.probe_count` STAMP packets at the configured cadence and credit
/// replies by their echoed Session-Sender sequence (late/reordered/duplicate
/// replies are credited to the probe that sent them — V12 semantics). After
/// the last send, the drain stays open until every outstanding probe has had
/// its full Tmax (no censoring).
async fn probe_train(
    target_addr: SocketAddr,
    cfg: &StampProbeConfig,
) -> Result<Vec<Option<Reply>>, String> {
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

    let count = cfg.probe_count.max(1) as usize;
    let interval = Duration::from_millis(cfg.interval_ms.max(1));
    let tmax = Duration::from_millis(cfg.timeout_ms.max(1));

    // Per-probe send records: monotonic instant + the wall-clock T1 (secs).
    let mut send_mono: Vec<Option<Instant>> = vec![None; count];
    let mut send_t1: Vec<Option<f64>> = vec![None; count];
    let mut replies: Vec<Option<Reply>> = vec![None; count];

    let mut buf = [0u8; STAMP_PACKET_LEN];
    // Error Estimate: S=0 (wall clock not certified synchronized), mult 1.
    let err_est: [u8; 2] = [0x00, 0x01];

    for seq in 0..count {
        let (ntp_s, ntp_f) = ntp_now();
        buf[0..4].copy_from_slice(&(seq as u32).to_be_bytes());
        buf[4..8].copy_from_slice(&ntp_s.to_be_bytes());
        buf[8..12].copy_from_slice(&ntp_f.to_be_bytes());
        buf[12..14].copy_from_slice(&err_est);
        buf[14..].fill(0); // MBZ

        let sent_at = Instant::now();
        if socket.send(&buf).await.is_ok() {
            send_mono[seq] = Some(sent_at);
            send_t1[seq] = Some(ntp_to_secs(ntp_s, ntp_f));
        }
        // Drain replies for one cadence interval, then send the next probe
        // whether or not this one came back (periodic stream, RFC 3432).
        recv_replies(
            &socket,
            sent_at + interval,
            &send_mono,
            &send_t1,
            &mut replies,
        )
        .await;
    }

    // Tail drain: wait until the LAST probe's full Tmax has elapsed (every
    // earlier probe's Tmax expires sooner) — same no-censoring discipline as
    // rpm's loaded phase.
    if replies.iter().any(|r| r.is_none()) {
        if let Some(last_send) = send_mono.iter().flatten().max().copied() {
            recv_replies(
                &socket,
                last_send + tmax,
                &send_mono,
                &send_t1,
                &mut replies,
            )
            .await;
        }
    }

    Ok(replies)
}

/// Receive and credit reflected packets until `deadline` (or until nothing is
/// outstanding).
async fn recv_replies(
    socket: &UdpSocket,
    deadline: Instant,
    send_mono: &[Option<Instant>],
    send_t1: &[Option<f64>],
    replies: &mut [Option<Reply>],
) {
    let mut buf = [0u8; 2048];
    loop {
        if replies.iter().all(|r| r.is_some()) {
            return;
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        match tokio::time::timeout(deadline - now, socket.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= STAMP_PACKET_LEN => {
                let t4_mono = Instant::now();
                let t4_wall = {
                    let (s, f) = ntp_now();
                    ntp_to_secs(s, f)
                };
                let pkt = &buf[..n];
                let reflector_seq = u32::from_be_bytes(pkt[0..4].try_into().unwrap());
                let t3 = read_ntp(pkt, 4);
                let t2 = read_ntp(pkt, 16);
                let sender_seq = u32::from_be_bytes(pkt[24..28].try_into().unwrap()) as usize;
                if sender_seq >= replies.len() {
                    continue; // not ours / future seq — ignore
                }
                if let (Some(sent_mono), Some(t1), None) = (
                    send_mono[sender_seq],
                    send_t1[sender_seq],
                    replies[sender_seq],
                ) {
                    let rtt_mono_ms = t4_mono.duration_since(sent_mono).as_secs_f64() * 1000.0;
                    let processing_ms = ((t3 - t2) * 1000.0).max(0.0);
                    // Corrected RTT can mathematically go negative only via
                    // reflector clock noise inside the processing window;
                    // clamp at 0 rather than report a negative time.
                    let rtt_corrected_ms = (rtt_mono_ms - processing_ms).max(0.0);
                    replies[sender_seq] = Some(Reply {
                        rtt_corrected_ms,
                        processing_ms,
                        fwd_raw_ms: (t2 - t1) * 1000.0,
                        rev_raw_ms: (t4_wall - t3) * 1000.0,
                        reflector_seq,
                    });
                }
                // else: duplicate or unsent seq — ignored (dup evidence is a
                // separate RFC 5560 work item, m4 §2.5).
            }
            Ok(Ok(_)) => {}       // runt datagram — ignore
            Ok(Err(_)) => return, // socket error — end this window
            Err(_) => return,     // deadline
        }
    }
}

/// Mean |IPDV| and (sample-gated) p95 |IPDV| over consecutive-by-sequence
/// received readings (RFC 3393 §4.2 selection — same convention as the
/// project's other jitter figures).
fn ipdv_stats(readings: &[f64]) -> (Option<f64>, Option<f64>) {
    if readings.len() < 2 {
        return (None, None);
    }
    let mut deltas: Vec<f64> = readings.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
    let p95 = if deltas.len() >= MIN_IPDV_P95_SAMPLES {
        deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((deltas.len() as f64) * 0.95).ceil() as usize - 1;
        Some(deltas[idx.min(deltas.len() - 1)])
    } else {
        None
    };
    (Some(mean), p95)
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

fn stamp_failed(
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
        protocol: Protocol::Stamp,
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
        ping: None,
        path: None,
        dualstack: None,
        websocket: None,
        pmtud: None,
        responsiveness: None,
        stamp: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-test reflector implementing the RFC 8762 §4.3 layout with a
    /// stateful sequence counter and an optional injected processing delay.
    fn spawn_reflector(processing_delay: Duration) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        server.set_nonblocking(true).unwrap();
        let server = UdpSocket::from_std(server).unwrap();
        let handle = tokio::spawn(async move {
            let mut seq = 0u32;
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = server.recv_from(&mut buf).await {
                if n < STAMP_PACKET_LEN {
                    continue;
                }
                let (t2s, t2f) = ntp_now();
                if !processing_delay.is_zero() {
                    tokio::time::sleep(processing_delay).await;
                }
                let (t3s, t3f) = ntp_now();
                let mut out = [0u8; STAMP_PACKET_LEN];
                out[0..4].copy_from_slice(&seq.to_be_bytes());
                out[4..8].copy_from_slice(&t3s.to_be_bytes());
                out[8..12].copy_from_slice(&t3f.to_be_bytes());
                out[12..14].copy_from_slice(&[0x00, 0x01]);
                out[16..20].copy_from_slice(&t2s.to_be_bytes());
                out[20..24].copy_from_slice(&t2f.to_be_bytes());
                out[24..28].copy_from_slice(&buf[0..4]);
                out[28..36].copy_from_slice(&buf[4..12]);
                out[36..38].copy_from_slice(&buf[12..14]);
                out[40] = 255;
                seq = seq.wrapping_add(1);
                let _ = server.send_to(&out, from).await;
            }
        });
        (addr, handle)
    }

    #[test]
    fn ntp_round_trip_preserves_sub_ms_precision() {
        let (s, f) = ntp_now();
        let secs = ntp_to_secs(s, f);
        let frac_back = ((secs - s as f64) * 4_294_967_296.0) as i64;
        assert!((frac_back - f as i64).abs() < 10_000); // < ~2.4 µs
    }

    #[test]
    fn ipdv_stats_offsets_cancel() {
        // Readings with a huge constant clock offset: IPDV must see only the
        // variation, never the offset.
        let readings = [1_000_000.0, 1_000_002.0, 1_000_001.0];
        let (mean, p95) = ipdv_stats(&readings);
        assert!((mean.unwrap() - 1.5).abs() < 1e-9); // |2| and |−1| → mean 1.5
        assert!(p95.is_none(), "p95 must be gated below 20 deltas");
    }

    #[tokio::test]
    async fn stamp_probe_computes_corrected_rtt_and_directional_loss() {
        // 5 ms injected reflector processing: the corrected RTT must be far
        // below the raw RTT (which includes the delay).
        let (addr, server) = spawn_reflector(Duration::from_millis(5));
        let cfg = StampProbeConfig {
            target_host: addr.ip().to_string(),
            target_port: addr.port(),
            probe_count: 10,
            interval_ms: 10,
            timeout_ms: 2_000,
        };
        let attempt = run_stamp_probe(Uuid::new_v4(), 0, &cfg).await;
        server.abort();

        assert!(attempt.success, "{:?}", attempt.error);
        let s = attempt.stamp.expect("stamp result");
        assert_eq!(s.probes_sent, 10);
        assert_eq!(s.replies_received, 10);
        assert_eq!(s.loss_sent_percent, Some(0.0));
        assert_eq!(s.loss_return_percent, Some(0.0));
        // Corrected RTT excludes the injected 5 ms processing time.
        assert!(
            s.rtt_avg_ms < 5.0,
            "corrected RTT should exclude processing: {} ms",
            s.rtt_avg_ms
        );
        let proc_us = s.reflector_processing_avg_us.unwrap();
        assert!(
            proc_us > 3_000.0,
            "processing time must be visible: {proc_us} µs"
        );
        assert_eq!(s.reflector_seq_max, Some(9));
        assert!(s.near_ipdv_mean_ms.is_some());
        assert!(s.far_ipdv_mean_ms.is_some());
    }

    #[tokio::test]
    async fn stamp_probe_fails_cleanly_when_reflector_unreachable() {
        let cfg = StampProbeConfig {
            target_host: "127.0.0.1".into(),
            target_port: 19881, // nothing listening
            probe_count: 3,
            interval_ms: 10,
            timeout_ms: 100,
        };
        let attempt = run_stamp_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!attempt.success);
        assert_eq!(attempt.protocol, Protocol::Stamp);
        assert!(attempt.stamp.is_none());
        let err = attempt.error.expect("error record");
        assert_eq!(err.category, ErrorCategory::Udp);
        assert!(err.message.contains("STAMP"), "{}", err.message);
    }

    /// Forward vs reverse split: a reflector that silently DROPS every other
    /// received packet (forward loss from the sender's perspective is zero;
    /// the reflector seq still increments per RECEIVED packet, so unreflected
    /// receipts read as REVERSE loss — which is exactly where the loss
    /// happened: after the reflector, on the way back... actually dropped at
    /// the reflector = indistinguishable from return-path loss per RFC 8762;
    /// the test pins that accounting).
    #[tokio::test]
    async fn stamp_probe_attributes_reflector_drops_to_return_path() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        server.set_nonblocking(true).unwrap();
        let server = UdpSocket::from_std(server).unwrap();
        let handle = tokio::spawn(async move {
            let mut seq = 0u32;
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = server.recv_from(&mut buf).await {
                if n < STAMP_PACKET_LEN {
                    continue;
                }
                let my_seq = seq;
                seq = seq.wrapping_add(1);
                if my_seq % 2 == 1 {
                    continue; // received (counter advanced) but not reflected
                }
                let (t2s, t2f) = ntp_now();
                let mut out = [0u8; STAMP_PACKET_LEN];
                out[0..4].copy_from_slice(&my_seq.to_be_bytes());
                out[4..8].copy_from_slice(&t2s.to_be_bytes());
                out[8..12].copy_from_slice(&t2f.to_be_bytes());
                out[16..20].copy_from_slice(&t2s.to_be_bytes());
                out[20..24].copy_from_slice(&t2f.to_be_bytes());
                out[24..28].copy_from_slice(&buf[0..4]);
                out[28..36].copy_from_slice(&buf[4..12]);
                let _ = server.send_to(&out, from).await;
            }
        });

        let cfg = StampProbeConfig {
            target_host: addr.ip().to_string(),
            target_port: addr.port(),
            probe_count: 10,
            interval_ms: 10,
            timeout_ms: 500,
        };
        let attempt = run_stamp_probe(Uuid::new_v4(), 0, &cfg).await;
        handle.abort();

        let s = attempt.stamp.expect("stamp result");
        assert_eq!(s.replies_received, 5);
        // Reflector saw ALL 10 probes (max seq 8 is the last even one it
        // reflected; seq 9 was received but dropped — the tail ambiguity).
        assert_eq!(s.reflector_seq_max, Some(8));
        // Forward loss ≈ the one tail probe not provable as received;
        // everything else is attributed to the return direction.
        let fwd = s.loss_sent_percent.unwrap();
        let rev = s.loss_return_percent.unwrap();
        assert!(fwd <= 10.0 + 1e-9, "fwd {fwd}");
        assert!(rev > 30.0, "rev {rev}");
    }
}
