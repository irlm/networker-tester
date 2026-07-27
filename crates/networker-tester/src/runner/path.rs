//! Hop-discovery probe (`path` mode) — traceroute without raw sockets.
//!
//! Sends UDP probes to high ports with incrementing TTL (`IP_TTL` /
//! `IPV6_UNICAST_HOPS`) and reads the resulting ICMP errors WITHOUT raw
//! sockets:
//!
//! - **Linux** (`method = "udp-ttl/ip-recverr"`): `IP_RECVERR` queues each
//!   ICMP time-exceeded / port-unreachable on the UDP socket's error queue;
//!   `recvmsg(MSG_ERRQUEUE)` yields the error type AND the offending router's
//!   address — a full per-hop trace, fully unprivileged.
//! - **macOS / Windows** (`method = "udp-ttl-estimate"`): there is no
//!   unprivileged way to see WHICH router sent a time-exceeded (no
//!   `IP_RECVERR`; ICMP errors from intermediate hops are not delivered to
//!   UDP sockets at all). The probe degrades honestly: it scans TTLs upward
//!   on a connected UDP socket, on which a destination-generated ICMP
//!   port-unreachable surfaces as `ECONNREFUSED`/`ECONNRESET` — giving the
//!   hop COUNT (first TTL that reaches the destination) and final-hop
//!   reachability, with `hops: []`. Hop addresses are NEVER fabricated.
//!
//! Firewalls that drop the probes (or rate-limit ICMP) show up as silent
//! hops / an unreached destination — reported as such, not invented.

use crate::metrics::{ErrorCategory, ErrorRecord, PathHop, PathResult, Protocol, RequestAttempt};
use chrono::Utc;
use std::net::IpAddr;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PathProbeConfig {
    /// Hostname or IP literal to trace toward.
    pub target_host: String,
    /// Highest TTL probed (default 30 — the classic traceroute limit).
    pub max_ttl: u32,
    /// Per-hop wait for an ICMP error (ms).
    pub per_hop_timeout_ms: u64,
    /// First destination UDP port; hop N probes `base_port + N - 1`
    /// (traceroute's classic 33434+ range, chosen to be closed).
    pub base_port: u16,
}

pub const DEFAULT_PATH_MAX_TTL: u32 = 30;
pub const DEFAULT_PATH_HOP_TIMEOUT_MS: u64 = 1_000;
pub const DEFAULT_PATH_BASE_PORT: u16 = 33_434;

impl Default for PathProbeConfig {
    fn default() -> Self {
        Self {
            target_host: "127.0.0.1".to_string(),
            max_ttl: DEFAULT_PATH_MAX_TTL,
            per_hop_timeout_ms: DEFAULT_PATH_HOP_TIMEOUT_MS,
            base_port: DEFAULT_PATH_BASE_PORT,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_path_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &PathProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let addr: IpAddr = match resolve_host(&cfg.target_host).await {
        Ok(a) => a,
        Err(msg) => {
            return path_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Dns,
                msg,
            )
        }
    };

    let max_ttl = cfg.max_ttl.clamp(1, 64);
    let per_hop_timeout_ms = cfg.per_hop_timeout_ms.max(1);
    let base_port = cfg.base_port;
    let outcome = tokio::task::spawn_blocking(move || {
        platform::trace_blocking(addr, max_ttl, per_hop_timeout_ms, base_port)
    })
    .await;

    let trace = match outcome {
        Ok(Ok(t)) => t,
        Ok(Err(msg)) => {
            return path_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Other,
                msg,
            )
        }
        Err(e) => {
            return path_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Other,
                format!("path worker task failed: {e}"),
            )
        }
    };

    let result = PathResult {
        remote_addr: addr.to_string(),
        hops: trace.hops,
        hop_count: trace.hop_count,
        destination_reached: trace.destination_reached,
        destination_rtt_ms: trace.destination_rtt_ms,
        method: trace.method,
        max_ttl,
        started_at,
    };

    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Path,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        // The probe ran and produced an honest observation either way; an
        // unreached destination (firewalled path) is a finding, not a probe
        // failure — but a trace with NO information at all is a failure.
        success: result.destination_reached || !result.hops.is_empty(),
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: if result.destination_reached || !result.hops.is_empty() {
            None
        } else {
            Some(ErrorRecord {
                category: ErrorCategory::Udp,
                message: format!(
                    "No ICMP responses for any TTL 1..={max_ttl} and the destination never \
                     answered — path blocked or ICMP filtered ({})",
                    result.method
                ),
                detail: None,
                occurred_at: Utc::now(),
            })
        },
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
        ping: None,
        path: Some(result),
        dualstack: None,
        websocket: None,
        pmtud: None,
    }
}

