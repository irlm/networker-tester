/// Best-effort OS-level socket telemetry.
///
/// # What we can obtain without root / CAP_NET_ADMIN
///
/// | Platform | TCP_MAXSEG | TCP_INFO / TCP_CONNECTION_INFO           | TCP_CONGESTION |
/// |----------|------------|------------------------------------------|----------------|
/// | Linux    | ✓          | ✓ (all fields, no root)                  | ✓              |
/// | macOS    | ✓          | ✓ (srtt/rttvar, cwnd/ssthresh, retrans)  | ✗ (no such option in xnu) |
/// | Windows  | ✗          | ✗                                        | ✗              |
///
/// # macOS unit semantics (xnu `struct tcp_connection_info`)
///
/// xnu reports `tcpi_srtt`/`tcpi_rttvar` in **milliseconds** (Linux: µs) and
/// `tcpi_snd_cwnd`/`tcpi_snd_ssthresh` in **bytes** (Linux: segments). To keep
/// [`SocketInfo`] cross-platform comparable we keep the ms values as-is
/// (so macOS RTTs have whole-ms resolution) and convert cwnd/ssthresh
/// bytes → segments by dividing by the socket's MSS (`TCP_MAXSEG`), reporting
/// `None` when the MSS is unknown rather than publishing bytes in a
/// segments-typed field. `tcpi_txretransmitpackets` (a lifetime counter) maps
/// to `total_retrans`, matching the Linux `tcpi_total_retrans` semantics.
/// Verified live on Darwin 25.5 (2026-07-27): option 0x106 returns optlen 112;
/// srtt to example.com read 57 ms with cwnd 13899 B / MSS 1388 = initcwnd 10.
///
/// # Linux tcp_info byte offsets used for version-guarded fields
///
/// The kernel struct has grown over releases.  We read into a raw `[u8; 232]`
/// buffer and gate each field on the `optlen` returned by `getsockopt`, so the
/// binary runs on any kernel ≥ 3.x but silently omits fields the running kernel
/// does not report.
///
/// | Offset | Size | Field              | Added    |
/// |--------|------|--------------------|----------|
/// |    68  |  u32 | tcpi_rtt (µs)      | baseline |
/// |    72  |  u32 | tcpi_rttvar (µs)   | baseline |
/// |    76  |  u32 | tcpi_snd_ssthresh  | baseline |
/// |    80  |  u32 | tcpi_snd_cwnd      | baseline |
/// |    96  |  u32 | tcpi_rcv_space     | baseline |
/// |   100  |  u32 | tcpi_total_retrans | baseline |
/// |   136  |  u32 | tcpi_segs_out      | 4.2      |
/// |   140  |  u32 | tcpi_segs_in       | 4.2      |
/// |   148  |  u32 | tcpi_min_rtt (µs)  | 4.9      |
/// |   160  |  u64 | tcpi_delivery_rate | 4.9      |
use tokio::net::TcpStream;

#[derive(Debug, Clone, Default)]
pub struct SocketInfo {
    /// Maximum Segment Size in bytes (TCP_MAXSEG). Best-effort.
    pub mss_bytes: Option<u32>,
    /// Smoothed RTT in ms (TCP_INFO on Linux, µs-derived; TCP_CONNECTION_INFO
    /// on macOS, whole-ms resolution — xnu reports srtt in ms).
    pub rtt_estimate_ms: Option<f64>,
    /// Segments currently queued for retransmit (Linux tcpi_retransmits).
    /// macOS has no equivalent field — always `None` there.
    pub retransmits: Option<u32>,
    /// Lifetime retransmission count (Linux: tcpi_total_retrans; macOS:
    /// tcpi_txretransmitpackets).
    pub total_retrans: Option<u32>,
    /// Congestion window in segments (Linux tcpi_snd_cwnd is segments; macOS
    /// reports bytes and is converted via MSS, `None` when MSS is unknown).
    pub snd_cwnd: Option<u32>,
    /// Slow-start threshold in segments (same per-platform conversion as
    /// `snd_cwnd`); None when the kernel "not yet set" sentinel is present.
    pub snd_ssthresh: Option<u32>,
    /// RTT variance in ms (tcpi_rttvar; whole-ms resolution on macOS).
    pub rtt_variance_ms: Option<f64>,
    /// Receiver advertised window in bytes (tcpi_rcv_space). Linux only.
    pub rcv_space: Option<u32>,
    /// Segments sent since connection start (Linux ≥ 4.2: tcpi_segs_out).
    pub segs_out: Option<u32>,
    /// Segments received since connection start (Linux ≥ 4.2: tcpi_segs_in).
    pub segs_in: Option<u32>,
    /// Congestion control algorithm name, e.g. "cubic", "bbr" (TCP_CONGESTION).
    pub congestion_algorithm: Option<String>,
    /// Estimated TCP delivery rate in bytes/sec (Linux ≥ 4.9: tcpi_delivery_rate).
    pub delivery_rate_bps: Option<u64>,
    /// Minimum RTT ever observed by the kernel in ms (Linux ≥ 4.9: tcpi_min_rtt).
    pub min_rtt_ms: Option<f64>,
}

