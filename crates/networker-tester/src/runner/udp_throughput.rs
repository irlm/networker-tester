/// UDP bulk throughput probes — udpdownload and udpupload.
///
/// Uses the same wire protocol as `networker-endpoint`'s UDP throughput server.
/// See `crates/networker-endpoint/src/udp_throughput.rs` for protocol docs.
///
/// ## Download flow
/// 1. Client → Server: CMD_DOWNLOAD (value = requested bytes)
/// 2. Server → Client: CMD_ACK
/// 3. Server → Client: data packets (seq 0 .. total_seqs-1)
/// 4. Server → Client: CMD_DONE
/// 5. Client computes throughput from first-data to CMD_DONE (or timeout)
///
/// ## Upload flow
/// 1. Client → Server: CMD_UPLOAD (value = total bytes to send)
/// 2. Server → Client: CMD_ACK
/// 3. Client → Server: data packets
/// 4. Client → Server: CMD_DONE
/// 5. Server → Client: CMD_REPORT (value = bytes received)
/// 6. Client computes throughput from first-data to CMD_REPORT (or timeout)
use crate::metrics::{ErrorCategory, ErrorRecord, Protocol, RequestAttempt, UdpThroughputResult};
use chrono::Utc;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::debug;
use uuid::Uuid;

const MAGIC: &[u8; 4] = b"NWKT";
const CMD_DOWNLOAD: u8 = 0x01;
const CMD_UPLOAD: u8 = 0x02;
const CMD_DONE: u8 = 0x04;
const CMD_ACK: u8 = 0x10;
const CMD_REPORT: u8 = 0x11;
const CTRL_LEN: usize = 12;
const DATA_HDR_LEN: usize = 8;
const CHUNK_SIZE: usize = 1400;

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UdpThroughputConfig {
    pub target_host: String,
    pub target_port: u16,
    pub timeout_ms: u64,
}

impl Default for UdpThroughputConfig {
    fn default() -> Self {
        Self {
            target_host: "127.0.0.1".to_string(),
            target_port: 9998,
            timeout_ms: 30_000,
        }
    }
}

/// Download `payload_bytes` from the UDP throughput server and measure throughput.
pub async fn run_udpdownload_probe(
    run_id: Uuid,
    sequence_num: u32,
    payload_bytes: usize,
    cfg: &UdpThroughputConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let (sock, remote_addr) = match connect_udp(cfg).await {
        Ok(r) => r,
        Err(msg) => {
            return udp_tp_failed(
                run_id,
                attempt_id,
                sequence_num,
                Protocol::UdpDownload,
                started_at,
                msg,
            );
        }
    };

    // Send CMD_DOWNLOAD
    let cmd = make_ctrl(CMD_DOWNLOAD, payload_bytes as u32);
    if let Err(e) = sock.send(&cmd).await {
        return udp_tp_failed(
            run_id,
            attempt_id,
            sequence_num,
            Protocol::UdpDownload,
            started_at,
            format!("send CMD_DOWNLOAD: {e}"),
        );
    }

    // Wait for CMD_ACK
    if let Err(msg) = wait_for_ack(&sock, cfg.timeout_ms).await {
        return udp_tp_failed(
            run_id,
            attempt_id,
            sequence_num,
            Protocol::UdpDownload,
            started_at,
            msg,
        );
    }

    // Receive data packets until CMD_DONE (or timeout).
    let total_seqs_hint = payload_bytes.div_ceil(CHUNK_SIZE) as u32;
    let (received_seqs, received_bytes, transfer_ms) =
        recv_download(&sock, total_seqs_hint, cfg.timeout_ms).await;

    let datagrams_sent = total_seqs_hint;
    let datagrams_received = received_seqs.len() as u32;
    let loss = loss_percent(datagrams_sent, datagrams_received);
    let throughput_mbps = mbps(received_bytes, transfer_ms);

    let result = UdpThroughputResult {
        remote_addr: remote_addr.to_string(),
        payload_bytes,
        datagrams_sent,
        datagrams_received,
        bytes_acked: None,
        loss_percent: loss,
        transfer_ms,
        throughput_mbps,
        started_at,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::UdpDownload,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: datagrams_received > 0,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: Some(result),
        page_load: None,
    }
}

