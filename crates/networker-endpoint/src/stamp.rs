//! STAMP Session-Reflector (RFC 8762, unauthenticated mode) — UDP port 9997.
//!
//! Reflects each 44-byte Session-Sender test packet per RFC 8762 §4.3:
//! copies the sender's sequence number, timestamp, and error estimate into
//! the reflected packet, stamps the receive timestamp (T2) and its own
//! transmit timestamp (T3), and inserts a reflector sequence number.
//!
//! # Sequence-number mode
//!
//! The reflector runs in the RFC's **stateful** sequence mode (RFC 8762
//! §4.2): it keeps one monotonically increasing sequence counter per sender
//! (source address+port), incremented for every test packet received. This is
//! what lets the Session-Sender split loss by direction — a gap between the
//! highest reflector sequence and its own send count is forward
//! (sender→reflector) loss, while gaps among received reflector sequences are
//! reverse (reflector→sender) loss. Sessions expire after 60 s idle so the
//! map cannot grow unbounded.
//!
//! # Timestamps
//!
//! NTP 64-bit format (RFC 8762's default; PTP not used), taken from the
//! system wall clock. Only DIFFERENCES of the two reflector timestamps
//! (T3−T2, the processing time) are consumed by the sender's arithmetic, so
//! reflector clock accuracy does not affect RTT correctness.
//!
//! # Deviations (documented)
//!
//! - The Session-Sender TTL field is set to 255: the received TTL is not
//!   readable from a portable unprivileged UDP socket. (RFC 8762 puts the
//!   received TTL here; 255 is the documented "unknown" fallback.)
//! - Error Estimate is reported as S=0 (unsynchronized), multiplier 1.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Unauthenticated STAMP test packets are exactly 44 bytes (RFC 8762 §4.2).
pub const STAMP_PACKET_LEN: usize = 44;

/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch (1970-01-01).
const NTP_UNIX_OFFSET_SECS: u64 = 2_208_988_800;

/// Idle time after which a sender's session (sequence counter) is dropped.
const SESSION_IDLE_EXPIRY: Duration = Duration::from_secs(60);

/// Error Estimate: S=0 (not synchronized to UTC), scale 0, multiplier 1
/// (RFC 4656 §4.1.2 format, referenced by RFC 8762).
const ERROR_ESTIMATE: [u8; 2] = [0x00, 0x01];

/// Current time as an NTP 64-bit timestamp (seconds, fraction).
pub fn ntp_now() -> (u32, u32) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = (now.as_secs() + NTP_UNIX_OFFSET_SECS) as u32; // wraps in 2036, like all NTP-era code
    let frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs, frac as u32)
}

/// Build the RFC 8762 §4.3 unauthenticated reflected packet.
///
/// Layout (offsets):
/// -  0..4   reflector Sequence Number (stateful per-sender counter)
/// -  4..12  Timestamp (T3 — set by the caller just before send)
/// - 12..14  Error Estimate (reflector's)
/// - 14..16  MBZ
/// - 16..24  Receive Timestamp (T2)
/// - 24..28  Session-Sender Sequence Number (copied)
/// - 28..36  Session-Sender Timestamp (copied, T1)
/// - 36..38  Session-Sender Error Estimate (copied)
/// - 38..40  MBZ
/// - 40      Session-Sender TTL (255 = not observable unprivileged)
/// - 41..44  MBZ
pub fn build_reflected_packet(
    request: &[u8],
    reflector_seq: u32,
    t2: (u32, u32),
    t3: (u32, u32),
) -> [u8; STAMP_PACKET_LEN] {
    let mut out = [0u8; STAMP_PACKET_LEN];
    out[0..4].copy_from_slice(&reflector_seq.to_be_bytes());
    out[4..8].copy_from_slice(&t3.0.to_be_bytes());
    out[8..12].copy_from_slice(&t3.1.to_be_bytes());
    out[12..14].copy_from_slice(&ERROR_ESTIMATE);
    // 14..16 MBZ
    out[16..20].copy_from_slice(&t2.0.to_be_bytes());
    out[20..24].copy_from_slice(&t2.1.to_be_bytes());
    out[24..28].copy_from_slice(&request[0..4]); // sender sequence
    out[28..36].copy_from_slice(&request[4..12]); // sender timestamp (T1)
    out[36..38].copy_from_slice(&request[12..14]); // sender error estimate
                                                   // 38..40 MBZ
    out[40] = 255; // Session-Sender TTL: not observable unprivileged
                   // 41..44 MBZ
    out
}

struct Session {
    next_seq: u32,
    last_seen: Instant,
}

/// Run the STAMP Session-Reflector until the task is aborted.
pub async fn run_stamp_reflector(socket: tokio::net::UdpSocket) {
    debug!(
        "STAMP reflector (RFC 8762 unauthenticated) listening on {:?}",
        socket.local_addr().ok()
    );
    run_stamp_reflector_on(socket).await;
}

