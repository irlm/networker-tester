//! One-shot SNTP (RFC 4330) clock-sync cross-check (measurement-gap #16).
//!
//! Runs ONCE per run alongside client-info collection to validate the local
//! clock against an NTP server, independently of the per-attempt
//! `ServerTimingResult::clock_skew_ms` heuristic (whose semantics are
//! unchanged). Fully best-effort:
//!
//! * `NETWORKER_NTP_DISABLE=1` opts out entirely.
//! * `NETWORKER_NTP_SERVER` overrides the server (default `pool.ntp.org:123`;
//!   `:123` is appended when no port is given).
//! * 1s socket timeout, plus an outer async timeout that also bounds DNS
//!   resolution — the query can never stall the run.
//! * Any failure (resolve, send, timeout, malformed reply) → `None`.

use std::net::UdpSocket;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::metrics::ClockSync;

/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch (1970-01-01).
const NTP_UNIX_EPOCH_DELTA: f64 = 2_208_988_800.0;
/// SNTP packet size (RFC 4330 §4, without authenticator).
const SNTP_PACKET_LEN: usize = 48;
/// Per-socket send/recv timeout.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(1);
/// Outer bound on the whole query including DNS resolution.
const OVERALL_TIMEOUT: Duration = Duration::from_millis(2500);

/// Server receive + transmit timestamps parsed from an SNTP reply, as Unix
/// seconds (fractional).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SntpServerTimestamps {
    /// T2 — when the server received our request.
    pub receive_unix_secs: f64,
    /// T3 — when the server sent its reply.
    pub transmit_unix_secs: f64,
}

/// Convert Unix seconds to the NTP 64-bit timestamp wire format
/// (32-bit seconds since 1900 + 32-bit fraction, big-endian).
fn unix_secs_to_ntp_bytes(unix_secs: f64) -> [u8; 8] {
    let ntp_secs = unix_secs + NTP_UNIX_EPOCH_DELTA;
    let secs = ntp_secs.floor();
    let frac = ((ntp_secs - secs) * (u32::MAX as f64 + 1.0)) as u64;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(secs as u32).to_be_bytes());
    out[4..].copy_from_slice(&((frac as u32).to_be_bytes()));
    out
}

/// Convert an NTP 64-bit timestamp (big-endian bytes) to Unix seconds.
/// Returns `None` for the "unset" all-zero timestamp.
fn ntp_bytes_to_unix_secs(bytes: &[u8]) -> Option<f64> {
    let secs = u32::from_be_bytes(bytes[..4].try_into().ok()?);
    let frac = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    if secs == 0 && frac == 0 {
        return None;
    }
    let ntp_secs = secs as f64 + frac as f64 / (u32::MAX as f64 + 1.0);
    Some(ntp_secs - NTP_UNIX_EPOCH_DELTA)
}

/// Build an SNTP client request packet (LI=0, VN=4, Mode=3 → first byte
/// 0x23), stamping the Transmit Timestamp field with the client send time.
pub fn build_client_packet(transmit_unix_secs: f64) -> [u8; SNTP_PACKET_LEN] {
    let mut pkt = [0u8; SNTP_PACKET_LEN];
    pkt[0] = 0x23; // LI=0 (no warning), VN=4, Mode=3 (client)
    pkt[40..48].copy_from_slice(&unix_secs_to_ntp_bytes(transmit_unix_secs));
    pkt
}

/// Parse a server reply: validates length, server/broadcast mode, and a
/// non-zero stratum (stratum 0 is a kiss-of-death), then extracts the
/// Receive (T2, bytes 32..40) and Transmit (T3, bytes 40..48) timestamps.
pub fn parse_server_packet(buf: &[u8]) -> Option<SntpServerTimestamps> {
    if buf.len() < SNTP_PACKET_LEN {
        return None;
    }
    let mode = buf[0] & 0x07;
    if mode != 4 && mode != 5 {
        return None; // not a server (4) or broadcast (5) reply
    }
    if buf[1] == 0 {
        return None; // stratum 0 = kiss-of-death / unsynchronized
    }
    Some(SntpServerTimestamps {
        receive_unix_secs: ntp_bytes_to_unix_secs(&buf[32..40])?,
        transmit_unix_secs: ntp_bytes_to_unix_secs(&buf[40..48])?,
    })
}

/// Standard NTP offset/delay computation (RFC 4330 §5), all inputs in Unix
/// seconds. Returns `(offset_secs, round_trip_secs)`.
pub fn compute_offset_and_delay(t1: f64, t2: f64, t3: f64, t4: f64) -> (f64, f64) {
    let offset = ((t2 - t1) + (t3 - t4)) / 2.0;
    let delay = (t4 - t1) - (t3 - t2);
    (offset, delay)
}

fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Resolve the configured NTP server address (appending `:123` when the
/// override has no port).
fn ntp_server_addr() -> String {
    let configured = std::env::var("NETWORKER_NTP_SERVER")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "pool.ntp.org:123".to_string());
    // Append the default port when none was given (IPv6 literals with ports
    // use brackets, so a bare ':' check is sufficient for hostnames/IPv4).
    if configured.contains(':') {
        configured
    } else {
        format!("{configured}:123")
    }
}

