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

    let mut probe_rtts: Vec<Option<f64>> = Vec::with_capacity(cfg.probe_count as usize);

    for seq in 0..cfg.probe_count {
        let rtt = send_probe(&socket, seq, cfg).await;
        debug!(
            "UDP probe {seq}/{}: {:?}",
            cfg.probe_count,
            rtt.map(|r| format!("{r:.2}ms"))
                .as_deref()
                .unwrap_or("LOST")
        );
        probe_rtts.push(rtt);
    }

    let stats = aggregate_udp_rtts(&probe_rtts);
    let success_count = probe_rtts.iter().filter(|r| r.is_some()).count() as u32;

    let result = UdpResult {
        remote_addr: target_addr.to_string(),
        probe_count: cfg.probe_count,
        success_count,
        loss_percent: stats.loss_percent,
        rtt_min_ms: stats.min,
        rtt_avg_ms: stats.avg,
        rtt_p95_ms: stats.p95,
        jitter_ms: stats.jitter,
        started_at,
        probe_rtts_ms: probe_rtts,
    };

    RequestAttempt {
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
        error: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Send one probe, wait for echo.  Returns RTT in ms or None on timeout/error.
async fn send_probe(socket: &UdpSocket, seq: u32, cfg: &UdpProbeConfig) -> Option<f64> {
    let payload_size = cfg.payload_size.max(12); // need at least seq + ts
    let mut buf = vec![0u8; payload_size];

    // Header: seq (4 bytes) | timestamp_us (8 bytes)
    let now_us = Utc::now().timestamp_micros();
    buf[..4].copy_from_slice(&seq.to_be_bytes());
    buf[4..12].copy_from_slice(&now_us.to_be_bytes());

    let t0 = Instant::now();
    if socket.send(&buf).await.is_err() {
        return None;
    }

    let mut recv_buf = vec![0u8; 4096];
    match tokio::time::timeout(
        Duration::from_millis(cfg.timeout_ms),
        socket.recv(&mut recv_buf),
    )
    .await
    {
        Ok(Ok(n)) if n >= 4 => {
            // Verify echo has the same sequence number
            let echo_seq = u32::from_be_bytes(recv_buf[..4].try_into().ok()?);
            if echo_seq == seq {
                Some(t0.elapsed().as_secs_f64() * 1000.0)
            } else {
                // Out-of-order; treat as lost for simplicity
                None
            }
        }
        _ => None,
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
    }
}