async fn resolve_host(host: &str) -> Result<IpAddr, String> {
    // url::Url brackets IPv6 literals ("[::1]") — strip before parsing.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }
    match tokio::net::lookup_host((host, 0u16)).await {
        Ok(mut addrs) => addrs
            .next()
            .map(|a| a.ip())
            .ok_or_else(|| format!("No address resolved for {host}")),
        Err(e) => Err(format!("DNS error for {host}: {e}")),
    }
}

fn path_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Path,
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
            category,
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
        rpm: None,
        ping: None,
        path: None,
        dualstack: None,
        websocket: None,
        pmtud: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform implementations
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) struct TraceOutcome {
    pub hops: Vec<PathHop>,
    pub hop_count: Option<u32>,
    pub destination_reached: bool,
    pub destination_rtt_ms: Option<f64>,
    pub method: String,
}

#[cfg(target_os = "linux")]
pub(crate) use linux_impl as platform;
#[cfg(not(target_os = "linux"))]
pub(crate) use portable_impl as platform;

/// Linux: full per-hop trace via `IP_RECVERR` + `recvmsg(MSG_ERRQUEUE)`.
#[cfg(target_os = "linux")]
pub(crate) mod linux_impl {
    use super::TraceOutcome;
    use crate::metrics::PathHop;
    use std::io;
    use std::mem;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    const METHOD: &str = "udp-ttl/ip-recverr";

    /// What one error-queue entry told us.
    enum IcmpEvent {
        /// Router decremented TTL to zero — one hop discovered.
        TimeExceeded { offender: Option<IpAddr> },
        /// Destination (or loopback fast-path) refused the probe's port —
        /// the packet REACHED the destination.
        DestinationReached,
        /// Some other unreachable (host/net/admin-prohibited) — path ends
        /// before the destination.
        Unreachable { offender: Option<IpAddr> },
    }