/// Upload `payload_bytes` to the UDP throughput server and measure throughput.
pub async fn run_udpupload_probe(
    run_id: Uuid,
    sequence_num: u32,
    payload_bytes: usize,
    cfg: &UdpThroughputConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let (sock, remote_addr) = match connect_udp(cfg).await {
        Ok(r) => r,
        Err(msg) => {
            return udp_tp_failed(
                run_id,
                attempt_id,
                sequence_num,
                Protocol::UdpUpload,
                started_at,
                msg,
            );
        }
    };

    // Send CMD_UPLOAD
    let cmd = make_ctrl(CMD_UPLOAD, payload_bytes as u32);
    if let Err(e) = sock.send(&cmd).await {
        return udp_tp_failed(
            run_id,
            attempt_id,
            sequence_num,
            Protocol::UdpUpload,
            started_at,
            format!("send CMD_UPLOAD: {e}"),
        );
    }

    // Wait for CMD_ACK
    if let Err(msg) = wait_for_ack(&sock, cfg.timeout_ms).await {
        return udp_tp_failed(
            run_id,
            attempt_id,
            sequence_num,
            Protocol::UdpUpload,
            started_at,
            msg,
        );
    }

    // Send data packets and CMD_DONE; measure transfer window.
    let (datagrams_sent, transfer_ms, bytes_acked) =
        send_upload(&sock, payload_bytes, cfg.timeout_ms).await;

    let total_seqs = payload_bytes.div_ceil(CHUNK_SIZE) as u32;
    let loss = loss_percent(total_seqs, datagrams_sent);
    let throughput_mbps = mbps(payload_bytes, transfer_ms);

    let result = UdpThroughputResult {
        remote_addr: remote_addr.to_string(),
        payload_bytes,
        datagrams_sent,
        datagrams_received: datagrams_sent,
        bytes_acked,
        loss_percent: loss,
        transfer_ms,
        throughput_mbps,
        started_at,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::UdpUpload,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: datagrams_sent > 0,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: Some(result),
        page_load: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn connect_udp(cfg: &UdpThroughputConfig) -> Result<(UdpSocket, SocketAddr), String> {
    let target = format!("{}:{}", cfg.target_host, cfg.target_port);
    let target_addr: SocketAddr = match target.parse() {
        Ok(a) => a,
        Err(_) => match tokio::net::lookup_host(&target).await {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => return Err(format!("No address resolved for {target}")),
            },
            Err(e) => return Err(format!("DNS error for {target}: {e}")),
        },
    };

    let bind_addr: SocketAddr = if target_addr.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };

    let sock = UdpSocket::bind(bind_addr)
        .await
        .map_err(|e| format!("UDP bind failed: {e}"))?;
    sock.connect(target_addr)
        .await
        .map_err(|e| format!("UDP connect failed: {e}"))?;

    Ok((sock, target_addr))
}

/// Wait for CMD_ACK from the server; return Err with a message on timeout/error.
async fn wait_for_ack(sock: &UdpSocket, timeout_ms: u64) -> Result<(), String> {
    let mut buf = vec![0u8; CTRL_LEN * 2];
    match tokio::time::timeout(Duration::from_millis(timeout_ms), sock.recv(&mut buf)).await {
        Ok(Ok(n)) if n == CTRL_LEN => {
            if buf[..4] == *MAGIC && buf[4] == CMD_ACK {
                Ok(())
            } else {
                Err(format!("Unexpected response to CMD: {:?}", &buf[..n]))
            }
        }
        Ok(Ok(n)) => Err(format!("Short response: {n} bytes")),
        Ok(Err(e)) => Err(format!("recv error: {e}")),
        Err(_) => Err("Timed out waiting for CMD_ACK".into()),
    }
}

