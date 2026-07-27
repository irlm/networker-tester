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
/// The kernel struct has grown over releases (uapi `linux/tcp.h`, verified
/// against torvalds/linux master 2026-07: append-only through the 6.17 AccECN
/// fields, 280 bytes total).  We read into a raw `[u8; 288]` buffer and gate
/// each field on the `optlen` returned by `getsockopt`, so the binary runs on
/// any kernel ≥ 3.x but silently omits fields the running kernel does not
/// report.  The `TcpInfoLayout` test mirror below pins every offset.
///
/// | Offset | Size | Field                     | Added    |
/// |--------|------|---------------------------|----------|
/// |     5  |  u8  | tcpi_options (bitflags)   | baseline |
/// |     7  |  u8  | delivery_rate_app_limited | 4.9 (bit 0 of the byte) |
/// |    68  |  u32 | tcpi_rtt (µs)             | baseline |
/// |    72  |  u32 | tcpi_rttvar (µs)          | baseline |
/// |    76  |  u32 | tcpi_snd_ssthresh         | baseline |
/// |    80  |  u32 | tcpi_snd_cwnd             | baseline |
/// |    92  |  u32 | tcpi_rcv_rtt (µs)         | baseline |
/// |    96  |  u32 | tcpi_rcv_space            | baseline |
/// |   100  |  u32 | tcpi_total_retrans        | baseline |
/// |   104  |  u64 | tcpi_pacing_rate (B/s)    | 3.15     |
/// |   120  |  u64 | tcpi_bytes_acked          | 4.1      |
/// |   136  |  u32 | tcpi_segs_out             | 4.2      |
/// |   140  |  u32 | tcpi_segs_in              | 4.2      |
/// |   144  |  u32 | tcpi_notsent_bytes        | 4.6      |
/// |   148  |  u32 | tcpi_min_rtt (µs)         | 4.6      |
/// |   160  |  u64 | tcpi_delivery_rate        | 4.9      |
/// |   168  |  u64 | tcpi_busy_time (µs)       | 4.10     |
/// |   176  |  u64 | tcpi_rwnd_limited (µs)    | 4.10     |
/// |   184  |  u64 | tcpi_sndbuf_limited (µs)  | 4.10     |
/// |   192  |  u32 | tcpi_delivered            | 4.18     |
/// |   196  |  u32 | tcpi_delivered_ce         | 4.18     |
/// |   200  |  u64 | tcpi_bytes_sent           | 4.19     |
/// |   208  |  u64 | tcpi_bytes_retrans        | 4.19     |
/// |   216  |  u32 | tcpi_dsack_dups           | 4.19     |
/// |   220  |  u32 | tcpi_reord_seen           | 4.19     |
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
    // ── B.2 full tcp_info additions (all Linux-only; honest None elsewhere) ──
    /// µs the connection spent busy sending data (Linux ≥ 4.10:
    /// tcpi_busy_time). Denominator of the throughput-attribution triad.
    pub busy_time_us: Option<u64>,
    /// µs of busy time limited by the peer's receive window (Linux ≥ 4.10:
    /// tcpi_rwnd_limited) — bottleneck was the receiver, not the path.
    pub rwnd_limited_us: Option<u64>,
    /// µs of busy time limited by our own send buffer (Linux ≥ 4.10:
    /// tcpi_sndbuf_limited) — bottleneck was local, not the path.
    pub sndbuf_limited_us: Option<u64>,
    /// Bytes acked by the peer (Linux ≥ 4.1: tcpi_bytes_acked, RFC 4898
    /// tcpEStatsAppHCThruOctetsAcked).
    pub bytes_acked: Option<u64>,
    /// Bytes sent incl. retransmissions (Linux ≥ 4.19: tcpi_bytes_sent,
    /// RFC 4898 tcpEStatsPerfHCDataOctetsOut).
    pub bytes_sent: Option<u64>,
    /// Bytes retransmitted (Linux ≥ 4.19: tcpi_bytes_retrans, RFC 4898
    /// tcpEStatsPerfOctetsRetrans) — the RFC 6349 retransmitted-bytes-ratio
    /// numerator over `bytes_sent`.
    pub bytes_retrans: Option<u64>,
    /// Data packets delivered to the peer incl. retransmits (Linux ≥ 4.18:
    /// tcpi_delivered).
    pub delivered: Option<u32>,
    /// Delivered packets that carried a CE mark (Linux ≥ 4.18:
    /// tcpi_delivered_ce) — a real ECN/L4S congestion signal (RFC 3168/9330).
    pub delivered_ce: Option<u32>,
    /// ECN was negotiated at session establishment (tcpi_options bit
    /// TCPI_OPT_ECN).
    pub ecn_negotiated: Option<bool>,
    /// TCP Fast Open data was used on the SYN (tcpi_options bit
    /// TCPI_OPT_SYN_DATA).
    pub tfo_used: Option<bool>,
    /// The `delivery_rate_bps` sample was application-limited rather than
    /// network-limited (Linux ≥ 4.9: tcpi_delivery_rate_app_limited bitfield)
    /// — when true the delivery rate is NOT a path-capacity signal.
    pub app_limited: Option<bool>,
    /// Kernel pacing rate in bytes/sec (Linux ≥ 3.15: tcpi_pacing_rate).
    pub pacing_rate_bps: Option<u64>,
    /// Bytes buffered but not yet sent at sample time (Linux ≥ 4.6:
    /// tcpi_notsent_bytes).
    pub notsent_bytes: Option<u32>,
    /// Reordering events seen (Linux ≥ 4.19: tcpi_reord_seen). With
    /// `dsack_dups` distinguishes spurious retransmission from genuine loss.
    pub reord_seen: Option<u32>,
    /// DSACK-reported duplicate segments (Linux ≥ 4.19: tcpi_dsack_dups,
    /// RFC 4898 tcpEStatsStackDSACKDups).
    pub dsack_dups: Option<u32>,
    /// Receiver-side RTT estimate in ms (tcpi_rcv_rtt, µs in the kernel).
    pub rcv_rtt_ms: Option<f64>,
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
            busy_time_us: i.busy_time_us,
            rwnd_limited_us: i.rwnd_limited_us,
            sndbuf_limited_us: i.sndbuf_limited_us,
            bytes_acked: i.bytes_acked,
            bytes_sent: i.bytes_sent,
            bytes_retrans: i.bytes_retrans,
            delivered: i.delivered,
            delivered_ce: i.delivered_ce,
            ecn_negotiated: i.ecn_negotiated,
            tfo_used: i.tfo_used,
            app_limited: i.app_limited,
            pacing_rate_bps: i.pacing_rate_bps,
            notsent_bytes: i.notsent_bytes,
            reord_seen: i.reord_seen,
            dsack_dups: i.dsack_dups,
            rcv_rtt_ms: i.rcv_rtt_ms,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// tcp_info bit decoding (pure helpers, unit-tested on every platform)
