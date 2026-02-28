/// Best-effort OS-level socket telemetry.
///
/// # What we can obtain without root / CAP_NET_ADMIN
///
/// | Platform | TCP_MAXSEG | TCP_INFO / TCP_CONNECTION_INFO | TCP_CONGESTION |
/// |----------|------------|-------------------------------|----------------|
/// | Linux    | ✓          | ✓ (all fields, no root)        | ✓              |
/// | macOS    | ✓          | ✓ (RTT, cwnd, retrans)         | ✓              |
/// | Windows  | ✗          | ✗                              | ✗              |
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
    /// Smoothed RTT in ms (TCP_INFO on Linux, TCP_CONNECTION_INFO on macOS).
    pub rtt_estimate_ms: Option<f64>,
    /// Segments currently queued for retransmit (tcpi_retransmits / txretransmitpackets).
    pub retransmits: Option<u32>,
    /// Lifetime retransmission count (Linux: tcpi_total_retrans).
    pub total_retrans: Option<u32>,
    /// Congestion window in segments (tcpi_snd_cwnd).
    pub snd_cwnd: Option<u32>,
    /// Slow-start threshold; None when the kernel sentinel (infinite) is set.
    pub snd_ssthresh: Option<u32>,
    /// RTT variance in ms (tcpi_rttvar).
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
        #[cfg(target_os = "linux")]
        {
            return linux_socket_info(stream);
        }
        #[cfg(target_os = "macos")]
        {
            return macos_socket_info(stream);
        }
        #[allow(unreachable_code)]
        Self::default()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Linux
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn linux_socket_info(stream: &TcpStream) -> SocketInfo {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
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

/// Manual layout for the macOS `tcp_connection_info` struct.
/// We only need the first 17 × 4-byte words (68 bytes).
#[cfg(target_os = "macos")]
#[repr(C)]
struct TcpConnectionInfoPartial {
    tcpi_state: u8,
    tcpi_snd_wscale: u8,
    tcpi_rcv_wscale: u8,
    _pad1: u8,
    tcpi_options: u32,
    tcpi_flags: u32,
    tcpi_rto: u32,
    tcpi_maxseg: u32,
    tcpi_snd_ssthresh: u32,
    tcpi_snd_cwnd: u32,
    tcpi_snd_wnd: u32,
    tcpi_snd_nxt: u32,
    tcpi_rcv_wnd: u32,
    tcpi_rttcur: u32,
    tcpi_srtt: u32,                // smoothed RTT, microseconds
    tcpi_rttvar: u32,              // RTT variance, microseconds
    tcpi_txretransmitpackets: u32, // retransmit packet count
}

#[cfg(target_os = "macos")]
const TCP_CONNECTION_INFO_OPT: libc::c_int = 0x24;

/// TCP_CONGESTION on macOS (IPPROTO_TCP option 0x20).
#[cfg(target_os = "macos")]
const TCP_CONGESTION_MACOS: libc::c_int = 0x20;

#[cfg(target_os = "macos")]
fn macos_socket_info(stream: &TcpStream) -> SocketInfo {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();

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

    let congestion_algorithm = unsafe {
        let mut buf = [0u8; 32];
        let mut len = 32u32;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            TCP_CONGESTION_MACOS,
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
    };

    let (rtt_ms, rtt_var_ms, snd_cwnd, snd_ssthresh, retransmits) = unsafe {
        let mut info: TcpConnectionInfoPartial = std::mem::zeroed();
        let mut len = std::mem::size_of::<TcpConnectionInfoPartial>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            TCP_CONNECTION_INFO_OPT,
            &mut info as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if ret == 0 {
            let rtt = if info.tcpi_srtt > 0 {
                Some(info.tcpi_srtt as f64 / 1000.0)
            } else {
                None
            };
            let rttvar = if info.tcpi_rttvar > 0 {
                Some(info.tcpi_rttvar as f64 / 1000.0)
            } else {
                None
            };
            let cwnd = if info.tcpi_snd_cwnd > 0 {
                Some(info.tcpi_snd_cwnd)
            } else {
                None
            };
            let ssthresh = {
                let s = info.tcpi_snd_ssthresh;
                if s > 0 && s < 0x7fff_ffff {
                    Some(s)
                } else {
                    None
                }
            };
            let retrans = if info.tcpi_txretransmitpackets > 0 {
                Some(info.tcpi_txretransmitpackets)
            } else {
                None
            };
            (rtt, rttvar, cwnd, ssthresh, retrans)
        } else {
            (None, None, None, None, None)
        }
    };

    SocketInfo {
        mss_bytes: mss,
        rtt_estimate_ms: rtt_ms,
        retransmits,
        total_retrans: None,
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
}
