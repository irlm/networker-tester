/// Best-effort OS-level socket telemetry.
///
/// # What we can obtain without root / CAP_NET_ADMIN
///
/// | Platform | TCP_MAXSEG (MSS) | TCP_INFO (RTT) | cwnd |
/// |----------|-----------------|----------------|------|
/// | Linux    | ✓ getsockopt    | ✓ tcpi_rtt µs  | ✗    |
/// | macOS    | ✓ getsockopt    | ✗              | ✗    |
/// | Windows  | ✗               | ✗              | ✗    |
///
/// cwnd, retransmit counters, and CC state are only visible via BPF/netlink
/// (Linux) or ETW (Windows) which require elevated privileges.
use tokio::net::TcpStream;

#[derive(Debug, Clone, Default)]
pub struct SocketInfo {
    /// Maximum Segment Size in bytes (from TCP_MAXSEG). Best-effort.
    pub mss_bytes: Option<u32>,
    /// Smoothed RTT in milliseconds (from TCP_INFO tcpi_rtt). Linux only.
    pub rtt_estimate_ms: Option<f64>,
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
    let rtt = get_tcp_rtt_linux(fd);
    SocketInfo { mss_bytes: mss, rtt_estimate_ms: rtt }
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
        if ret == 0 && val > 0 { Some(val as u32) } else { None }
    }
}

/// Reads tcpi_rtt from the kernel's tcp_info struct (microseconds → ms).
#[cfg(target_os = "linux")]
fn get_tcp_rtt_linux(fd: std::os::unix::io::RawFd) -> Option<f64> {
    // tcp_info is a well-known Linux kernel struct; libc exposes it.
    unsafe {
        let mut info: libc::tcp_info = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::tcp_info>() as libc::socklen_t;
        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            &mut info as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if ret == 0 && info.tcpi_rtt > 0 {
            Some(info.tcpi_rtt as f64 / 1000.0) // µs → ms
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// macOS
// ─────────────────────────────────────────────────────────────────────────────

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
        if ret == 0 && val > 0 { Some(val as u32) } else { None }
    };
    SocketInfo { mss_bytes: mss, rtt_estimate_ms: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_info_default_is_none() {
        let info = SocketInfo::default();
        assert!(info.mss_bytes.is_none());
        assert!(info.rtt_estimate_ms.is_none());
    }
}