// ─────────────────────────────────────────────────────────────────────────────

/// `tcpi_options` bitflags (uapi `linux/tcp.h`, stable since their addition).
/// TCPI_OPT_ECN: ECN negotiated at session init. TCPI_OPT_SYN_DATA (3.7+,
/// TFO era): data was acked on the SYN — TCP Fast Open was actually used.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const TCPI_OPT_ECN: u8 = 8;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const TCPI_OPT_SYN_DATA: u8 = 32;

/// Decode `(ecn_negotiated, tfo_used)` from the `tcpi_options` byte (@5).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn decode_tcpi_options(options: u8) -> (bool, bool) {
    (
        options & TCPI_OPT_ECN != 0,
        options & TCPI_OPT_SYN_DATA != 0,
    )
}

/// Decode `tcpi_delivery_rate_app_limited` from byte @7 of tcp_info.
///
/// The byte is a C bitfield (`app_limited:1, fastopen_client_fail:2, pad:5`);
/// the first-declared bitfield occupies the least-significant bit on
/// little-endian targets and the most-significant bit on big-endian targets
/// (GCC/Clang bitfield allocation order follows target endianness). Kernel
/// 4.9 added the bit — callers must additionally gate on the 4.9 struct
/// length (`optlen ≥ 168`) because older kernels report the byte as zero
/// padding, which would decode as a false `Some(false)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn decode_app_limited(byte7: u8) -> bool {
    #[cfg(target_endian = "little")]
    const APP_LIMITED_BIT: u8 = 0x01;
    #[cfg(target_endian = "big")]
    const APP_LIMITED_BIT: u8 = 0x80;
    byte7 & APP_LIMITED_BIT != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// UDP socket diagnostics — local-drop vs path-loss split (B.6)
