/// UDP echo probe – sends N datagrams and measures round-trip times.
///
/// Wire format (per datagram, big-endian):
///   [4 bytes: seq u32] [8 bytes: timestamp_us i64] [padding to payload_size]
///
/// The server echoes back the entire datagram unchanged.
use crate::metrics::{
    aggregate_udp_rtts, ErrorCategory, ErrorRecord, Protocol, RequestAttempt, UdpResult,
};
use chrono::Utc;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::debug;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UdpProbeConfig {
    pub target_host: String,
    pub target_port: u16,
    pub probe_count: u32,
    pub timeout_ms: u64,
    pub payload_size: usize,
}

impl Default for UdpProbeConfig {
    fn default() -> Self {
        Self {
            target_host: "127.0.0.1".to_string(),
            target_port: 9999,
            probe_count: 10,
            timeout_ms: 5000,
            payload_size: 64,
        }
    }
}

pub async fn run_udp_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &UdpProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let target = format!("{}:{}", cfg.target_host, cfg.target_port);
    let target_addr: SocketAddr = match target.parse() {
        Ok(a) => a,
        Err(_) => {
            // Try DNS resolution for hostnames
            match tokio::net::lookup_host(&target).await {
                Ok(mut addrs) => match addrs.next() {
                    Some(a) => a,
                    None => {
                        return udp_failed(
                            run_id,
                            attempt_id,
                            sequence_num,
                            started_at,
                            format!("No address resolved for {target}"),
                        )
                    }
                },
                Err(e) => {
                    return udp_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        started_at,
                        format!("DNS error for {target}: {e}"),
                    )
                }
            }
        }
    };

    let bind_addr: SocketAddr = if target_addr.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };

    let socket = match UdpSocket::bind(bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            return udp_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                format!("UDP bind failed: {e}"),
            )
        }
    };

    if let Err(e) = socket.connect(target_addr).await {
        return udp_failed(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            format!("UDP connect failed: {e}"),
        );
    }

    // Per-seq send times and results. Echoes are matched by their embedded
    // sequence id against the outstanding-probe table, so a late, reordered,
    // or duplicated echo is credited to the probe that actually sent it —
    // it can no longer desync the matcher and cascade false loss onto every
    // subsequent probe. (Trust audit V12.)
    let mut send_times: Vec<Option<Instant>> = vec![None; cfg.probe_count as usize];
    let mut probe_rtts: Vec<Option<f64>> = vec![None; cfg.probe_count as usize];

    let payload_size = cfg.payload_size.max(12); // need at least seq + ts
    let mut send_buf = vec![0u8; payload_size];

    for seq in 0..cfg.probe_count {
        // Header: seq (4 bytes) | timestamp_us (8 bytes)
        let now_us = Utc::now().timestamp_micros();
        send_buf[..4].copy_from_slice(&seq.to_be_bytes());
        send_buf[4..12].copy_from_slice(&now_us.to_be_bytes());

        let sent_at = Instant::now();
        if socket.send(&send_buf).await.is_ok() {
            send_times[seq as usize] = Some(sent_at);
            let deadline = sent_at + Duration::from_millis(cfg.timeout_ms);
            recv_outstanding_echoes(&socket, seq, deadline, &send_times, &mut probe_rtts).await;
        }
        debug!(
            "UDP probe {seq}/{}: {}",
            cfg.probe_count,
            probe_rtts[seq as usize]
                .map(|r| format!("{r:.2}ms"))
                .as_deref()
                .unwrap_or("LOST")
        );
    }

    let stats = aggregate_udp_rtts(&probe_rtts);
    let success_count = probe_rtts.iter().filter(|r| r.is_some()).count() as u32;
    let loss_pattern = crate::metrics::compute_loss_pattern(&probe_rtts);

    // Local-drop vs path-loss split (B.6): one getsockopt at train end — the
    // socket is fresh, so the cumulative kernel drop counter is per-train.
    // No per-datagram syscalls are added to the echo hot path.
    let diag = crate::runner::socket_info::UdpSocketDiag::from_socket(&socket);

    let result = UdpResult {
        remote_addr: target_addr.to_string(),
        probe_count: cfg.probe_count,
        success_count,
        loss_percent: stats.loss_percent,
        rtt_min_ms: stats.min,
        rtt_avg_ms: stats.avg,
        rtt_p95_ms: stats.p95,
        jitter_ms: stats.jitter,
        ipdv_p95_ms: stats.ipdv_p95,
        ipdv_p99_ms: stats.ipdv_p99,
        started_at,
        probe_rtts_ms: probe_rtts,
        local_drops: diag.local_drops,
        so_rcvbuf_bytes: diag.so_rcvbuf_bytes,
        loss_pattern,
    };

    // Zero echoes back is ambiguous from the wire alone — 100% path loss, a
    // firewall, or (most common in the field) nothing speaking the echo
    // protocol at the target: the probe requires a networker-endpoint echo
    // server, and against an arbitrary URL every probe is "lost" by
    // construction. Say so instead of leaving a bare failed attempt
    // (user-caught 2026-08-12: 0/10 with no explanation).
    let error = if success_count == 0 {
        Some(ErrorRecord {
            category: ErrorCategory::Udp,
            message: format!(
                "No UDP echo replies from {target_addr} ({count} probes): either the path \
                 drops UDP or nothing is listening for the echo protocol there — this mode \
                 requires a LagHound endpoint's echo server, so 100% loss is expected \
                 against ordinary websites",
                count = cfg.probe_count,
            ),
            detail: None,
            occurred_at: Utc::now(),
        })
    } else {
        None
    };

    RequestAttempt {
        target_url: None,
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Udp,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: success_count > 0,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: Some(result),
        error,
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
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Receive echoes until `current_seq`'s echo has been credited or `deadline`
/// passes.
///
/// Every received datagram is matched by its embedded sequence id against
/// `send_times`: an echo for ANY outstanding probe (late, reordered) is
/// credited to that probe using its own send time; duplicates and unknown
/// sequence ids are ignored and the wait continues. The previous
/// implementation did a single `recv` per probe and marked the CURRENT probe
/// lost whenever the one datagram it read carried a different seq — so one
/// delayed echo desynced the matcher and every subsequent probe was falsely
/// counted lost. (Trust audit V12.)
async fn recv_outstanding_echoes(
    socket: &UdpSocket,
    current_seq: u32,
    deadline: Instant,
    send_times: &[Option<Instant>],
    probe_rtts: &mut [Option<f64>],
) {
    let mut recv_buf = vec![0u8; 4096];
    while probe_rtts[current_seq as usize].is_none() {
        let now = Instant::now();
        if now >= deadline {
            return; // current probe's window expired
        }
        match tokio::time::timeout(deadline - now, socket.recv(&mut recv_buf)).await {
            Ok(Ok(n)) if n >= 4 => {
                let echo_seq = u32::from_be_bytes(recv_buf[..4].try_into().unwrap());
                let idx = echo_seq as usize;
                if idx < probe_rtts.len() {
                    if let (Some(sent_at), None) = (send_times[idx], probe_rtts[idx]) {
                        // Credit the echo to the probe that sent it, timed
                        // from that probe's own send instant.
                        probe_rtts[idx] = Some(sent_at.elapsed().as_secs_f64() * 1000.0);
                    }
                    // else: duplicate echo or echo for a not-yet-sent seq — ignore.
                }
                // Keep waiting for the current probe's echo.
            }
            Ok(Ok(_)) => {}       // runt datagram (< 4 bytes) — ignore
            Ok(Err(_)) => return, // socket error — give up on this window
            Err(_) => return,     // deadline reached
        }
    }
}

fn udp_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        target_url: None,
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Udp,
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
        mthroughput: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Spin up a local UDP echo server and run the probe against it.
    #[tokio::test]
    async fn udp_probe_loopback_echo() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();

        // Spawn echo server
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            for _ in 0..20 {
                match server.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        let _ = server.send_to(&buf[..n], addr).await;
                    }
                    Err(_) => break,
                }
            }
        });

        let cfg = UdpProbeConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: server_port,
            probe_count: 5,
            timeout_ms: 2000,
            payload_size: 64,
        };

        let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(attempt.success, "UDP probe failed: {:?}", attempt.error);
        let udp = attempt.udp.unwrap();
        assert_eq!(udp.probe_count, 5);
        assert_eq!(udp.success_count, 5);
        assert_eq!(udp.loss_percent, 0.0);
        assert!(udp.rtt_avg_ms >= 0.0);
    }

    #[tokio::test]
    async fn udp_probe_no_server_records_loss() {
        let cfg = UdpProbeConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: 19876, // nothing listening
            probe_count: 3,
            timeout_ms: 200,
            payload_size: 64,
        };

        let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
        // success=false because all probes lost
        let udp = attempt.udp.as_ref().unwrap();
        assert_eq!(udp.success_count, 0);
        assert_eq!(udp.loss_percent, 100.0);
        // Zero echoes must carry an explanation — the most common cause is
        // "no echo server at the target" and a bare failed attempt reads as
        // a network problem (user-caught 2026-08-12).
        assert!(!attempt.success);
        let err = attempt
            .error
            .as_ref()
            .expect("all-lost attempt must explain itself");
        assert_eq!(err.category, ErrorCategory::Udp);
        assert!(
            err.message.contains("echo"),
            "message names the echo protocol: {}",
            err.message
        );
    }

    /// Partial loss is a measurement, not an error — the error record is
    /// reserved for the zero-information case.
    #[tokio::test]
    async fn udp_probe_partial_echoes_carry_no_error() {
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let mut n_seen = 0u32;
            while let Ok((n, addr)) = server.recv_from(&mut buf).await {
                n_seen += 1;
                // Echo only every second datagram: guaranteed partial loss.
                if n_seen % 2 == 1 {
                    let _ = server.send_to(&buf[..n], addr).await;
                }
            }
        });

        let cfg = UdpProbeConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: server_port,
            probe_count: 4,
            timeout_ms: 500,
            payload_size: 64,
        };

        let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(attempt.success);
        assert!(
            attempt.error.is_none(),
            "partial loss must not set an error"
        );
        let udp = attempt.udp.unwrap();
        assert!(udp.success_count > 0 && udp.success_count < 4);
    }

    #[test]
    fn udp_failed_sets_correct_fields() {
        let run_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let started_at = chrono::Utc::now();
        let a = udp_failed(run_id, attempt_id, 3, started_at, "bind failed".to_string());
        assert!(!a.success);
        assert_eq!(a.run_id, run_id);
        assert_eq!(a.attempt_id, attempt_id);
        assert_eq!(a.sequence_num, 3);
        assert_eq!(a.protocol, Protocol::Udp);
        assert!(a.udp.is_none());
        assert!(a.tcp.is_none());
        assert!(a.dns.is_none());
        let err = a.error.expect("error should be set");
        assert_eq!(err.message, "bind failed");
        assert_eq!(err.category, ErrorCategory::Udp);
        assert_eq!(a.retry_count, 0);
    }

    /// Regression test for trust-audit V12 (duplicates): a server that echoes
    /// every datagram TWICE must not cause any loss. The pre-fix matcher did
    /// one recv per probe, so probe N+1's single recv consumed the duplicate
    /// echo of probe N, failed the seq check, and marked N+1 lost — cascading
    /// so that only probe 0 succeeded (80% false loss on 5 probes).
    #[tokio::test]
    async fn udp_probe_duplicate_echoes_do_not_cause_false_loss() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            for _ in 0..10 {
                match server.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        // Echo twice — duplicate delivery.
                        let _ = server.send_to(&buf[..n], addr).await;
                        let _ = server.send_to(&buf[..n], addr).await;
                    }
                    Err(_) => break,
                }
            }
        });

        let cfg = UdpProbeConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: server_port,
            probe_count: 5,
            timeout_ms: 2000,
            payload_size: 64,
        };
        let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(attempt.success, "probe failed: {:?}", attempt.error);
        let udp = attempt.udp.unwrap();
        assert_eq!(
            udp.success_count, 5,
            "duplicate echoes desynced the matcher (cascading false loss): {:?}",
            udp.probe_rtts_ms
        );
        assert_eq!(udp.loss_percent, 0.0);
    }

    /// Regression test for trust-audit V12 (reordering / late echo): the
    /// server delays the echo of the FIRST probe until after it has echoed
    /// the second. The late echo must be credited to probe 0 (matched by its
    /// sequence id, timed from probe 0's own send instant) instead of being
    /// misread during a later probe's window and marking that probe lost.
    #[tokio::test]
    async fn udp_probe_reordered_echo_credited_to_original_probe() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            // Hold probe 0's echo until probe 1 arrives.
            let (n0, addr) = server.recv_from(&mut buf).await.unwrap();
            let held = buf[..n0].to_vec();
            let (n1, addr1) = server.recv_from(&mut buf).await.unwrap();
            let _ = server.send_to(&buf[..n1], addr1).await; // echo seq 1 first
            let _ = server.send_to(&held, addr).await; // then late echo of seq 0
                                                       // Echo the rest normally.
            for _ in 0..8 {
                match server.recv_from(&mut buf).await {
                    Ok((n, a)) => {
                        let _ = server.send_to(&buf[..n], a).await;
                    }
                    Err(_) => break,
                }
            }
        });

        let cfg = UdpProbeConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: server_port,
            probe_count: 4,
            timeout_ms: 500,
            payload_size: 64,
        };
        let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
        let udp = attempt.udp.unwrap();
        assert_eq!(
            udp.success_count, 4,
            "reordered echo must not mark any probe lost: {:?}",
            udp.probe_rtts_ms
        );
        assert_eq!(udp.loss_percent, 0.0);
        // Probe 0's RTT includes the deliberate hold (>= probe 0's full
        // timeout window since its echo only came back during probe 1's
        // window) — proving it was timed from probe 0's OWN send instant.
        let rtt0 = udp.probe_rtts_ms[0].expect("late echo credited to probe 0");
        assert!(
            rtt0 >= 400.0,
            "late echo must be timed from probe 0's send instant, got {rtt0:.2}ms"
        );
    }

    #[tokio::test]
    async fn udp_probe_all_responses_sets_zero_loss() {
        // A perfect echo server: 5 probes, 0 loss.
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            for _ in 0..10 {
                match server.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        let _ = server.send_to(&buf[..n], addr).await;
                    }
                    Err(_) => break,
                }
            }
        });
        let cfg = UdpProbeConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: server_port,
            probe_count: 5,
            timeout_ms: 2000,
            payload_size: 32,
        };
        let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(attempt.success);
        let udp = attempt.udp.unwrap();
        assert_eq!(udp.loss_percent, 0.0);
        // All 5 RTT entries should be Some.
        assert_eq!(udp.probe_rtts_ms.len(), 5);
        assert!(udp.probe_rtts_ms.iter().all(|r| r.is_some()));
    }
}
