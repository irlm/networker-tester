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
use tracing::{debug, info};

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
pub async fn run_udp_throughput(socket: UdpSocket) {
    let sock = Arc::new(socket);
    info!("UDP throughput → {:?}", sock.local_addr().ok());

    let mut buf = vec![0u8; 65536];
    // Per-client upload state: tracks seq_nums and byte counts until CMD_DONE.
    let mut upload_states: HashMap<SocketAddr, UploadState> = HashMap::new();
    let mut pkt_counter: u64 = 0;

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
                            created_at: std::time::Instant::now(),
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

        // Periodically reap stale upload states to prevent memory leaks
        // from clients that disconnect without sending CMD_DONE.
        pkt_counter += 1;
        if pkt_counter.is_multiple_of(100) {
            reap_stale_uploads(&mut upload_states, std::time::Instant::now());
        }
    }
}

/// TTL for upload states — reap entries older than this to prevent leaks.
const UPLOAD_STATE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Drop upload states older than [`UPLOAD_STATE_TTL`]. Takes `now` explicitly
/// so the TTL comparison is unit-testable with backdated instants — these
/// were the file's 4 remaining documented mutation survivors (the 5th, the
/// once-per-100-packets cadence, stays: reaping fresh state is a no-op, so
/// cadence changes are unobservable without a leak-sized test).
fn reap_stale_uploads(
    upload_states: &mut HashMap<SocketAddr, UploadState>,
    now: std::time::Instant,
) {
    upload_states.retain(|addr, state| {
        let alive = now.duration_since(state.created_at) < UPLOAD_STATE_TTL;
        if !alive {
            debug!("Reaping stale upload state for {addr}");
        }
        alive
    });
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
    created_at: std::time::Instant,
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests — real sockets over loopback, driving the wire protocol.
//
// Until 2026-08-07 this file had NO test module: its 26 surviving mutants were
// the largest gap the mutation pilot found (the file's only guards were the
// tester's integration probes, which exercise the happy path end-to-end but
// pin none of the protocol arithmetic). Every test is deadline-bounded — a
// test that hangs on regression is barely better than one that passes on it
// (bind_failure.rs lesson).
//
// The stale-state reaper's TTL comparison is extracted into
// reap_stale_uploads(now) precisely so it is testable with backdated instants
// (std::time::Instant ignores tokio's paused clock). Two deliberate survivors
// remain: the once-per-100-packets cadence (reaping fresh state is a no-op,
// so cadence changes are unobservable without a leak-sized test) and the
// reaper's debug! guard (log-only — no behavioral surface to assert).
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DEADLINE: Duration = Duration::from_secs(5);

    /// Bind the server on an ephemeral loopback port, spawn it, and return a
    /// client socket already `connect`ed to it.
    async fn start() -> UdpSocket {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_sock.local_addr().unwrap();
        tokio::spawn(run_udp_throughput(server_sock));

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.connect(server_addr).await.unwrap();
        client
    }

    async fn recv(client: &UdpSocket, buf: &mut [u8]) -> usize {
        tokio::time::timeout(DEADLINE, client.recv(buf))
            .await
            .expect("timed out waiting for a server packet")
            .expect("recv failed")
    }

    fn is_ctrl(pkt: &[u8], cmd: u8) -> bool {
        pkt.len() == CTRL_LEN && pkt[..4] == *MAGIC && pkt[4] == cmd
    }

    fn ctrl_value(pkt: &[u8]) -> u32 {
        u32::from_le_bytes(pkt[8..12].try_into().unwrap())
    }

    /// Build a client→server data packet: seq/total header + `payload` zeros.
    fn data_pkt(seq: u32, total: u32, payload: usize) -> Vec<u8> {
        let mut pkt = vec![0u8; DATA_HDR_LEN + payload];
        pkt[..4].copy_from_slice(&seq.to_le_bytes());
        pkt[4..8].copy_from_slice(&total.to_le_bytes());
        pkt
    }

    // ── reap_stale_uploads ───────────────────────────────────────────────────

    #[test]
    fn reap_drops_only_states_older_than_the_ttl() {
        let now = std::time::Instant::now();
        let stale_created = now
            .checked_sub(UPLOAD_STATE_TTL + Duration::from_secs(1))
            .expect("test clock underflow");
        let fresh_created = now - Duration::from_secs(1);

        let stale_addr: SocketAddr = "127.0.0.1:2001".parse().unwrap();
        let fresh_addr: SocketAddr = "127.0.0.1:2002".parse().unwrap();
        let mut states = HashMap::new();
        for (addr, created_at) in [(stale_addr, stale_created), (fresh_addr, fresh_created)] {
            states.insert(
                addr,
                UploadState {
                    expected_bytes: 1,
                    received_seqs: HashSet::new(),
                    received_bytes: 0,
                    created_at,
                },
            );
        }

        reap_stale_uploads(&mut states, now);

        assert!(
            !states.contains_key(&stale_addr),
            "state older than the TTL must be reaped"
        );
        assert!(
            states.contains_key(&fresh_addr),
            "fresh state must survive — reaping it would zero an in-flight upload's count"
        );
    }

    #[test]
    fn reap_boundary_is_exact() {
        // The liveness comparison is strict: a state aged EXACTLY the TTL is
        // dead (`<` not `<=`).
        let now = std::time::Instant::now();
        let exactly_ttl_old = now
            .checked_sub(UPLOAD_STATE_TTL)
            .expect("test clock underflow");

        let addr: SocketAddr = "127.0.0.1:2003".parse().unwrap();
        let mut states = HashMap::new();
        states.insert(
            addr,
            UploadState {
                expected_bytes: 1,
                received_seqs: HashSet::new(),
                received_bytes: 0,
                created_at: exactly_ttl_old,
            },
        );

        reap_stale_uploads(&mut states, now);
        assert!(
            !states.contains_key(&addr),
            "age == TTL must be reaped (strict <)"
        );
    }

    // ── make_ctrl ────────────────────────────────────────────────────────────

    #[test]
    fn make_ctrl_layout_is_the_documented_wire_format() {
        let pkt = make_ctrl(CMD_REPORT, 0x0102_0304);
        assert_eq!(pkt.len(), CTRL_LEN, "control packets are exactly 12 bytes");
        assert_eq!(&pkt[..4], MAGIC);
        assert_eq!(pkt[4], CMD_REPORT);
        assert_eq!(&pkt[5..8], &[0, 0, 0], "padding must stay zeroed");
        assert_eq!(&pkt[8..12], &0x0102_0304u32.to_le_bytes());
    }

    // ── download ─────────────────────────────────────────────────────────────

    /// CMD_DOWNLOAD must be ACKed, then deliver EXACTLY the requested bytes in
    /// correctly-headered chunks, then CMD_DONE carrying the byte total.
    /// 3000 bytes = 1400 + 1400 + 200 — a non-chunk-aligned size, so the
    /// last-packet arithmetic (payload_size, sent_bytes accumulation) is load-
    /// bearing here.
    #[tokio::test]
    async fn download_delivers_exactly_the_requested_bytes() {
        let client = start().await;
        client.send(&make_ctrl(CMD_DOWNLOAD, 3000)).await.unwrap();

        let mut buf = vec![0u8; 65536];
        let n = recv(&client, &mut buf).await;
        assert!(
            is_ctrl(&buf[..n], CMD_ACK),
            "first reply must be CMD_ACK, got {:02x?}",
            &buf[..n.min(16)]
        );

        let mut payload_total = 0usize;
        let mut seqs = Vec::new();
        loop {
            let n = recv(&client, &mut buf).await;
            let pkt = &buf[..n];
            if is_ctrl(pkt, CMD_DONE) {
                assert_eq!(ctrl_value(pkt), 3000, "CMD_DONE must report the byte total");
                break;
            }
            assert!(n > DATA_HDR_LEN, "data packet with no payload");
            let seq = u32::from_le_bytes(pkt[..4].try_into().unwrap());
            let total = u32::from_le_bytes(pkt[4..8].try_into().unwrap());
            assert_eq!(total, 3, "3000 bytes at 1400/chunk is 3 packets (div_ceil)");
            assert!(
                n - DATA_HDR_LEN <= CHUNK_SIZE,
                "payload above CHUNK_SIZE breaks the sub-MTU guarantee"
            );
            seqs.push(seq);
            payload_total += n - DATA_HDR_LEN;
        }

        assert_eq!(
            payload_total, 3000,
            "download must deliver exactly the requested bytes"
        );
        assert_eq!(seqs, vec![0, 1, 2], "seq numbers are 0-based and ordered");
    }

    /// A zero-byte download is legal: no data packets, immediate CMD_DONE(0).
    /// (Guards the `total_bytes == 0` early-return — inverted, it would blast
    /// data for 0-byte requests and dead-silence real ones.)
    #[tokio::test]
    async fn download_of_zero_bytes_is_an_immediate_done() {
        let client = start().await;
        client.send(&make_ctrl(CMD_DOWNLOAD, 0)).await.unwrap();

        let mut buf = vec![0u8; 65536];
        let n = recv(&client, &mut buf).await;
        assert!(is_ctrl(&buf[..n], CMD_ACK), "download must still be ACKed");

        let n = recv(&client, &mut buf).await;
        assert!(
            is_ctrl(&buf[..n], CMD_DONE),
            "zero-byte download must go straight to CMD_DONE, got {:02x?}",
            &buf[..n.min(16)]
        );
        assert_eq!(ctrl_value(&buf[..n]), 0);
    }

    // ── upload ───────────────────────────────────────────────────────────────

    /// Full upload round-trip with the two counting hazards the wire format
    /// allows: a DUPLICATE seq (must count once) and a header-only 8-byte
    /// packet (must not be treated as data — it would poison the dedup set
    /// and eat the real packet's bytes).
    #[tokio::test]
    async fn upload_report_counts_each_seq_once_and_ignores_header_only_packets() {
        let client = start().await;
        client.send(&make_ctrl(CMD_UPLOAD, 3000)).await.unwrap();

        let mut buf = vec![0u8; 65536];
        let n = recv(&client, &mut buf).await;
        assert!(is_ctrl(&buf[..n], CMD_ACK), "CMD_UPLOAD must be ACKed");

        // A header-only packet for seq 2: exactly DATA_HDR_LEN bytes, no
        // payload. The server must ignore it entirely (`n > DATA_HDR_LEN`);
        // if it were processed, seq 2 would enter the dedup set with 0 bytes
        // and the real seq-2 packet below would be discarded as a duplicate.
        client.send(&data_pkt(2, 3, 0)).await.unwrap();

        client.send(&data_pkt(0, 3, 1400)).await.unwrap();
        client.send(&data_pkt(1, 3, 1400)).await.unwrap();
        client.send(&data_pkt(0, 3, 1400)).await.unwrap(); // duplicate seq 0
        client.send(&data_pkt(2, 3, 200)).await.unwrap();

        client.send(&make_ctrl(CMD_DONE, 0)).await.unwrap();
        let n = recv(&client, &mut buf).await;
        assert!(
            is_ctrl(&buf[..n], CMD_REPORT),
            "CMD_DONE after an upload must yield CMD_REPORT, got {:02x?}",
            &buf[..n.min(16)]
        );
        assert_eq!(
            ctrl_value(&buf[..n]),
            3000,
            "report must count 1400+1400+200 exactly once each \
             (duplicate seq recounted, or header-only packet processed, or \
             payload length miscomputed)"
        );
    }

    /// A 12-byte packet WITHOUT the magic must be handled as data, not
    /// control. (Guards the `n == CTRL_LEN && magic` conjunction: with `||`,
    /// any 12-byte data packet becomes an unknown-command control packet and
    /// its bytes vanish from the upload count.)
    #[tokio::test]
    async fn twelve_byte_packet_without_magic_is_data_not_control() {
        let client = start().await;
        client.send(&make_ctrl(CMD_UPLOAD, 4)).await.unwrap();

        let mut buf = vec![0u8; 65536];
        let n = recv(&client, &mut buf).await;
        assert!(is_ctrl(&buf[..n], CMD_ACK));

        // 12 bytes total = 8-byte header (seq 0) + 4 payload bytes. Same size
        // as a control packet, but no magic.
        client.send(&data_pkt(0, 1, 4)).await.unwrap();

        client.send(&make_ctrl(CMD_DONE, 0)).await.unwrap();
        let n = recv(&client, &mut buf).await;
        assert!(is_ctrl(&buf[..n], CMD_REPORT));
        assert_eq!(
            ctrl_value(&buf[..n]),
            4,
            "a magic-less 12-byte packet is a 4-byte-payload data packet"
        );
    }
}