    pub fn trace_blocking(
        addr: IpAddr,
        max_ttl: u32,
        per_hop_timeout_ms: u64,
        base_port: u16,
    ) -> Result<TraceOutcome, String> {
        let bind: SocketAddr = match addr {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(bind).map_err(|e| format!("UDP bind failed: {e}"))?;
        enable_recverr(&socket, &addr).map_err(|e| format!("IP_RECVERR setsockopt failed: {e}"))?;

        let mut hops: Vec<PathHop> = Vec::new();
        let mut destination_reached = false;
        let mut destination_rtt_ms = None;
        let mut hop_count = None;

        for ttl in 1..=max_ttl {
            set_ttl(&socket, &addr, ttl).map_err(|e| format!("set TTL={ttl} failed: {e}"))?;
            let port = base_port.wrapping_add((ttl - 1) as u16);
            let dest = SocketAddr::new(addr, port);
            let sent_at = Instant::now();
            if let Err(e) = socket.send_to(&[0u8; 8], dest) {
                // A synchronous refusal (previous hop's queued error) is
                // handled below via the error queue; other send errors on a
                // specific TTL count as a silent hop.
                tracing::debug!("path probe ttl={ttl} send error: {e}");
            }

            let deadline = sent_at + Duration::from_millis(per_hop_timeout_ms);
            match wait_icmp_event(&socket, deadline) {
                Some(IcmpEvent::TimeExceeded { offender }) => {
                    hops.push(PathHop {
                        index: ttl,
                        addr: offender.map(|a| a.to_string()),
                        rtt_ms: Some(sent_at.elapsed().as_secs_f64() * 1000.0),
                    });
                }
                Some(IcmpEvent::DestinationReached) => {
                    let rtt = sent_at.elapsed().as_secs_f64() * 1000.0;
                    hops.push(PathHop {
                        index: ttl,
                        addr: Some(addr.to_string()),
                        rtt_ms: Some(rtt),
                    });
                    destination_reached = true;
                    destination_rtt_ms = Some(rtt);
                    hop_count = Some(ttl);
                    break;
                }
                Some(IcmpEvent::Unreachable { offender }) => {
                    // Path terminates before the destination (host/net
                    // unreachable, admin prohibited). Record who said so and
                    // stop — probing higher TTLs cannot get further.
                    hops.push(PathHop {
                        index: ttl,
                        addr: offender.map(|a| a.to_string()),
                        rtt_ms: Some(sent_at.elapsed().as_secs_f64() * 1000.0),
                    });
                    break;
                }
                None => {
                    // Silent hop (rate-limited/filtered ICMP) — an honest `*`.
                    hops.push(PathHop {
                        index: ttl,
                        addr: None,
                        rtt_ms: None,
                    });
                }
            }
        }

        // A trace where NOTHING answered carries no path information — do
        // not report 30 silent `*` rows as if they were hops.
        if !destination_reached && hops.iter().all(|h| h.addr.is_none()) {
            hops.clear();
        }

        Ok(TraceOutcome {
            hops,
            hop_count,
            destination_reached,
            destination_rtt_ms,
            method: METHOD.to_string(),
        })
    }

    fn enable_recverr(socket: &UdpSocket, addr: &IpAddr) -> io::Result<()> {
        let one: libc::c_int = 1;
        let (level, opt) = match addr {
            IpAddr::V4(_) => (libc::IPPROTO_IP, libc::IP_RECVERR),
            IpAddr::V6(_) => (libc::IPPROTO_IPV6, libc::IPV6_RECVERR),
        };
        // SAFETY: valid fd, c_int option value.
        let rc = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                level,
                opt,
                &one as *const _ as *const libc::c_void,
                mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn set_ttl(socket: &UdpSocket, addr: &IpAddr, ttl: u32) -> io::Result<()> {
        match addr {
            IpAddr::V4(_) => socket.set_ttl(ttl),
            IpAddr::V6(_) => {
                let hops: libc::c_int = ttl as libc::c_int;
                // SAFETY: valid fd, c_int option value.
                let rc = unsafe {
                    libc::setsockopt(
                        socket.as_raw_fd(),
                        libc::IPPROTO_IPV6,
                        libc::IPV6_UNICAST_HOPS,
                        &hops as *const _ as *const libc::c_void,
                        mem::size_of::<libc::c_int>() as libc::socklen_t,
                    )
                };
                if rc < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Poll the socket until `deadline` for an error-queue entry and decode it.
    fn wait_icmp_event(socket: &UdpSocket, deadline: Instant) -> Option<IcmpEvent> {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let wait_ms = (deadline - now).as_millis().min(100) as libc::c_int;
            let mut pfd = libc::pollfd {
                fd: socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: single valid pollfd. POLLERR is always reported even
            // when not requested — it signals error-queue readiness.
            let rc = unsafe { libc::poll(&mut pfd, 1, wait_ms) };
            if rc < 0 {
                return None;
            }
            if let Some(ev) = drain_errqueue(socket) {
                return Some(ev);
            }
            // rc == 0 (timeout slice) or spurious wake: loop re-checks deadline.
        }
    }

    /// Non-blocking read of one error-queue entry.
    fn drain_errqueue(socket: &UdpSocket) -> Option<IcmpEvent> {
        let mut data = [0u8; 512];
        let mut iov = libc::iovec {
            iov_base: data.as_mut_ptr() as *mut libc::c_void,
            iov_len: data.len(),
        };
        let mut name = [0u8; 128]; // original destination sockaddr
        let mut ctrl = [0u8; 512];
        // SAFETY: zeroed msghdr pointed at live buffers.
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_name = name.as_mut_ptr() as *mut libc::c_void;
        msg.msg_namelen = name.len() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = ctrl.as_mut_ptr() as *mut libc::c_void;
        // `as _`: msg_controllen is usize on glibc but u32 (socklen_t) on musl —
        // the release binaries build for x86_64-unknown-linux-musl (v0.28.76's
        // release broke on exactly this line).
        msg.msg_controllen = ctrl.len() as _;

        // SAFETY: MSG_ERRQUEUE recvmsg never blocks; returns -1/EAGAIN when
        // the queue is empty.
        let n = unsafe {
            libc::recvmsg(
                socket.as_raw_fd(),
                &mut msg,
                libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT,
            )
        };
        if n < 0 {
            return None;
        }

        // SAFETY: cmsg walk over the kernel-filled control buffer.
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                let c = &*cmsg;
                let is_err = (c.cmsg_level == libc::IPPROTO_IP && c.cmsg_type == libc::IP_RECVERR)
                    || (c.cmsg_level == libc::IPPROTO_IPV6 && c.cmsg_type == libc::IPV6_RECVERR);
                if is_err {
                    let ee = &*(libc::CMSG_DATA(cmsg) as *const libc::sock_extended_err);
                    // Offender sockaddr immediately follows sock_extended_err
                    // (the SO_EE_OFFENDER(ee) macro in C).
                    let offender_ptr =
                        (ee as *const libc::sock_extended_err).add(1) as *const libc::sockaddr;
                    let offender = decode_sockaddr(offender_ptr);
                    return Some(classify(ee, offender));
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        None
    }

    fn classify(ee: &libc::sock_extended_err, offender: Option<IpAddr>) -> IcmpEvent {
        const ICMP_TIME_EXCEEDED: u8 = 11;
        const ICMP_DEST_UNREACH: u8 = 3;
        const ICMP_PORT_UNREACH_CODE: u8 = 3;
        const ICMPV6_TIME_EXCEEDED: u8 = 3;
        const ICMPV6_DEST_UNREACH: u8 = 1;
        const ICMPV6_PORT_UNREACH_CODE: u8 = 4;

        match u32::from(ee.ee_origin) {
            o if o == libc::SO_EE_ORIGIN_ICMP as u32 => match (ee.ee_type, ee.ee_code) {
                (ICMP_TIME_EXCEEDED, _) => IcmpEvent::TimeExceeded { offender },
                (ICMP_DEST_UNREACH, ICMP_PORT_UNREACH_CODE) => IcmpEvent::DestinationReached,
                _ => IcmpEvent::Unreachable { offender },
            },
            o if o == libc::SO_EE_ORIGIN_ICMP6 as u32 => match (ee.ee_type, ee.ee_code) {
                (ICMPV6_TIME_EXCEEDED, _) => IcmpEvent::TimeExceeded { offender },
                (ICMPV6_DEST_UNREACH, ICMPV6_PORT_UNREACH_CODE) => IcmpEvent::DestinationReached,
                _ => IcmpEvent::Unreachable { offender },
            },
            // Local origin: loopback / same-host fast path reports
            // ECONNREFUSED without a wire ICMP.
            _ if ee.ee_errno == libc::ECONNREFUSED as u32 => IcmpEvent::DestinationReached,
            _ => IcmpEvent::Unreachable { offender: None },
        }
    }

    /// SAFETY: `ptr` must point into the cmsg buffer right after a
    /// sock_extended_err; family tag is validated before reading.
    unsafe fn decode_sockaddr(ptr: *const libc::sockaddr) -> Option<IpAddr> {
        if ptr.is_null() {
            return None;
        }
        match u32::from((*ptr).sa_family) {
            f if f == libc::AF_INET as u32 => {
                let sa = &*(ptr as *const libc::sockaddr_in);
                // s_addr is stored in network byte order — its in-memory
                // bytes ARE the address octets.
                Some(IpAddr::V4(Ipv4Addr::from(sa.sin_addr.s_addr.to_ne_bytes())))
            }
            f if f == libc::AF_INET6 as u32 => {
                let sa = &*(ptr as *const libc::sockaddr_in6);
                Some(IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

/// macOS / Windows: honest degradation — TTL scan on a connected UDP socket.
/// A destination ICMP port-unreachable surfaces as ConnectionRefused (macOS)
/// or ConnectionReset (Windows) on the connected socket; intermediate-hop
/// time-exceeded errors are NOT observable unprivileged, so `hops` stays
/// empty and only the hop-count estimate + reachability are reported.
#[cfg(not(target_os = "linux"))]
pub(crate) mod portable_impl {
    use super::TraceOutcome;
    use std::io::ErrorKind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
    use std::time::{Duration, Instant};

    const METHOD: &str = "udp-ttl-estimate";

    pub fn trace_blocking(
        addr: IpAddr,
        max_ttl: u32,
        per_hop_timeout_ms: u64,
        base_port: u16,
    ) -> Result<TraceOutcome, String> {
        let bind: SocketAddr = match addr {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };

        let mut destination_reached = false;
        let mut destination_rtt_ms = None;
        let mut hop_count = None;

        for ttl in 1..=max_ttl {
            // Fresh socket per TTL so a queued ICMP error from a previous
            // probe cannot be misattributed to this one.
            let socket = UdpSocket::bind(bind).map_err(|e| format!("UDP bind failed: {e}"))?;
            set_hop_limit(&socket, &addr, ttl)?;
            let port = base_port.wrapping_add((ttl - 1) as u16);
            socket
                .connect(SocketAddr::new(addr, port))
                .map_err(|e| format!("UDP connect failed: {e}"))?;
            socket
                .set_read_timeout(Some(Duration::from_millis(per_hop_timeout_ms)))
                .map_err(|e| format!("set_read_timeout failed: {e}"))?;

            let sent_at = Instant::now();
            if socket.send(&[0u8; 8]).is_err() {
                continue; // transient send failure — try the next TTL
            }

            // recv surfaces the async ICMP error on a connected socket;
            // an actual datagram (something listening on the trace port)
            // also proves the destination was reached.
            let deadline = sent_at + Duration::from_millis(per_hop_timeout_ms);
            let mut buf = [0u8; 64];
            loop {
                match socket.recv(&mut buf) {
                    Ok(_) => {
                        destination_reached = true;
                    }
                    Err(e)
                        if e.kind() == ErrorKind::ConnectionRefused
                            || e.kind() == ErrorKind::ConnectionReset =>
                    {
                        destination_reached = true;
                    }
                    Err(e)
                        if (e.kind() == ErrorKind::WouldBlock
                            || e.kind() == ErrorKind::TimedOut)
                            && Instant::now() < deadline =>
                    {
                        continue;
                    }
                    Err(_) => {}
                }
                break;
            }
            if destination_reached {
                destination_rtt_ms = Some(sent_at.elapsed().as_secs_f64() * 1000.0);
                hop_count = Some(ttl);
                break;
            }
        }

        Ok(TraceOutcome {
            hops: Vec::new(), // never fabricated — see module docs
            hop_count,
            destination_reached,
            destination_rtt_ms,
            method: METHOD.to_string(),
        })
    }

    #[cfg(unix)]
    fn set_hop_limit(socket: &UdpSocket, addr: &IpAddr, ttl: u32) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        match addr {
            IpAddr::V4(_) => socket
                .set_ttl(ttl)
                .map_err(|e| format!("set TTL failed: {e}")),
            IpAddr::V6(_) => {
                let hops: libc::c_int = ttl as libc::c_int;
                // SAFETY: valid fd, c_int option value.
                let rc = unsafe {
                    libc::setsockopt(
                        socket.as_raw_fd(),
                        libc::IPPROTO_IPV6,
                        libc::IPV6_UNICAST_HOPS,
                        &hops as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                    )
                };
                if rc < 0 {
                    Err(format!(
                        "IPV6_UNICAST_HOPS failed: {}",
                        std::io::Error::last_os_error()
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }

    #[cfg(not(unix))]
    fn set_hop_limit(socket: &UdpSocket, addr: &IpAddr, ttl: u32) -> Result<(), String> {
        match addr {
            IpAddr::V4(_) => socket
                .set_ttl(ttl)
                .map_err(|e| format!("set TTL failed: {e}")),
            IpAddr::V6(_) => Err(
                "path mode cannot set the IPv6 hop limit unprivileged on Windows — \
                 use an IPv4 target"
                    .to_string(),
            ),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Loopback: the destination answers at TTL 1 (Linux full trace) or the
    /// estimate scan finds hop_count = 1 (degraded platforms). Firewalled
    /// environments that swallow even loopback ICMP get a graceful skip.
    #[tokio::test]
    async fn path_loopback_reaches_destination() {
        let cfg = PathProbeConfig {
            target_host: "127.0.0.1".into(),
            max_ttl: 4,
            per_hop_timeout_ms: 1000,
            base_port: DEFAULT_PATH_BASE_PORT,
        };
        let attempt = run_path_probe(Uuid::new_v4(), 0, &cfg).await;
        let Some(p) = attempt.path.as_ref() else {
            panic!("path result missing: {:?}", attempt.error);
        };
        if !p.destination_reached {
            eprintln!(
                "SKIP path_loopback_reaches_destination: loopback ICMP not observable here \
                 (method={}, hops={:?})",
                p.method, p.hops
            );
            return;
        }
        assert!(attempt.success);
        assert_eq!(attempt.protocol, Protocol::Path);
        assert_eq!(p.hop_count, Some(1), "loopback is one hop");
        assert!(p.destination_rtt_ms.unwrap_or(0.0) > 0.0);
        assert!(!p.method.is_empty());
        // Degraded platforms must not fabricate hop addresses.
        if p.method == "udp-ttl-estimate" {
            assert!(p.hops.is_empty(), "estimate mode must not invent hops");
        } else {
            assert_eq!(p.hops.len(), 1);
            assert_eq!(p.hops[0].addr.as_deref(), Some("127.0.0.1"));
        }
    }

    #[tokio::test]
    async fn path_unresolvable_host_is_dns_error() {
        let cfg = PathProbeConfig {
            target_host: "this-hostname-does-not-exist.invalid".into(),
            ..Default::default()
        };
        let attempt = run_path_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!attempt.success);
        assert!(attempt.path.is_none());
        assert_eq!(
            attempt.error.expect("error must be set").category,
            ErrorCategory::Dns
        );
    }
}