impl SocketInfo {
    #[allow(unused_variables)]
    pub fn from_stream(stream: &TcpStream) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            return Self::from_raw_fd(stream.as_raw_fd());
        }
        #[allow(unreachable_code)]
        Self::default()
    }

    /// Query kernel TCP stats for an arbitrary raw fd referring to a TCP
    /// socket (used by [`SocketProbe`] to sample a `dup(2)` of the probe
    /// socket after the transfer). Best-effort: returns all-None on failure.
    #[cfg(unix)]
    #[allow(unused_variables)]
    pub fn from_raw_fd(fd: std::os::unix::io::RawFd) -> Self {
        #[cfg(target_os = "linux")]
        {
            return linux_socket_info(fd);
        }
        #[cfg(target_os = "macos")]
        {
            return macos_socket_info(fd);
        }
        #[allow(unreachable_code)]
        Self::default()
    }
}

impl From<SocketInfo> for crate::metrics::SocketStats {
    fn from(i: SocketInfo) -> Self {
        Self {
            mss_bytes: i.mss_bytes,
            rtt_estimate_ms: i.rtt_estimate_ms,
            retransmits: i.retransmits,
            total_retrans: i.total_retrans,
            snd_cwnd: i.snd_cwnd,
            snd_ssthresh: i.snd_ssthresh,
            rtt_variance_ms: i.rtt_variance_ms,
            rcv_space: i.rcv_space,
            segs_out: i.segs_out,
            segs_in: i.segs_in,
            congestion_algorithm: i.congestion_algorithm,
            delivery_rate_bps: i.delivery_rate_bps,
            min_rtt_ms: i.min_rtt_ms,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SocketProbe — post-transfer sampling handle
// ─────────────────────────────────────────────────────────────────────────────

/// A duplicated file descriptor onto a probe's TCP socket.
///
/// The HTTP-family probes hand their `TcpStream` to TLS/hyper, which owns it
/// for the rest of the request — so the socket is no longer reachable when
/// the transfer finishes, which is exactly when cwnd/retrans/delivery-rate
/// become meaningful. `dup(2)`-ing the fd before the handover keeps an
/// independent handle onto the *same* kernel socket (same file description),
/// so `getsockopt(TCP_INFO)` on the dup after the transfer reports
/// post-transfer state. Even if hyper closes its fd first, the dup keeps the
/// socket object alive for querying.
///
/// Read-only: the dup is only ever passed to `getsockopt`; no I/O happens on
/// it, so it cannot disturb the measurement.
///
/// On non-Unix platforms `new` returns `None` and stats stay absent.
pub struct SocketProbe {
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

impl SocketProbe {
    /// Duplicate the stream's fd. Returns `None` on non-Unix platforms or if
    /// `dup` fails (fd limit). Uses `F_DUPFD_CLOEXEC` so the dup never leaks
    /// into child processes.
    #[cfg(unix)]
    pub fn new(stream: &TcpStream) -> Option<Self> {
        use std::os::fd::{FromRawFd, OwnedFd};
        use std::os::unix::io::AsRawFd;
        let fd = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return None;
        }
        // SAFETY: fcntl(F_DUPFD_CLOEXEC) returned a fresh fd we now own; the
        // OwnedFd closes it on drop.
        Some(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    #[cfg(not(unix))]
    pub fn new(_stream: &TcpStream) -> Option<Self> {
        None
    }

    /// Sample the kernel's current TCP stats for the underlying socket.
    #[cfg(unix)]
    pub fn stats(&self) -> SocketInfo {
        use std::os::fd::AsRawFd;
        SocketInfo::from_raw_fd(self.fd.as_raw_fd())
    }

    #[cfg(not(unix))]
    pub fn stats(&self) -> SocketInfo {
        SocketInfo::default()
    }

    /// Post-transfer stats as the JSON-contract struct, or `None` when the
    /// kernel reported nothing (so reports store `null`, not `{}`).
    pub fn stats_for_result(&self) -> Option<crate::metrics::SocketStats> {
        let stats: crate::metrics::SocketStats = self.stats().into();
        if stats.is_empty() {
            None
        } else {
            Some(stats)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Linux
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn linux_socket_info(fd: std::os::unix::io::RawFd) -> SocketInfo {
    let mss = get_tcp_maxseg_linux(fd);
    let congestion_algorithm = get_congestion_algorithm_linux(fd);

    // Read tcp_info into a raw buffer.  We use byte-offset reads rather than
    // casting to libc::tcp_info so that fields added in later kernels (4.2, 4.9,
    // 4.13 …) are safely gated on the `optlen` the kernel actually filled.
    const BUF: usize = 232; // larger than any known tcp_info
    let mut buf = [0u8; BUF];
    let mut optlen = BUF as libc::socklen_t;

    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut optlen,
        )
    };

    if ret != 0 || (optlen as usize) < 104 {
        return SocketInfo {
            mss_bytes: mss,
            congestion_algorithm,
            ..Default::default()
        };
    }
    let n = optlen as usize;

    // Helper macros for safe offset reads.
    macro_rules! u32_at {
        ($off:expr) => {
            if n >= $off + 4 {
                Some(u32::from_ne_bytes(buf[$off..$off + 4].try_into().unwrap()))
            } else {
                None
            }
        };
    }
    macro_rules! u64_at {
        ($off:expr) => {
            if n >= $off + 8 {
                Some(u64::from_ne_bytes(buf[$off..$off + 8].try_into().unwrap()))
            } else {
                None
            }
        };
    }

    // tcpi_retransmits is a u8 at offset 2.
    let retransmits = if n > 2 && buf[2] > 0 {
        Some(buf[2] as u32)
    } else {
        None
    };

    let rtt_ms = u32_at!(68).and_then(|v| if v > 0 { Some(v as f64 / 1000.0) } else { None });
    let rtt_var_ms = u32_at!(72).and_then(|v| if v > 0 { Some(v as f64 / 1000.0) } else { None });
    let snd_ssthresh = u32_at!(76).and_then(|v| if v < 0x7fff_ffff { Some(v) } else { None });
    let snd_cwnd = u32_at!(80).and_then(|v| if v > 0 { Some(v) } else { None });
    let rcv_space = u32_at!(96).and_then(|v| if v > 0 { Some(v) } else { None });
    let total_retrans = u32_at!(100);

    // Linux ≥ 4.2
    let segs_out = u32_at!(136).and_then(|v| if v > 0 { Some(v) } else { None });
    let segs_in = u32_at!(140).and_then(|v| if v > 0 { Some(v) } else { None });

    // Linux ≥ 4.9
    let min_rtt_ms = u32_at!(148).and_then(|v| if v > 0 { Some(v as f64 / 1000.0) } else { None });
    let delivery_rate_bps = u64_at!(160).and_then(|v| if v > 0 { Some(v) } else { None });

    SocketInfo {
        mss_bytes: mss,
        rtt_estimate_ms: rtt_ms,
        retransmits,
        total_retrans,
        snd_cwnd,
        snd_ssthresh,
        rtt_variance_ms: rtt_var_ms,
        rcv_space,
        segs_out,
        segs_in,
        congestion_algorithm,
        delivery_rate_bps,
        min_rtt_ms,
    }
}

#[cfg(target_os = "linux")]
fn get_tcp_maxseg_linux(fd: libc::c_int) -> Option<u32> {
    unsafe {
        let mut val: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_MAXSEG,
            &mut val as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if ret == 0 && val > 0 {
            Some(val as u32)
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn get_congestion_algorithm_linux(fd: libc::c_int) -> Option<String> {
    unsafe {
        let mut buf = [0u8; 32];
        let mut len = 32u32;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_CONGESTION,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
        );
        if ret == 0 && len > 0 {
            let s = std::str::from_utf8(&buf[..len as usize])
                .unwrap_or("")
                .trim_end_matches('\0');
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// macOS
// ─────────────────────────────────────────────────────────────────────────────

/// Full layout of xnu's `struct tcp_connection_info` (112 bytes), mirrored
/// field-for-field from the macOS SDK `netinet/tcp.h` (verified against
/// /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk on Darwin 25.5).
///
/// Units per the xnu header comments:
/// - `tcpi_rttcur`/`tcpi_srtt`/`tcpi_rttvar` — **milliseconds**
/// - `tcpi_snd_ssthresh`/`tcpi_snd_cwnd`/`tcpi_snd_wnd`/`tcpi_rcv_wnd` — **bytes**
/// - the `u32` at offset 52 is the TFO **bitfield**, NOT a retransmit count
///   (the pre-v0.28.82 struct misread it as `tcpi_txretransmitpackets`);
///   the real `tcpi_txretransmitpackets` is the `u64` at offset 104.
#[cfg(target_os = "macos")]
#[repr(C)]
struct TcpConnectionInfo {
    tcpi_state: u8,
    tcpi_snd_wscale: u8,
    tcpi_rcv_wscale: u8,
    _pad1: u8,
    tcpi_options: u32,
    tcpi_flags: u32,
    tcpi_rto: u32,          // retransmit timeout, ms
    tcpi_maxseg: u32,       // MSS, bytes
    tcpi_snd_ssthresh: u32, // slow-start threshold, BYTES
    tcpi_snd_cwnd: u32,     // congestion window, BYTES
    tcpi_snd_wnd: u32,      // send window, bytes
    tcpi_snd_sbbytes: u32,  // bytes in send socket buffer
    tcpi_rcv_wnd: u32,      // receive window, bytes
    tcpi_rttcur: u32,       // most recent RTT, MILLISECONDS
    tcpi_srtt: u32,         // smoothed RTT, MILLISECONDS
    tcpi_rttvar: u32,       // RTT variance, MILLISECONDS
    tcpi_tfo_bits: u32,     // TCP Fast Open bitfield (15 flags + padding)
    tcpi_txpackets: u64,    // 8-aligned lifetime counters follow
    tcpi_txbytes: u64,
    tcpi_txretransmitbytes: u64,
    tcpi_rxpackets: u64,
    tcpi_rxbytes: u64,
    tcpi_rxoutoforderbytes: u64,
    tcpi_txretransmitpackets: u64, // lifetime retransmitted packets (offset 104)
}

/// `TCP_CONNECTION_INFO` per xnu `netinet/tcp.h`. The previous value (0x24)
/// is not a valid xnu TCP option — every `getsockopt` with it failed
/// `ENOPROTOOPT` and macOS TCP stats were silently absent since they shipped
/// (verified live on this Darwin 25.5 machine: 0x24 → "Protocol not
/// available", 0x106 → optlen 112 with plausible srtt/cwnd).
#[cfg(target_os = "macos")]
const TCP_CONNECTION_INFO_OPT: libc::c_int = 0x106;

/// xnu's "slow-start threshold not yet set" sentinel:
/// `TCP_MAXWIN << TCP_MAX_WINSHIFT` = 65535 << 14 = 1_073_725_440 bytes,
/// observed verbatim on idle connections during the live verification.
#[cfg(target_os = "macos")]
const MACOS_SSTHRESH_UNSET: u32 = 65_535 << 14;

#[cfg(target_os = "macos")]
fn macos_socket_info(fd: std::os::unix::io::RawFd) -> SocketInfo {
    let mss = unsafe {
        let mut val: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_MAXSEG,
            &mut val as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if ret == 0 && val > 0 {
            Some(val as u32)
        } else {
            None
        }
    };

    // xnu has NO `TCP_CONGESTION` getsockopt (option 0x20 is
    // `TCP_CONNECTIONTIMEOUT` — an int; the previous code read it and
    // interpreted the raw bytes as a UTF-8 algorithm name). The congestion
    // algorithm is simply not observable per-socket on macOS: honest `None`.
    let congestion_algorithm = None;

    let (rtt_ms, rtt_var_ms, snd_cwnd, snd_ssthresh, total_retrans) = unsafe {
        let mut info: TcpConnectionInfo = std::mem::zeroed();
        let mut len = std::mem::size_of::<TcpConnectionInfo>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            TCP_CONNECTION_INFO_OPT,
            &mut info as *mut _ as *mut libc::c_void,
            &mut len,
        );
        // Gate on the length the kernel actually filled: the u32 core needs
        // 56 bytes, the u64 lifetime counters need the full 112.
        if ret == 0 && (len as usize) >= 56 {
            // srtt/rttvar are already in milliseconds (whole-ms resolution;
            // xnu floors sub-ms loopback RTTs to 1 ms). Dividing by 1000 as
            // the pre-fix code did would under-report macOS RTT 1000×.
            let rtt = (info.tcpi_srtt > 0).then_some(info.tcpi_srtt as f64);
            let rttvar = (info.tcpi_rttvar > 0).then_some(info.tcpi_rttvar as f64);
            // cwnd/ssthresh are bytes on macOS; convert to segments via MSS
            // so the field means the same thing on Linux and macOS. Without a
            // usable MSS the honest answer is None, not bytes-as-segments.
            let to_segments = |bytes: u32| -> Option<u32> {
                match mss {
                    Some(m) if m > 0 && bytes > 0 => {
                        Some(((bytes as f64 / m as f64).round() as u32).max(1))
                    }
                    _ => None,
                }
            };
            let cwnd = to_segments(info.tcpi_snd_cwnd);
            let ssthresh = if info.tcpi_snd_ssthresh >= MACOS_SSTHRESH_UNSET {
                None
            } else {
                to_segments(info.tcpi_snd_ssthresh)
            };
            let retrans = ((len as usize) >= std::mem::size_of::<TcpConnectionInfo>())
                .then(|| u32::try_from(info.tcpi_txretransmitpackets).unwrap_or(u32::MAX));
            (rtt, rttvar, cwnd, ssthresh, retrans)
        } else {
            (None, None, None, None, None)
        }
    };

    SocketInfo {
        mss_bytes: mss,
        rtt_estimate_ms: rtt_ms,
        // xnu exposes no "currently queued for retransmit" count; the lifetime
        // tcpi_txretransmitpackets counter maps to total_retrans instead
        // (Linux tcpi_total_retrans semantics).
        retransmits: None,
        total_retrans,
        snd_cwnd,
        snd_ssthresh,
        rtt_variance_ms: rtt_var_ms,
        rcv_space: None,
        segs_out: None,
        segs_in: None,
        congestion_algorithm,
        delivery_rate_bps: None, // not available via TCP_CONNECTION_INFO
        min_rtt_ms: None,        // not available via TCP_CONNECTION_INFO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_info_default_is_none() {
        let info = SocketInfo::default();
        assert!(info.mss_bytes.is_none());
        assert!(info.rtt_estimate_ms.is_none());
        assert!(info.retransmits.is_none());
        assert!(info.snd_cwnd.is_none());
        assert!(info.rtt_variance_ms.is_none());
        assert!(info.congestion_algorithm.is_none());
        assert!(info.delivery_rate_bps.is_none());
        assert!(info.min_rtt_ms.is_none());
    }

    #[test]
    fn default_socket_info_converts_to_empty_socket_stats() {
        let stats: crate::metrics::SocketStats = SocketInfo::default().into();
        assert!(stats.is_empty());
    }

    #[test]
    fn socket_info_conversion_preserves_fields() {
        let info = SocketInfo {
            mss_bytes: Some(1460),
            rtt_estimate_ms: Some(1.5),
            total_retrans: Some(3),
            snd_cwnd: Some(40),
            congestion_algorithm: Some("cubic".into()),
            delivery_rate_bps: Some(12_000_000),
            ..Default::default()
        };
        let stats: crate::metrics::SocketStats = info.into();
        assert!(!stats.is_empty());
        assert_eq!(stats.mss_bytes, Some(1460));
        assert_eq!(stats.rtt_estimate_ms, Some(1.5));
        assert_eq!(stats.total_retrans, Some(3));
        assert_eq!(stats.snd_cwnd, Some(40));
        assert_eq!(stats.congestion_algorithm.as_deref(), Some("cubic"));
        assert_eq!(stats.delivery_rate_bps, Some(12_000_000));
    }

    /// The dup(2) handle must keep reporting kernel stats for the socket even
    /// after the original stream is dropped — this is exactly the hyper
    /// scenario, where the connection task owns (and may close) the original
    /// fd before we sample post-transfer stats.
    #[cfg(unix)]
    #[tokio::test]
    async fn socket_probe_dup_survives_stream_drop() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stream = TcpStream::connect(addr).await.unwrap();
        let (_server_side, _) = listener.accept().await.unwrap();

        let probe = SocketProbe::new(&stream).expect("dup should succeed on Unix");
        drop(stream);

        // Linux and macOS both report at least MSS + smoothed RTT (or the
        // congestion algorithm) on a live loopback socket.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let stats = probe
                .stats_for_result()
                .expect("dup'd fd should still report TCP kernel stats");
            // Regression guard for the broken TCP_CONNECTION_INFO constant
            // (0x24 instead of xnu's 0x106): the old test passed on MSS
            // alone, masking that EVERY macOS TCP_CONNECTION_INFO field was
            // silently None since ship. Require the fields the option call
            // actually provides. xnu reports srtt in whole ms and floors
            // loopback RTT to 1 ms, so it is reliably present here; cwnd
            // (bytes ÷ MSS) is always nonzero on an established connection.
            #[cfg(target_os = "macos")]
            {
                assert!(
                    stats.rtt_estimate_ms.is_some(),
                    "macOS TCP_CONNECTION_INFO must yield a smoothed RTT \
                     (broken getsockopt constant?): {stats:?}"
                );
                assert!(
                    stats.snd_cwnd.is_some(),
                    "macOS TCP_CONNECTION_INFO must yield cwnd segments: {stats:?}"
                );
            }
            let _ = &stats;
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = probe.stats();
        }
    }

    /// xnu's `struct tcp_connection_info` is exactly 112 bytes; the u64
    /// lifetime counters start at offset 56 and `tcpi_txretransmitpackets`
    /// sits at offset 104 (NOT 52, which is the TFO bitfield). A drifted
    /// layout would silently read garbage — pin size and key offsets.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tcp_connection_info_layout_matches_xnu() {
        assert_eq!(std::mem::size_of::<TcpConnectionInfo>(), 112);
        let probe: TcpConnectionInfo = unsafe { std::mem::zeroed() };
        let base = &probe as *const _ as usize;
        assert_eq!(&probe.tcpi_snd_ssthresh as *const _ as usize - base, 20);
        assert_eq!(&probe.tcpi_snd_cwnd as *const _ as usize - base, 24);
        assert_eq!(&probe.tcpi_srtt as *const _ as usize - base, 44);
        assert_eq!(&probe.tcpi_rttvar as *const _ as usize - base, 48);
        assert_eq!(&probe.tcpi_tfo_bits as *const _ as usize - base, 52);
        assert_eq!(&probe.tcpi_txpackets as *const _ as usize - base, 56);
        assert_eq!(
            &probe.tcpi_txretransmitpackets as *const _ as usize - base,
            104
        );
    }
}