// ─────────────────────────────────────────────────────────────────────────────

/// Post-transfer observations of a receiving UDP socket.
///
/// `local_drops` splits "loss" honestly: datagrams the kernel dropped because
/// THIS socket's receive buffer was full arrived over the path fine — counting
/// them as path loss (as all pre-fix results do) blames the network for a
/// local bottleneck.
///
/// Sourced from `getsockopt(SOL_SOCKET, SO_MEMINFO)` (Linux ≥ 4.14,
/// socket(7)): one syscall at transfer end, zero hot-path cost. The
/// `SK_MEMINFO_DROPS` slot is the same cumulative `sk_drops` counter the
/// `SO_RXQ_OVFL`/`SCM_RXQ_OVFL` cmsg reports, without per-datagram
/// `recvmsg` cmsg parsing (the report offers either; the probe sockets are
/// created fresh per transfer, so the cumulative counter IS the per-transfer
/// count). macOS exposes no per-socket drop counter (no SO_RXQ_OVFL /
/// SO_MEMINFO; SO_NREAD reports current queued bytes, not drops) — honest
/// `None`. Windows: `None` (no per-socket counter without privileged ETW).
///
/// `so_rcvbuf_bytes` is the effective `SO_RCVBUF` at transfer end (Unix; on
/// Linux the kernel-doubled bookkeeping value, as sysadmins see in `ss -m`),
/// recorded so a drop report carries its context.
#[derive(Debug, Clone, Default)]
pub struct UdpSocketDiag {
    /// Datagrams dropped by the kernel on this socket (rcvbuf overflow).
    /// `None` = unobservable on this platform/kernel, NOT zero.
    pub local_drops: Option<u64>,
    /// Effective receive buffer size in bytes at sample time.
    pub so_rcvbuf_bytes: Option<u32>,
}

impl UdpSocketDiag {
    /// Sample drop/buffer state for a UDP socket. Best-effort: all-None on
    /// unsupported platforms or failed getsockopt.
    #[allow(unused_variables)]
    pub fn from_socket(sock: &tokio::net::UdpSocket) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = sock.as_raw_fd();
            return Self {
                local_drops: udp_local_drops(fd),
                so_rcvbuf_bytes: so_rcvbuf(fd),
            };
        }
        #[allow(unreachable_code)]
        Self::default()
    }
}

/// Effective SO_RCVBUF in bytes (any Unix).
#[cfg(unix)]
fn so_rcvbuf(fd: std::os::unix::io::RawFd) -> Option<u32> {
    unsafe {
        let mut val: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
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

/// Per-socket dropped-datagram counter via SO_MEMINFO (Linux ≥ 4.14).
///
/// Returns the `SK_MEMINFO_DROPS` slot (`sk_drops`): datagrams the kernel
/// discarded on this socket — for UDP that is receive-queue overflow (and
/// rmem exhaustion). Gated on the syscall succeeding AND the kernel filling
/// all 9 `u32` slots; older kernels → `None`, never a fabricated 0.
#[cfg(target_os = "linux")]
fn udp_local_drops(fd: std::os::unix::io::RawFd) -> Option<u64> {
    const SLOTS: usize = 9; // SK_MEMINFO_VARS
    let mut vals = [0u32; SLOTS];
    let mut len = (SLOTS * std::mem::size_of::<u32>()) as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_MEMINFO,
            vals.as_mut_ptr() as *mut libc::c_void,
            &mut len,
        )
    };
    if ret == 0 && (len as usize) >= SLOTS * std::mem::size_of::<u32>() {
        Some(vals[libc::SK_MEMINFO_DROPS as usize] as u64)
    } else {
        None
    }
}