/// Reflector loop on an already-bound socket (tests bind `127.0.0.1:0`
/// directly, avoiding learn-a-port/rebind races).
async fn run_stamp_reflector_on(socket: tokio::net::UdpSocket) {
    let mut sessions: HashMap<SocketAddr, Session> = HashMap::new();
    let mut last_sweep = Instant::now();
    let mut buf = vec![0u8; 2048];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, addr)) => {
                let t2 = ntp_now();
                if n < STAMP_PACKET_LEN {
                    debug!("STAMP: runt packet ({n} bytes) from {addr} — ignored");
                    continue;
                }
                // Periodic session sweep so the map cannot grow unbounded.
                if last_sweep.elapsed() > SESSION_IDLE_EXPIRY {
                    sessions.retain(|_, s| s.last_seen.elapsed() < SESSION_IDLE_EXPIRY);
                    last_sweep = Instant::now();
                }
                let session = sessions.entry(addr).or_insert(Session {
                    next_seq: 0,
                    last_seen: Instant::now(),
                });
                let reflector_seq = session.next_seq;
                session.next_seq = session.next_seq.wrapping_add(1);
                session.last_seen = Instant::now();

                let t3 = ntp_now();
                let reply = build_reflected_packet(&buf[..n], reflector_seq, t2, t3);
                if let Err(e) = socket.send_to(&reply, addr).await {
                    warn!("STAMP reflector send error: {e}");
                }
            }
            Err(e) => {
                warn!("STAMP reflector recv error: {e}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    fn sender_packet(seq: u32) -> [u8; STAMP_PACKET_LEN] {
        let mut p = [0u8; STAMP_PACKET_LEN];
        p[0..4].copy_from_slice(&seq.to_be_bytes());
        let (s, f) = ntp_now();
        p[4..8].copy_from_slice(&s.to_be_bytes());
        p[8..12].copy_from_slice(&f.to_be_bytes());
        p[12..14].copy_from_slice(&[0x00, 0x01]);
        p
    }

    #[test]
    fn ntp_now_is_after_the_2020s() {
        let (secs, _) = ntp_now();
        // 2020-01-01 in NTP-era seconds.
        assert!(secs > 3_786_825_600);
    }

    #[test]
    fn reflected_packet_copies_sender_fields_per_rfc8762() {
        let req = sender_packet(7);
        let out = build_reflected_packet(&req, 3, (100, 200), (101, 201));
        // Reflector sequence.
        assert_eq!(u32::from_be_bytes(out[0..4].try_into().unwrap()), 3);
        // T3.
        assert_eq!(u32::from_be_bytes(out[4..8].try_into().unwrap()), 101);
        assert_eq!(u32::from_be_bytes(out[8..12].try_into().unwrap()), 201);
        // T2.
        assert_eq!(u32::from_be_bytes(out[16..20].try_into().unwrap()), 100);
        assert_eq!(u32::from_be_bytes(out[20..24].try_into().unwrap()), 200);
        // Sender sequence + timestamp + error estimate copied verbatim.
        assert_eq!(&out[24..28], &req[0..4]);
        assert_eq!(&out[28..36], &req[4..12]);
        assert_eq!(&out[36..38], &req[12..14]);
        // TTL fallback.
        assert_eq!(out[40], 255);
        // MBZ regions are zero.
        assert_eq!(&out[14..16], &[0, 0]);
        assert_eq!(&out[38..40], &[0, 0]);
        assert_eq!(&out[41..44], &[0, 0, 0]);
    }

    #[tokio::test]
    async fn reflector_reflects_and_increments_sequence() {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let reflector_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = reflector_socket.local_addr().unwrap().port();
        let task = tokio::spawn(run_stamp_reflector_on(reflector_socket));

        // Wait for the reflector to answer (readiness poll — the first
        // packets may race the bind).
        let mut reply = [0u8; 2048];
        let mut attempt = 0u32;
        let n = loop {
            probe
                .send_to(&sender_packet(attempt), ("127.0.0.1", port))
                .await
                .unwrap();
            match tokio::time::timeout(Duration::from_millis(200), probe.recv(&mut reply)).await {
                Ok(Ok(n)) => break n,
                _ => {
                    attempt += 1;
                    assert!(attempt < 50, "reflector never answered");
                }
            }
        };
        assert_eq!(n, STAMP_PACKET_LEN);
        let first_reflector_seq = u32::from_be_bytes(reply[0..4].try_into().unwrap());

        // Second packet: reflector sequence must increment by exactly 1
        // (stateful mode) and echo OUR sequence back.
        probe
            .send_to(&sender_packet(attempt + 1), ("127.0.0.1", port))
            .await
            .unwrap();
        let n = tokio::time::timeout(Duration::from_secs(2), probe.recv(&mut reply))
            .await
            .expect("timeout")
            .unwrap();
        assert_eq!(n, STAMP_PACKET_LEN);
        let second_reflector_seq = u32::from_be_bytes(reply[0..4].try_into().unwrap());
        assert_eq!(second_reflector_seq, first_reflector_seq + 1);
        let echoed_sender_seq = u32::from_be_bytes(reply[24..28].try_into().unwrap());
        assert_eq!(echoed_sender_seq, attempt + 1);

        // T2 ≤ T3 (receive stamped before transmit).
        let t3s = u32::from_be_bytes(reply[4..8].try_into().unwrap());
        let t2s = u32::from_be_bytes(reply[16..20].try_into().unwrap());
        assert!(t3s >= t2s);

        task.abort();
    }

    #[tokio::test]
    async fn reflector_ignores_runt_packets() {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let reflector_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = reflector_socket.local_addr().unwrap().port();
        let task = tokio::spawn(run_stamp_reflector_on(reflector_socket));
        tokio::time::sleep(Duration::from_millis(100)).await;

        probe.send_to(b"tiny", ("127.0.0.1", port)).await.unwrap();
        let mut reply = [0u8; 128];
        let got = tokio::time::timeout(Duration::from_millis(300), probe.recv(&mut reply)).await;
        assert!(got.is_err(), "runt packet must not be reflected");
        task.abort();
    }
}