/// Receive data packets and CMD_DONE for a download probe.
///
/// Returns `(received_seqs, received_bytes, transfer_ms)`.
async fn recv_download(
    sock: &UdpSocket,
    expected_seqs: u32,
    timeout_ms: u64,
) -> (HashSet<u32>, usize, f64) {
    let mut received_seqs: HashSet<u32> = HashSet::new();
    let mut received_bytes: usize = 0;
    let mut t_first: Option<Instant> = None;
    let mut t_last = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf = vec![0u8; CTRL_LEN + CHUNK_SIZE + 64];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n == CTRL_LEN && buf[..4] == *MAGIC && buf[4] == CMD_DONE => {
                // Server signals end of stream.
                debug!("UDP download: CMD_DONE received");
                t_last = Instant::now();
                break;
            }
            Ok(Ok(n)) if n > DATA_HDR_LEN => {
                let seq = u32::from_le_bytes(buf[..4].try_into().unwrap_or([0; 4]));
                let data_len = n - DATA_HDR_LEN;
                if received_seqs.insert(seq) {
                    received_bytes += data_len;
                }
                let now = Instant::now();
                if t_first.is_none() {
                    t_first = Some(now);
                }
                t_last = now;
                debug!("UDP download: seq {seq}/{expected_seqs} ({data_len}B)");
            }
            Ok(Ok(_)) => {} // ignore short or unrecognised packets
            Ok(Err(e)) => {
                debug!("UDP download recv error: {e}");
                break;
            }
            Err(_) => {
                debug!(
                    "UDP download: timeout after {} datagrams",
                    received_seqs.len()
                );
                break;
            }
        }
    }

    let transfer_ms = match t_first {
        Some(t0) => (t_last - t0).as_secs_f64() * 1000.0,
        None => 0.0,
    };

    (received_seqs, received_bytes, transfer_ms)
}

/// Send upload data packets + CMD_DONE; wait for CMD_REPORT.
///
/// Returns `(datagrams_sent, transfer_ms, bytes_acked)`.
async fn send_upload(
    sock: &UdpSocket,
    payload_bytes: usize,
    timeout_ms: u64,
) -> (u32, f64, Option<usize>) {
    if payload_bytes == 0 {
        let done = make_ctrl(CMD_DONE, 0);
        let _ = sock.send(&done).await;
        return (0, 0.0, Some(0));
    }

    let total_seqs = payload_bytes.div_ceil(CHUNK_SIZE) as u32;
    let mut sent = 0u32;
    let mut sent_bytes = 0usize;

    let t0 = Instant::now();

    for seq in 0..total_seqs {
        let payload_size = (payload_bytes - sent_bytes).min(CHUNK_SIZE);
        let mut pkt = vec![0u8; DATA_HDR_LEN + payload_size];
        pkt[..4].copy_from_slice(&seq.to_le_bytes());
        pkt[4..8].copy_from_slice(&total_seqs.to_le_bytes());
        if sock.send(&pkt).await.is_err() {
            break;
        }
        sent += 1;
        sent_bytes += payload_size;
    }

    // Signal upload complete.
    let done = make_ctrl(CMD_DONE, payload_bytes as u32);
    let _ = sock.send(&done).await;

    // Wait for CMD_REPORT from server.
    let bytes_acked = wait_for_report(sock, timeout_ms).await;
    let transfer_ms = t0.elapsed().as_secs_f64() * 1000.0;

    (sent, transfer_ms, bytes_acked)
}

/// Wait for CMD_REPORT; return the server's acknowledged byte count (or None on timeout).
async fn wait_for_report(sock: &UdpSocket, timeout_ms: u64) -> Option<usize> {
    let mut buf = vec![0u8; CTRL_LEN * 2];
    match tokio::time::timeout(Duration::from_millis(timeout_ms), sock.recv(&mut buf)).await {
        Ok(Ok(n)) if n == CTRL_LEN && buf[..4] == *MAGIC && buf[4] == CMD_REPORT => {
            let value = u32::from_le_bytes(buf[8..12].try_into().unwrap_or([0; 4])) as usize;
            debug!("UDP upload: CMD_REPORT bytes_received={value}");
            Some(value)
        }
        _ => {
            debug!("UDP upload: no CMD_REPORT received");
            None
        }
    }
}

fn make_ctrl(cmd: u8, value: u32) -> Vec<u8> {
    let mut pkt = vec![0u8; CTRL_LEN];
    pkt[..4].copy_from_slice(MAGIC);
    pkt[4] = cmd;
    pkt[8..12].copy_from_slice(&value.to_le_bytes());
    pkt
}

fn loss_percent(sent: u32, received: u32) -> f64 {
    if sent == 0 {
        return 0.0;
    }
    let lost = sent.saturating_sub(received);
    lost as f64 / sent as f64 * 100.0
}

fn mbps(payload_bytes: usize, transfer_ms: f64) -> Option<f64> {
    if transfer_ms > 0.0 {
        Some(payload_bytes as f64 / transfer_ms * 1000.0 / (1024.0 * 1024.0))
    } else {
        None
    }
}