/// macOS/other Unix: no per-socket UDP drop counter — honest `None`.
#[cfg(all(unix, not(target_os = "linux")))]
fn udp_local_drops(_fd: std::os::unix::io::RawFd) -> Option<u64> {
    None
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
    // 288 covers the current uapi struct (280 bytes as of the 6.17 AccECN
    // fields; append-only) with headroom — the kernel copies min(optlen,
    // sizeof(struct)) and reports what it filled.
    const BUF: usize = 288;
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
    let rcv_rtt_ms = u32_at!(92).and_then(|v| if v > 0 { Some(v as f64 / 1000.0) } else { None });
    let rcv_space = u32_at!(96).and_then(|v| if v > 0 { Some(v) } else { None });
    let total_retrans = u32_at!(100);

    // tcpi_options (u8 @5) — baseline bitflags; the ECN and SYN_DATA bits are
    // decoded into report-grade facts. Present whenever the 104-byte floor
    // above passed.
    let (ecn_negotiated, tfo_used) = {
        let (ecn, tfo) = decode_tcpi_options(buf[5]);
        (Some(ecn), Some(tfo))
    };

    // Linux ≥ 3.15
    let pacing_rate_bps = u64_at!(104).and_then(|v| if v > 0 { Some(v) } else { None });

    // Linux ≥ 4.1
    let bytes_acked = u64_at!(120);

    // Linux ≥ 4.2
    let segs_out = u32_at!(136).and_then(|v| if v > 0 { Some(v) } else { None });
    let segs_in = u32_at!(140).and_then(|v| if v > 0 { Some(v) } else { None });

    // Linux ≥ 4.6
    let notsent_bytes = u32_at!(144);
    let min_rtt_ms = u32_at!(148).and_then(|v| if v > 0 { Some(v as f64 / 1000.0) } else { None });

    // Linux ≥ 4.9
    let delivery_rate_bps = u64_at!(160).and_then(|v| if v > 0 { Some(v) } else { None });
    // tcpi_delivery_rate_app_limited lives in byte @7 (bitfield), which
    // pre-4.9 kernels report as zero padding — gate on the 4.9 struct length
    // (168 = end of tcpi_delivery_rate) so old kernels stay honest None
    // instead of a false Some(false).
    let app_limited = (n >= 168).then(|| decode_app_limited(buf[7]));

    // Linux ≥ 4.10 — the throughput-attribution triad. Zero is meaningful
    // ("never limited by X"), so values are kept as-is once the offset is
    // covered by optlen.
    let busy_time_us = u64_at!(168);
    let rwnd_limited_us = u64_at!(176);
    let sndbuf_limited_us = u64_at!(184);

    // Linux ≥ 4.18
    let delivered = u32_at!(192);
    let delivered_ce = u32_at!(196);

    // Linux ≥ 4.19
    let bytes_sent = u64_at!(200);
    let bytes_retrans = u64_at!(208);
    let dsack_dups = u32_at!(216);
    let reord_seen = u32_at!(220);

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
        busy_time_us,
        rwnd_limited_us,
        sndbuf_limited_us,
        bytes_acked,
        bytes_sent,
        bytes_retrans,
        delivered,
        delivered_ce,
        ecn_negotiated,
        tfo_used,
        app_limited,
        pacing_rate_bps,
        notsent_bytes,
        reord_seen,
        dsack_dups,
        rcv_rtt_ms,
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
        // B.2 Linux tcp_info additions: xnu's tcp_connection_info exposes no
        // busy/rwnd/sndbuf chronographs, delivery accounting, ECN/TFO options
        // byte in this layout slice, or pacing rate — honest None on macOS.
        ..Default::default()
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

    /// Field-for-field mirror of uapi `linux/tcp.h` `struct tcp_info` through
    /// the 4.19 fields we read (bitfield bytes flattened to explicit u8s —
    /// same layout). Every field is naturally aligned, so the offsets are
    /// identical on every target we build for; the test therefore pins the
    /// `u32_at!/u64_at!` offset table even when it runs on macOS/Windows CI.
    #[repr(C)]
    #[allow(dead_code)] // fields exist for offset_of! layout pinning only
    struct TcpInfoLayout {
        tcpi_state: u8,            // 0
        tcpi_ca_state: u8,         // 1
        tcpi_retransmits: u8,      // 2
        tcpi_probes: u8,           // 3
        tcpi_backoff: u8,          // 4
        tcpi_options: u8,          // 5
        tcpi_wscale_bits: u8,      // 6: snd_wscale:4, rcv_wscale:4
        tcpi_app_limited_bits: u8, // 7: delivery_rate_app_limited:1, …
        tcpi_rto: u32,             // 8
        tcpi_ato: u32,             // 12
        tcpi_snd_mss: u32,         // 16
        tcpi_rcv_mss: u32,         // 20
        tcpi_unacked: u32,         // 24
        tcpi_sacked: u32,          // 28
        tcpi_lost: u32,            // 32
        tcpi_retrans: u32,         // 36
        tcpi_fackets: u32,         // 40
        tcpi_last_data_sent: u32,  // 44
        tcpi_last_ack_sent: u32,   // 48
        tcpi_last_data_recv: u32,  // 52
        tcpi_last_ack_recv: u32,   // 56
        tcpi_pmtu: u32,            // 60
        tcpi_rcv_ssthresh: u32,    // 64
        tcpi_rtt: u32,             // 68
        tcpi_rttvar: u32,          // 72
        tcpi_snd_ssthresh: u32,    // 76
        tcpi_snd_cwnd: u32,        // 80
        tcpi_advmss: u32,          // 84
        tcpi_reordering: u32,      // 88
        tcpi_rcv_rtt: u32,         // 92
        tcpi_rcv_space: u32,       // 96
        tcpi_total_retrans: u32,   // 100
        tcpi_pacing_rate: u64,     // 104 (3.15)
        tcpi_max_pacing_rate: u64, // 112 (3.15)
        tcpi_bytes_acked: u64,     // 120 (4.1)
        tcpi_bytes_received: u64,  // 128 (4.1)
        tcpi_segs_out: u32,        // 136 (4.2)
        tcpi_segs_in: u32,         // 140 (4.2)
        tcpi_notsent_bytes: u32,   // 144 (4.6)
        tcpi_min_rtt: u32,         // 148 (4.6)
        tcpi_data_segs_in: u32,    // 152 (4.6)
        tcpi_data_segs_out: u32,   // 156 (4.6)
        tcpi_delivery_rate: u64,   // 160 (4.9)
        tcpi_busy_time: u64,       // 168 (4.10)
        tcpi_rwnd_limited: u64,    // 176 (4.10)
        tcpi_sndbuf_limited: u64,  // 184 (4.10)
        tcpi_delivered: u32,       // 192 (4.18)
        tcpi_delivered_ce: u32,    // 196 (4.18)
        tcpi_bytes_sent: u64,      // 200 (4.19)
        tcpi_bytes_retrans: u64,   // 208 (4.19)
        tcpi_dsack_dups: u32,      // 216 (4.19)
        tcpi_reord_seen: u32,      // 220 (4.19)
    }

    /// Pin every byte offset the Linux reader uses (mirror of the doc table).
    /// A silent uapi drift or a typo'd offset would read garbage into a
    /// report-grade field — this is the tripwire.
    #[test]
    fn linux_tcp_info_offset_table_matches_uapi() {
        use std::mem::offset_of;
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_retransmits), 2);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_options), 5);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_app_limited_bits), 7);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_rtt), 68);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_rttvar), 72);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_snd_ssthresh), 76);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_snd_cwnd), 80);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_rcv_rtt), 92);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_rcv_space), 96);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_total_retrans), 100);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_pacing_rate), 104);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_bytes_acked), 120);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_segs_out), 136);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_segs_in), 140);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_notsent_bytes), 144);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_min_rtt), 148);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_delivery_rate), 160);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_busy_time), 168);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_rwnd_limited), 176);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_sndbuf_limited), 184);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_delivered), 192);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_delivered_ce), 196);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_bytes_sent), 200);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_bytes_retrans), 208);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_dsack_dups), 216);
        assert_eq!(offset_of!(TcpInfoLayout, tcpi_reord_seen), 220);
        // Mirror ends right after the last field we read; the 4.19 struct
        // continues (rcv_ooopack @224 …) but nothing below 224 can move —
        // the uapi struct is append-only.
        assert_eq!(std::mem::size_of::<TcpInfoLayout>(), 224);
    }

    /// tcpi_options bitflag fixtures (uapi: ECN=8, SYN_DATA=32). A wrong bit
    /// value would publish false ECN/TFO facts on every Linux probe.
    #[test]
    fn tcpi_options_bits_decode_correctly() {
        assert_eq!(decode_tcpi_options(0), (false, false));
        assert_eq!(decode_tcpi_options(8), (true, false)); // TCPI_OPT_ECN
        assert_eq!(decode_tcpi_options(32), (false, true)); // TCPI_OPT_SYN_DATA
        assert_eq!(decode_tcpi_options(8 | 32), (true, true));
        // TIMESTAMPS|SACK|WSCALE (1|2|4) must not leak into either flag.
        assert_eq!(decode_tcpi_options(7), (false, false));
        // USEC_TS (64) / TFO_CHILD (128) must not either.
        assert_eq!(decode_tcpi_options(64 | 128), (false, false));
    }

    /// app_limited is bit 0 of byte @7 on little-endian (first-declared C
    /// bitfield = LSB); the neighboring fastopen_client_fail:2 bits must not
    /// bleed in.
    #[test]
    fn app_limited_bitfield_decodes_correctly() {
        #[cfg(target_endian = "little")]
        {
            assert!(decode_app_limited(0x01));
            assert!(decode_app_limited(0x07)); // app_limited + fastopen_fail bits
            assert!(!decode_app_limited(0x00));
            assert!(!decode_app_limited(0x06)); // fastopen_client_fail only
            assert!(!decode_app_limited(0x80));
        }
        #[cfg(target_endian = "big")]
        {
            assert!(decode_app_limited(0x80));
            assert!(!decode_app_limited(0x01));
        }
    }

    /// The B.2 fields must survive the SocketInfo → SocketStats conversion.
    #[test]
    fn socket_info_conversion_preserves_b2_fields() {
        let info = SocketInfo {
            busy_time_us: Some(1_000_000),
            rwnd_limited_us: Some(840_000),
            sndbuf_limited_us: Some(10_000),
            bytes_acked: Some(1_048_577),
            bytes_sent: Some(1_050_000),
            bytes_retrans: Some(2_800),
            delivered: Some(750),
            delivered_ce: Some(3),
            ecn_negotiated: Some(true),
            tfo_used: Some(false),
            app_limited: Some(true),
            pacing_rate_bps: Some(125_000_000),
            notsent_bytes: Some(0),
            reord_seen: Some(2),
            dsack_dups: Some(1),
            rcv_rtt_ms: Some(12.5),
            ..Default::default()
        };
        let stats: crate::metrics::SocketStats = info.into();
        assert_eq!(stats.busy_time_us, Some(1_000_000));
        assert_eq!(stats.rwnd_limited_us, Some(840_000));
        assert_eq!(stats.sndbuf_limited_us, Some(10_000));
        assert_eq!(stats.bytes_acked, Some(1_048_577));
        assert_eq!(stats.bytes_sent, Some(1_050_000));
        assert_eq!(stats.bytes_retrans, Some(2_800));
        assert_eq!(stats.delivered, Some(750));
        assert_eq!(stats.delivered_ce, Some(3));
        assert_eq!(stats.ecn_negotiated, Some(true));
        assert_eq!(stats.tfo_used, Some(false));
        assert_eq!(stats.app_limited, Some(true));
        assert_eq!(stats.pacing_rate_bps, Some(125_000_000));
        assert_eq!(stats.notsent_bytes, Some(0));
        assert_eq!(stats.reord_seen, Some(2));
        assert_eq!(stats.dsack_dups, Some(1));
        assert_eq!(stats.rcv_rtt_ms, Some(12.5));
    }

    /// UdpSocketDiag on a live socket: rcvbuf is observable on every Unix;
    /// local_drops is Some(0) on Linux (fresh socket, nothing dropped) and
    /// honest None on macOS.
    #[cfg(unix)]
    #[tokio::test]
    async fn udp_socket_diag_reports_rcvbuf_and_platform_honest_drops() {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let diag = UdpSocketDiag::from_socket(&sock);
        assert!(
            diag.so_rcvbuf_bytes.is_some_and(|b| b > 0),
            "SO_RCVBUF must be observable on Unix: {diag:?}"
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            diag.local_drops,
            Some(0),
            "fresh Linux socket must report zero drops via SO_MEMINFO"
        );
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            diag.local_drops, None,
            "non-Linux platforms have no per-socket drop counter — must be None, not 0"
        );
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