/// Blocking one-shot SNTP exchange. Returns `None` on any failure.
fn query_sntp_blocking(server: &str) -> Option<ClockSync> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(SOCKET_TIMEOUT)).ok()?;
    socket.set_write_timeout(Some(SOCKET_TIMEOUT)).ok()?;
    // connect() performs the (blocking) DNS resolution of the pool hostname.
    socket.connect(server).ok()?;

    let t1 = now_unix_secs();
    let request = build_client_packet(t1);
    socket.send(&request).ok()?;

    let mut buf = [0u8; 128];
    let n = socket.recv(&mut buf).ok()?;
    let t4 = now_unix_secs();

    let ts = parse_server_packet(&buf[..n])?;
    let (offset_secs, delay_secs) =
        compute_offset_and_delay(t1, ts.receive_unix_secs, ts.transmit_unix_secs, t4);
    if !offset_secs.is_finite() || !delay_secs.is_finite() || delay_secs < 0.0 {
        return None;
    }
    Some(ClockSync {
        ntp_server: Some(server.to_string()),
        offset_ms: Some(offset_secs * 1000.0),
        round_trip_ms: Some(delay_secs * 1000.0),
    })
}

/// Best-effort one-shot clock-sync query, bounded by [`OVERALL_TIMEOUT`]
/// (which also covers DNS resolution). Returns `None` when disabled via
/// `NETWORKER_NTP_DISABLE=1` or on any failure.
pub async fn query_clock_sync() -> Option<ClockSync> {
    if std::env::var("NETWORKER_NTP_DISABLE").is_ok_and(|v| v == "1") {
        return None;
    }
    let server = ntp_server_addr();
    tokio::time::timeout(
        OVERALL_TIMEOUT,
        tokio::task::spawn_blocking(move || query_sntp_blocking(&server)),
    )
    .await
    .ok()?
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid 48-byte server reply with the given T2/T3 (Unix secs).
    fn server_reply(t2_unix: f64, t3_unix: f64) -> [u8; SNTP_PACKET_LEN] {
        let mut pkt = [0u8; SNTP_PACKET_LEN];
        pkt[0] = 0x24; // LI=0, VN=4, Mode=4 (server)
        pkt[1] = 2; // stratum 2
        pkt[32..40].copy_from_slice(&unix_secs_to_ntp_bytes(t2_unix));
        pkt[40..48].copy_from_slice(&unix_secs_to_ntp_bytes(t3_unix));
        pkt
    }

    #[test]
    fn client_packet_has_version_mode_and_transmit_timestamp() {
        let pkt = build_client_packet(1_700_000_000.5);
        assert_eq!(pkt.len(), SNTP_PACKET_LEN);
        assert_eq!(pkt[0], 0x23, "LI=0 VN=4 Mode=3");
        // Transmit timestamp (bytes 40..48) round-trips through the NTP format.
        let back = ntp_bytes_to_unix_secs(&pkt[40..48]).expect("transmit timestamp set");
        assert!((back - 1_700_000_000.5).abs() < 1e-4, "got {back}");
        // Everything before the transmit timestamp except the header is zero.
        assert!(pkt[1..40].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_server_packet_extracts_fixture_timestamps() {
        let t2 = 1_700_000_010.25;
        let t3 = 1_700_000_010.75;
        let ts = parse_server_packet(&server_reply(t2, t3)).expect("valid reply");
        assert!((ts.receive_unix_secs - t2).abs() < 1e-4);
        assert!((ts.transmit_unix_secs - t3).abs() < 1e-4);
    }

    #[test]
    fn parse_server_packet_rejects_bad_replies() {
        // Too short.
        assert!(parse_server_packet(&[0u8; 20]).is_none());
        // Wrong mode (client, 3).
        let mut pkt = server_reply(1.0e9, 1.0e9);
        pkt[0] = 0x23;
        assert!(parse_server_packet(&pkt).is_none());
        // Stratum 0 (kiss-of-death).
        let mut pkt = server_reply(1.0e9, 1.0e9);
        pkt[1] = 0;
        assert!(parse_server_packet(&pkt).is_none());
        // All-zero (unset) timestamps.
        let mut pkt = [0u8; SNTP_PACKET_LEN];
        pkt[0] = 0x24;
        pkt[1] = 2;
        assert!(parse_server_packet(&pkt).is_none());
    }

    #[test]
    fn offset_and_delay_match_rfc4330_formulas() {
        // Client clock 100ms behind server; 40ms symmetric path; 10ms server
        // processing. T1=10.000 (client), T2=10.120 (server = client+0.1+0.02),
        // T3=10.130, T4=10.050 (client).
        let (offset, delay) = compute_offset_and_delay(10.000, 10.120, 10.130, 10.050);
        assert!((offset - 0.100).abs() < 1e-9, "offset {offset}");
        assert!((delay - 0.040).abs() < 1e-9, "delay {delay}");
    }

    #[test]
    fn ntp_timestamp_round_trip() {
        // NTP era 0 covers Unix times up to ~2036 (2^32 − 2208988800 secs);
        // stay within it — the fraction gives ~2.3e-10s resolution.
        for secs in [0.0f64, 1.5, 1_234_567_890.123, 2_000_000_000.9] {
            let back = ntp_bytes_to_unix_secs(&unix_secs_to_ntp_bytes(secs)).unwrap();
            assert!((back - secs).abs() < 1e-4, "{secs} -> {back}");
        }
    }
}
