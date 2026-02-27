/// Best-effort OS-level socket telemetry.
///
/// # What we can obtain without root / CAP_NET_ADMIN
///
/// | Platform | TCP_MAXSEG (MSS) | TCP_INFO (RTT, cwnd, retrans) | TCP_CONNECTION_INFO |
/// |----------|-----------------|-------------------------------|---------------------|
/// | Linux    | ✓ getsockopt    | ✓ all fields without root      | n/a                 |
/// | macOS    | ✓ getsockopt    | n/a                            | ✓ RTT + cwnd       |
/// | Windows  | ✗               | ✗                              | ✗                   |
use tokio::net::TcpStream;

#[derive(Debug, Clone, Default)]
pub struct SocketInfo {
    /// Maximum Segment Size in bytes (from TCP_MAXSEG). Best-effort.
    pub mss_bytes: Option<u32>,
    /// Smoothed RTT in milliseconds (from TCP_INFO on Linux, TCP_CONNECTION_INFO on macOS).
    pub rtt_estimate_ms: Option<f64>,
    /// Segments currently queued for retransmit (tcpi_retransmits / txretransmitpackets).
    pub retransmits: Option<u32>,
    /// Lifetime retransmission count (Linux: tcpi_total_retrans).
    pub total_retrans: Option<u32>,
    /// Congestion window in segments (tcpi_snd_cwnd).
    pub snd_cwnd: Option<u32>,
    /// Slow-start threshold; None when set to the kernel sentinel (infinite).
    pub snd_ssthresh: Option<u32>,
    /// RTT variance in milliseconds (tcpi_rttvar).
    pub rtt_variance_ms: Option<f64>,
    /// Receiver advertised window in bytes (tcpi_rcv_space). Linux only.
    pub rcv_space: Option<u32>,
    /// Segments sent (Linux ≥ 4.6: tcpi_segs_out). Not yet read.
    pub segs_out: Option<u32>,
    /// Segments received (Linux ≥ 4.6: tcpi_segs_in). Not yet read.
    pub segs_in: Option<u32>,
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
    let mss = get_tcp_maxseg(fd);

    // Read the full tcp_info struct for all extended stats.
    let (rtt_ms, retransmits, total_retrans, snd_cwnd, snd_ssthresh, rtt_var_ms, rcv_space) = unsafe {
        let mut info: libc::tcp_info = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::tcp_info>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            &mut info as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if ret == 0 {
            let rtt = if info.tcpi_rtt > 0 {
                Some(info.tcpi_rtt as f64 / 1000.0)
            } else {
                None
            };
            // tcpi_retransmits is u8 – segments in-flight and retransmitted
            let retrans = if info.tcpi_retransmits > 0 {
                Some(info.tcpi_retransmits as u32)
            } else {
                None
            };
            let total_retrans = Some(info.tcpi_total_retrans);
            let cwnd = if info.tcpi_snd_cwnd > 0 {
                Some(info.tcpi_snd_cwnd)
            } else {
                None
            };
            // 0x7fffffff is TCP_INFINITE_SSTHRESH sentinel
            let ssthresh = {
                let s = info.tcpi_snd_ssthresh;
                if s < 0x7fff_ffff {
                    Some(s)
                } else {
                    None
                }
            };
            let rttvar = if info.tcpi_rttvar > 0 {
                Some(info.tcpi_rttvar as f64 / 1000.0)
            } else {
                None
            };
            let rcv = if info.tcpi_rcv_space > 0 {
                Some(info.tcpi_rcv_space)
            } else {
                None
            };
            (rtt, retrans, total_retrans, cwnd, ssthresh, rttvar, rcv)
        } else {
            (None, None, None, None, None, None, None)
        }
    };

    SocketInfo {
        mss_bytes: mss,
        rtt_estimate_ms: rtt_ms,
        retransmits,
        total_retrans,
        snd_cwnd,
        snd_ssthresh,
        rtt_variance_ms: rtt_var_ms,
        rcv_space,
        // segs_out / segs_in require Linux ≥ 4.6; left as None pending kernel/libc guard
        segs_out: None,
        segs_in: None,
    }
}

#[cfg(target_os = "linux")]
fn get_tcp_maxseg(fd: std::os::unix::io::RawFd) -> Option<u32> {
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

// ─────────────────────────────────────────────────────────────────────────────
// macOS
// ─────────────────────────────────────────────────────────────────────────────

/// Manual layout for the macOS `tcp_connection_info` struct.
/// We only need the first 17 × 4-byte words (68 bytes); getsockopt fills up to
/// `size_of::<TcpConnectionInfoPartial>()` bytes so the layout must be exact.
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
const TCP_CONNECTION_INFO_OPT: libc::c_int = 0x24; // IPPROTO_TCP, TCP_CONNECTION_INFO

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

    // TCP_CONNECTION_INFO gives RTT, cwnd, and retransmit count without root.
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
                Some(info.tcpi_srtt as f64 / 1000.0) // µs → ms
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
        total_retrans: None, // not available via TCP_CONNECTION_INFO
        snd_cwnd,
        snd_ssthresh,
        rtt_variance_ms: rtt_var_ms,
        rcv_space: None,
        segs_out: None,
        segs_in: None,
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
    }
}
