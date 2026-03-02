/// UDP bulk throughput server handler.
///
/// Wire protocol – all multi-byte integers are little-endian.
///
/// ## Control packet (12 bytes)
/// ```text
/// [0..4]  magic = b"NWKT"
/// [4]     cmd byte
/// [5..8]  padding (zeros)
/// [8..12] value as u32 LE
/// ```
///
/// | Cmd  | Name         | Direction       | Value              |
/// |------|--------------|-----------------|---------------------|
/// | 0x01 | CMD_DOWNLOAD | client → server | requested bytes    |
/// | 0x02 | CMD_UPLOAD   | client → server | total bytes to upload |
/// | 0x04 | CMD_DONE     | client → server | (upload complete)  |
/// | 0x10 | CMD_ACK      | server → client | 0 (ready)          |
/// | 0x11 | CMD_REPORT   | server → client | bytes received     |
///
/// ## Data packet (header + payload)
/// ```text
/// [0..4]  seq_num as u32 LE (0-based)
/// [4..8]  total_seqs as u32 LE
/// [8..]   payload (up to CHUNK_SIZE bytes)
/// ```
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

const MAGIC: &[u8; 4] = b"NWKT";
const CMD_DOWNLOAD: u8 = 0x01;
const CMD_UPLOAD: u8 = 0x02;
const CMD_DONE: u8 = 0x04;
const CMD_ACK: u8 = 0x10;
const CMD_REPORT: u8 = 0x11;
const CTRL_LEN: usize = 12;
const DATA_HDR_LEN: usize = 8;
/// Maximum payload bytes per datagram — stays well below typical 1500-byte MTU.
const CHUNK_SIZE: usize = 1400;

/// Runs the UDP throughput server on `port`.
///
/// Separate from the UDP echo server so the two protocols never interfere.
pub async fn run_udp_throughput(port: u16) {
    let addr = format!("0.0.0.0:{port}");
    let sock = match UdpSocket::bind(&addr).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            warn!("Failed to bind UDP throughput socket on {addr}: {e}");
            return;
        }
    };
    info!("UDP throughput → 0.0.0.0:{port}");

    let mut buf = vec![0u8; 65536];
    // Per-client upload state: tracks seq_nums and byte counts until CMD_DONE.
    let mut upload_states: HashMap<SocketAddr, UploadState> = HashMap::new();

    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                debug!("UDP throughput recv_from error: {e}");
                continue;
            }
        };

        let pkt = &buf[..n];

        if n == CTRL_LEN && pkt[..4] == *MAGIC {
            // Control packet
            let cmd = pkt[4];
            let value = u32::from_le_bytes(pkt[8..12].try_into().unwrap_or([0; 4])) as usize;

            match cmd {
                CMD_DOWNLOAD => {
                    debug!("UDP throughput: CMD_DOWNLOAD {value} bytes from {src}");
                    let ack = make_ctrl(CMD_ACK, 0);
                    let _ = sock.send_to(&ack, src).await;
                    // Spawn a task to blast data packets to the client.
                    let sock_clone = sock.clone();
                    tokio::spawn(async move {
                        send_download(sock_clone, src, value).await;
                    });
                }
                CMD_UPLOAD => {
                    debug!("UDP throughput: CMD_UPLOAD {value} bytes expected from {src}");
                    upload_states.insert(
                        src,
                        UploadState {
                            expected_bytes: value,
                            received_seqs: HashSet::new(),
                            received_bytes: 0,
                        },
                    );
                    let ack = make_ctrl(CMD_ACK, 0);
                    let _ = sock.send_to(&ack, src).await;
                }
                CMD_DONE => {
                    if let Some(state) = upload_states.remove(&src) {
                        debug!(
                            "UDP throughput: CMD_DONE from {src}; \
                             received {}/{} bytes ({} datagrams)",
                            state.received_bytes,
                            state.expected_bytes,
                            state.received_seqs.len()
                        );
                        let report = make_ctrl(CMD_REPORT, state.received_bytes as u32);
                        let _ = sock.send_to(&report, src).await;
                    } else {
                        debug!("UDP throughput: CMD_DONE from {src} without prior CMD_UPLOAD");
                    }
                }
                other => {
                    debug!("UDP throughput: unknown cmd {other:#x} from {src}");
                }
            }
        } else if n > DATA_HDR_LEN {
            // Data packet (upload from client).
            if let Some(state) = upload_states.get_mut(&src) {
                let seq = u32::from_le_bytes(pkt[..4].try_into().unwrap_or([0; 4]));
                let data_len = n - DATA_HDR_LEN;
                // Only count each seq_num once (deduplication).
                if state.received_seqs.insert(seq) {
                    state.received_bytes += data_len;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Download helper
// ─────────────────────────────────────────────────────────────────────────────

/// Send `total_bytes` worth of zero-filled data packets to `dst`, then CMD_DONE.
async fn send_download(sock: Arc<UdpSocket>, dst: SocketAddr, total_bytes: usize) {
    if total_bytes == 0 {
        let done = make_ctrl(CMD_DONE, 0);
        let _ = sock.send_to(&done, dst).await;
        return;
    }

    let total_seqs = total_bytes.div_ceil(CHUNK_SIZE) as u32;
    let mut sent_bytes = 0usize;

    for seq in 0..total_seqs {
        let payload_size = (total_bytes - sent_bytes).min(CHUNK_SIZE);
        let mut pkt = vec![0u8; DATA_HDR_LEN + payload_size];
        pkt[..4].copy_from_slice(&seq.to_le_bytes());
        pkt[4..8].copy_from_slice(&total_seqs.to_le_bytes());
        // payload remains zeros
        if sock.send_to(&pkt, dst).await.is_err() {
            break;
        }
        sent_bytes += payload_size;
    }

    // Signal end of download stream.
    let done = make_ctrl(CMD_DONE, total_bytes as u32);
    let _ = sock.send_to(&done, dst).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn make_ctrl(cmd: u8, value: u32) -> Vec<u8> {
    let mut pkt = vec![0u8; CTRL_LEN];
    pkt[..4].copy_from_slice(MAGIC);
    pkt[4] = cmd;
    // pkt[5..8] = zeros (padding, already initialized)
    pkt[8..12].copy_from_slice(&value.to_le_bytes());
    pkt
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-client state
// ─────────────────────────────────────────────────────────────────────────────

struct UploadState {
    expected_bytes: usize,
    received_seqs: HashSet<u32>,
    received_bytes: usize,
}
