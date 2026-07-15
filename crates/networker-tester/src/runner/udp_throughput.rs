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
/// 6. Client computes throughput over the send window (first data packet to
///    CMD_DONE sent) using the server-acknowledged byte count from CMD_REPORT
///    as the numerator. The CMD_REPORT round-trip is deliberately excluded
///    from the transfer window, and loss is derived from the server's byte
///    count — never assumed. (Trust audit V3/V4.)
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
        datagrams_received: Some(datagrams_received),
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
        browser: None,
        http_stack: None,
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

    // Send data packets and CMD_DONE; the transfer window ends when CMD_DONE
    // is sent — the CMD_REPORT wait must not inflate the denominator (V4).
    let (datagrams_sent, transfer_ms, bytes_acked) =
        send_upload(&sock, payload_bytes, cfg.timeout_ms).await;

    // The server's CMD_REPORT byte count is the only honest source of truth
    // for upload loss — the client cannot observe what arrived. If the report
    // never came back the outcome is unknown: report an error rather than
    // fabricating a 0% loss figure (V3).
    let acked = match bytes_acked {
        Some(a) => a,
        None => {
            return udp_tp_failed(
                run_id,
                attempt_id,
                sequence_num,
                Protocol::UdpUpload,
                started_at,
                format!(
                    "No CMD_REPORT from server within {}ms — bytes received by \
                     the server are unknown, loss cannot be determined",
                    cfg.timeout_ms
                ),
            );
        }
    };

    let loss = if payload_bytes > 0 {
        payload_bytes.saturating_sub(acked) as f64 / payload_bytes as f64 * 100.0
    } else {
        0.0
    };
    // Throughput numerator = bytes the server actually received.
    let throughput_mbps = mbps(acked, transfer_ms);

    let result = UdpThroughputResult {
        remote_addr: remote_addr.to_string(),
        payload_bytes,
        datagrams_sent,
        // Unknown for uploads: CMD_REPORT carries bytes, not datagram counts.
        datagrams_received: None,
        bytes_acked: Some(acked),
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
        success: payload_bytes == 0 || acked > 0,
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
        browser: None,
        http_stack: None,
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
            Ok(Ok(_)) => {} // ignore short or unrecognized packets
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

    // The transfer window ends here: everything after CMD_DONE is a control
    // round-trip, not data transfer. A lost/late CMD_REPORT previously turned
    // a 50 ms transfer into timeout_ms + 50 ms (trust audit V4).
    let transfer_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Wait for CMD_REPORT from server (used only for the acked byte count).
    let bytes_acked = wait_for_report(sock, timeout_ms).await;

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
        browser: None,
        http_stack: None,
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

    // ── make_ctrl ─────────────────────────────────────────────────────────────

    #[test]
    fn make_ctrl_has_correct_magic() {
        let pkt = make_ctrl(CMD_DOWNLOAD, 0);
        assert_eq!(&pkt[..4], MAGIC);
    }

    #[test]
    fn make_ctrl_sets_command_byte() {
        assert_eq!(make_ctrl(CMD_DOWNLOAD, 0)[4], CMD_DOWNLOAD);
        assert_eq!(make_ctrl(CMD_UPLOAD, 0)[4], CMD_UPLOAD);
        assert_eq!(make_ctrl(CMD_ACK, 0)[4], CMD_ACK);
    }

    #[test]
    fn make_ctrl_encodes_value_little_endian() {
        let pkt = make_ctrl(CMD_DOWNLOAD, 0x0102_0304);
        assert_eq!(&pkt[8..12], &[0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn make_ctrl_zero_value() {
        let pkt = make_ctrl(CMD_DONE, 0);
        assert_eq!(&pkt[8..12], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn make_ctrl_length_is_ctrl_len() {
        assert_eq!(make_ctrl(CMD_DOWNLOAD, 1234).len(), CTRL_LEN);
    }

    // ── udp_tp_failed ─────────────────────────────────────────────────────────

    #[test]
    fn udp_tp_failed_sets_correct_fields() {
        let run_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let started_at = chrono::Utc::now();
        let attempt = udp_tp_failed(
            run_id,
            attempt_id,
            7,
            Protocol::UdpDownload,
            started_at,
            "test error".into(),
        );
        assert!(!attempt.success);
        assert_eq!(attempt.protocol, Protocol::UdpDownload);
        assert_eq!(attempt.sequence_num, 7);
        assert_eq!(attempt.run_id, run_id);
        assert_eq!(attempt.attempt_id, attempt_id);
        let err = attempt.error.expect("error should be set");
        assert_eq!(err.message, "test error");
        assert_eq!(err.category, ErrorCategory::Udp);
        assert!(attempt.udp_throughput.is_none());
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
        assert!(ut.datagrams_received.unwrap_or(0) > 0);
        assert!(ut.throughput_mbps.is_some());
    }

    /// Regression test for trust-audit V3: upload loss must come from the
    /// server's CMD_REPORT, not be fabricated as `received = sent`. The stub
    /// under-reports by half — the probe must report ~50% loss and use the
    /// acked bytes as throughput numerator.
    #[tokio::test]
    async fn udpupload_loss_derived_from_cmd_report() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();
        let server = std::sync::Arc::new(server_sock);
        tokio::spawn(lossy_upload_stub(server, 0.5, 0));

        let cfg = UdpThroughputConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: port,
            timeout_ms: 5000,
        };

        let payload = 14_000usize; // 10 datagrams
        let attempt = run_udpupload_probe(Uuid::new_v4(), 0, payload, &cfg).await;
        assert!(attempt.success, "probe failed: {:?}", attempt.error);
        let ut = attempt.udp_throughput.unwrap();
        assert_eq!(ut.bytes_acked, Some(payload / 2));
        assert!(
            (ut.loss_percent - 50.0).abs() < 1e-9,
            "loss must be derived from CMD_REPORT (expected 50%), got {}%",
            ut.loss_percent
        );
        assert_eq!(
            ut.datagrams_received, None,
            "upload datagram count is unknowable and must not be fabricated"
        );
    }

    /// Regression test for trust-audit V4: the transfer window ends at
    /// CMD_DONE. A CMD_REPORT delayed by 400 ms must not inflate transfer_ms
    /// (a loopback 14 KB send takes single-digit milliseconds).
    #[tokio::test]
    async fn udpupload_transfer_window_excludes_report_wait() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();
        let server = std::sync::Arc::new(server_sock);
        tokio::spawn(lossy_upload_stub(server, 1.0, 400));

        let cfg = UdpThroughputConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: port,
            timeout_ms: 5000,
        };

        let attempt = run_udpupload_probe(Uuid::new_v4(), 0, 14_000, &cfg).await;
        assert!(attempt.success, "probe failed: {:?}", attempt.error);
        let ut = attempt.udp_throughput.unwrap();
        assert!(
            ut.transfer_ms < 300.0,
            "transfer window must exclude the 400ms-delayed CMD_REPORT wait, got {}ms",
            ut.transfer_ms
        );
    }

    /// Regression test for trust-audit V3: when CMD_REPORT never arrives the
    /// client cannot know the loss — the attempt must fail with an explicit
    /// error instead of reporting fabricated numbers.
    #[tokio::test]
    async fn udpupload_missing_report_is_an_error_not_zero_loss() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();
        let server = std::sync::Arc::new(server_sock);
        // ack_fraction < 0 → stub never sends CMD_REPORT
        tokio::spawn(lossy_upload_stub(server, -1.0, 0));

        let cfg = UdpThroughputConfig {
            target_host: "127.0.0.1".to_string(),
            target_port: port,
            timeout_ms: 500, // short: this test waits out the report timeout
        };

        let attempt = run_udpupload_probe(Uuid::new_v4(), 0, 14_000, &cfg).await;
        assert!(!attempt.success, "missing CMD_REPORT must fail the attempt");
        assert!(attempt.udp_throughput.is_none());
        let err = attempt.error.expect("error must be set");
        assert!(
            err.message.contains("CMD_REPORT"),
            "error should explain the missing report, got: {}",
            err.message
        );
    }

    /// Upload stub: ACKs, absorbs data packets, then on CMD_DONE reports
    /// `payload * ack_fraction` bytes after `report_delay_ms` (no report at
    /// all when `ack_fraction < 0`).
    async fn lossy_upload_stub(
        sock: std::sync::Arc<UdpSocket>,
        ack_fraction: f64,
        report_delay_ms: u64,
    ) {
        let mut buf = vec![0u8; 65536];
        let mut received_bytes = 0usize;
        for _ in 0..500 {
            let (n, src) = match sock.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(_) => break,
            };
            let pkt = &buf[..n];
            if n == CTRL_LEN && pkt[..4] == *MAGIC {
                match pkt[4] {
                    CMD_UPLOAD => {
                        received_bytes = 0;
                        let _ = sock.send_to(&make_ctrl(CMD_ACK, 0), src).await;
                    }
                    CMD_DONE => {
                        if ack_fraction < 0.0 {
                            continue; // simulate a lost CMD_REPORT
                        }
                        if report_delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(report_delay_ms)).await;
                        }
                        let acked = (received_bytes as f64 * ack_fraction) as u32;
                        let _ = sock.send_to(&make_ctrl(CMD_REPORT, acked), src).await;
                    }
                    _ => {}
                }
            } else if n > DATA_HDR_LEN {
                received_bytes += n - DATA_HDR_LEN;
            }
        }
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