fn udp_tp_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    protocol: Protocol,
    started_at: chrono::DateTime<Utc>,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol,
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
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mbps_one_mib_in_one_second() {
        let result = mbps(1024 * 1024, 1000.0).expect("should produce a value");
        assert!((result - 1.0).abs() < 1e-9, "expected 1.0, got {result}");
    }

    #[test]
    fn mbps_zero_transfer_ms_returns_none() {
        assert!(mbps(1024, 0.0).is_none());
    }

    #[test]
    fn loss_percent_no_loss() {
        assert_eq!(loss_percent(10, 10), 0.0);
    }

    #[test]
    fn loss_percent_all_lost() {
        assert_eq!(loss_percent(10, 0), 100.0);
    }

    #[test]
    fn loss_percent_partial() {
        let l = loss_percent(10, 7);
        assert!((l - 30.0).abs() < 1e-9, "expected 30.0, got {l}");
    }

    #[test]
    fn loss_percent_zero_sent() {
        assert_eq!(loss_percent(0, 0), 0.0);
    }

    /// End-to-end download test: spin up a loopback stub server and download 4 KiB.
    #[tokio::test]
    async fn udpdownload_loopback() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();

        let server = std::sync::Arc::new(server_sock);
        let server_clone = server.clone();
        tokio::spawn(async move {
            stub_server(server_clone).await;
        });

        let cfg = UdpThroughputConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: port,
            timeout_ms: 5000,
        };

        let attempt = run_udpdownload_probe(Uuid::new_v4(), 0, 4096, &cfg).await;
        assert!(attempt.success, "probe failed: {:?}", attempt.error);
        let ut = attempt.udp_throughput.unwrap();
        assert_eq!(ut.payload_bytes, 4096);
        assert!(ut.datagrams_received > 0);
        assert!(ut.throughput_mbps.is_some());
    }

    /// Stub server implementing the minimal download + upload protocol.
    async fn stub_server(sock: std::sync::Arc<UdpSocket>) {
        let mut buf = vec![0u8; 65536];
        use std::collections::{HashMap, HashSet};

        struct UpState {
            received_seqs: HashSet<u32>,
            received_bytes: usize,
        }
        let mut upload_states: HashMap<std::net::SocketAddr, UpState> = HashMap::new();

        for _ in 0..500 {
            let (n, src) = match sock.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(_) => break,
            };
            let pkt = &buf[..n];
            if n == CTRL_LEN && pkt[..4] == *MAGIC {
                let cmd = pkt[4];
                let value = u32::from_le_bytes(pkt[8..12].try_into().unwrap_or([0; 4])) as usize;
                match cmd {
                    CMD_DOWNLOAD => {
                        let ack = make_ctrl(CMD_ACK, 0);
                        let _ = sock.send_to(&ack, src).await;
                        // Send all data packets inline (small enough for test)
                        let total_seqs = value.div_ceil(CHUNK_SIZE) as u32;
                        let mut sent_bytes = 0usize;
                        for seq in 0..total_seqs {
                            let ps = (value - sent_bytes).min(CHUNK_SIZE);
                            let mut dp = vec![0u8; DATA_HDR_LEN + ps];
                            dp[..4].copy_from_slice(&seq.to_le_bytes());
                            dp[4..8].copy_from_slice(&total_seqs.to_le_bytes());
                            let _ = sock.send_to(&dp, src).await;
                            sent_bytes += ps;
                        }
                        let done = make_ctrl(CMD_DONE, value as u32);
                        let _ = sock.send_to(&done, src).await;
                    }
                    CMD_UPLOAD => {
                        upload_states.insert(
                            src,
                            UpState {
                                received_seqs: HashSet::new(),
                                received_bytes: 0,
                            },
                        );
                        let ack = make_ctrl(CMD_ACK, 0);
                        let _ = sock.send_to(&ack, src).await;
                    }
                    CMD_DONE => {
                        if let Some(state) = upload_states.remove(&src) {
                            let report = make_ctrl(CMD_REPORT, state.received_bytes as u32);
                            let _ = sock.send_to(&report, src).await;
                        }
                    }
                    _ => {}
                }
            } else if n > DATA_HDR_LEN {
                if let Some(state) = upload_states.get_mut(&src) {
                    let seq = u32::from_le_bytes(pkt[..4].try_into().unwrap_or([0; 4]));
                    let data_len = n - DATA_HDR_LEN;
                    if state.received_seqs.insert(seq) {
                        state.received_bytes += data_len;
                    }
                }
            }
        }
    }
}
