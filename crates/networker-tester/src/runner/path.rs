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
//!
//! # Flow consistency (Paris-traceroute semantics)
//!
//! Every probe uses a CONSTANT 5-tuple: one socket (fixed source port on
//! Linux) and a single destination port for all TTLs. Classic traceroute
//! varied the destination port per TTL to demultiplex responses, which makes
//! per-flow ECMP load balancers hash consecutive TTLs onto different physical
//! paths — the false-link/phantom-loop artifact Paris traceroute (Augustin et
//! al., IMC 2006) was invented to eliminate. Our matching is temporal and
//! sequential (one probe in flight at a time), so the port variation bought
//! nothing; holding the 5-tuple constant keeps every hop on one flow.
//! Honest caveat: per-PACKET load balancers can still scramble any traceroute.

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
    /// Destination UDP port used for EVERY probe (traceroute's classic 33434,
    /// chosen to be closed). Constant across TTLs so all hops are measured on
    /// one ECMP flow — see the module docs on Paris-traceroute semantics.
    pub base_port: u16,
    /// TCP port used to classify a zero-information trace: if NOTHING
    /// answered but this port connects, the environment (cloud SNAT, e.g.
    /// Azure SLB — proven v0.28.120) is eating the ICMP errors and the
    /// verdict is "environment-blocked", not "path down". `None` skips the
    /// check.
    pub verify_tcp_port: Option<u16>,
    /// Wall-clock budget for the WHOLE trace (ms); `0` = unbounded.
    ///
    /// Without this the probe always cost `max_ttl * per_hop_timeout_ms`: a
    /// filtered path (no ICMP at all) burned 30 x 1 s = 30 s per attempt while
    /// the caller had asked for `--timeout 10`, which is what the production
    /// sweep saw as a "hang" on the second iteration (measured in the Docker lab
    /// with inbound ICMP dropped: 61 s for `--runs 2`). The dispatcher passes
    /// the run's per-attempt timeout; the scan stops at the budget and reports
    /// the hops it did discover.
    pub total_budget_ms: u64,
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
            verify_tcp_port: Some(443),
            total_budget_ms: 0, // unbounded unless the caller sets one
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
    // spawn_blocking tasks cannot be cancelled, so the trace has to bound
    // ITSELF: an outer timeout would return while the thread kept probing.
    let total_budget_ms = cfg.total_budget_ms;
    let outcome = tokio::task::spawn_blocking(move || {
        platform::trace_blocking(
            addr,
            max_ttl,
            per_hop_timeout_ms,
            base_port,
            total_budget_ms,
        )
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

    // How far the scan actually got: with a budget, "no ICMP for TTL 1..=30" is
    // a lie when only 10 TTLs were probed.
    let scanned = if trace.budget_truncated {
        format!(
            "1..={probed} (scan stopped at the {budget}ms budget, before TTL {max_ttl})",
            probed = trace.probed_ttls,
            budget = cfg.total_budget_ms,
        )
    } else {
        format!("1..={max_ttl}")
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

    // A trace with NO information at all is ambiguous: the path could be
    // down, or the runner's environment could be eating every ICMP error
    // (cloud SNAT does — Azure SLB proven v0.28.120). One TCP connect to
    // the target settles it and turns "failed" into an honest,
    // environment-classified verdict with evidence.
    let has_info = result.destination_reached || !result.hops.is_empty();
    let error = if has_info {
        None
    } else {
        let (category, message) = match cfg.verify_tcp_port {
            Some(port) => match tcp_reachability_ms(addr, port, 5_000).await {
                Some(rtt_ms) => (
                    ErrorCategory::Config,
                    format!(
                        "Hop discovery blocked by this runner's environment, not the path: \
                         no ICMP responses for any TTL {scanned} ({method}), but the \
                         destination IS reachable — TCP connect to port {port} in {rtt_ms:.1}ms. \
                         Cloud SNAT layers (e.g. Azure SLB) drop ICMP; run path from a \
                         runner with a direct public IP to see hops.",
                        method = result.method,
                    ),
                ),
                None => (
                    ErrorCategory::Udp,
                    format!(
                        "No ICMP responses for any TTL {scanned} and the destination \
                         never answered (also unreachable on TCP port {port}) — path \
                         blocked or target down ({})",
                        result.method
                    ),
                ),
            },
            None => (
                ErrorCategory::Udp,
                format!(
                    "No ICMP responses for any TTL {scanned} and the destination never \
                     answered — path blocked or ICMP filtered ({})",
                    result.method
                ),
            ),
        };
        Some(ErrorRecord {
            category,
            message,
            detail: None,
            occurred_at: Utc::now(),
        })
    };

    RequestAttempt {
        target_url: None,
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
        success: has_info,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error,
        retry_count: 0,
        sample_index: 0,
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
        responsiveness: None,
        stamp: None,
        mthroughput: None,
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

/// One TCP connect to classify a zero-information trace; returns the
/// handshake RTT in ms if the destination answered.
async fn tcp_reachability_ms(addr: IpAddr, port: u16, timeout_ms: u64) -> Option<f64> {
    let t0 = std::time::Instant::now();
    match tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        tokio::net::TcpStream::connect((addr, port)),
    )
    .await
    {
        Ok(Ok(_stream)) => Some(t0.elapsed().as_secs_f64() * 1000.0),
        _ => None,
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
        target_url: None,
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
        sample_index: 0,
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
        responsiveness: None,
        stamp: None,
        mthroughput: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform implementations
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct TraceOutcome {
    pub hops: Vec<PathHop>,
    pub hop_count: Option<u32>,
    pub destination_reached: bool,
    pub destination_rtt_ms: Option<f64>,
    pub method: String,
    /// Highest TTL actually probed, and whether the total budget cut the scan
    /// short. Used for honest messaging only - NOT part of the JSON contract.
    pub probed_ttls: u32,
    pub budget_truncated: bool,
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
        total_budget_ms: u64,
    ) -> Result<TraceOutcome, String> {
        let budget_deadline =
            (total_budget_ms > 0).then(|| Instant::now() + Duration::from_millis(total_budget_ms));
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

        // Constant 5-tuple: one socket (fixed src port) + one dest port for
        // all TTLs, so every hop rides the same per-flow ECMP path. Probes
        // are sequential and matched temporally via the error queue, so no
        // per-TTL port demultiplexing is needed (module docs).
        let dest = SocketAddr::new(addr, base_port);

        let mut probed_ttls = 0u32;
        let mut budget_truncated = false;
        for ttl in 1..=max_ttl {
            // Stop at the caller's budget rather than always spending
            // max_ttl x per_hop_timeout_ms on a path that answers nothing.
            if budget_deadline.is_some_and(|d| Instant::now() >= d) {
                budget_truncated = true;
                break;
            }
            probed_ttls = ttl;
            set_ttl(&socket, &addr, ttl).map_err(|e| format!("set TTL={ttl} failed: {e}"))?;
            let sent_at = Instant::now();
            if let Err(e) = socket.send_to(&[0u8; 8], dest) {
                // A synchronous refusal (previous hop's queued error) is
                // handled below via the error queue; other send errors on a
                // specific TTL count as a silent hop.
                tracing::debug!("path probe ttl={ttl} send error: {e}");
            }

            let mut deadline = sent_at + Duration::from_millis(per_hop_timeout_ms);
            if let Some(budget) = budget_deadline {
                deadline = deadline.min(budget);
            }
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
            probed_ttls,
            budget_truncated,
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
        total_budget_ms: u64,
    ) -> Result<TraceOutcome, String> {
        let budget_deadline =
            (total_budget_ms > 0).then(|| Instant::now() + Duration::from_millis(total_budget_ms));
        let bind: SocketAddr = match addr {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };

        let mut destination_reached = false;
        let mut destination_rtt_ms = None;
        let mut hop_count = None;

        let mut probed_ttls = 0u32;
        let mut budget_truncated = false;
        for ttl in 1..=max_ttl {
            // Same budget rule as the Linux path: never spend more than the
            // caller allowed on a destination that answers nothing.
            if budget_deadline.is_some_and(|d| Instant::now() >= d) {
                budget_truncated = true;
                break;
            }
            probed_ttls = ttl;
            // Fresh socket per TTL so a queued ICMP error from a previous
            // probe cannot be misattributed to this one.
            let socket = UdpSocket::bind(bind).map_err(|e| format!("UDP bind failed: {e}"))?;
            set_hop_limit(&socket, &addr, ttl)?;
            // Constant destination port (Paris-consistent); the source port
            // still varies per TTL here because each TTL needs a fresh socket
            // to keep queued ICMP errors from being misattributed.
            socket
                .connect(SocketAddr::new(addr, base_port))
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
            probed_ttls,
            budget_truncated,
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
            total_budget_ms: 0,
            verify_tcp_port: None,
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

    /// The trace must respect the caller's total budget instead of always
    /// spending `max_ttl * per_hop_timeout_ms`. Regression for the "path hangs
    /// on the second iteration" report: with every ICMP error filtered, each
    /// attempt cost 30 x 1 s regardless of `--timeout` (measured in the Docker
    /// lab: 61 s for `--runs 2`, 20 s after this fix with `--timeout 10`).
    ///
    /// Deliberately hermetic: TEST-NET-1 (RFC 5737) is not routable, so nothing
    /// answers - and whatever the environment does, the budget is the ceiling.
    #[test]
    fn a_total_budget_bounds_the_whole_trace() {
        let unbounded_cost = std::time::Duration::from_millis(8 * 5_000);
        let start = std::time::Instant::now();
        let outcome = platform::trace_blocking(
            "192.0.2.1".parse().unwrap(),
            8,     // max_ttl
            5_000, // per-hop: 8 x 5 s = 40 s if the budget were ignored
            DEFAULT_PATH_BASE_PORT,
            400, // total budget
        );
        let elapsed = start.elapsed();

        assert!(outcome.is_ok(), "trace errored: {outcome:?}");
        assert!(
            elapsed < unbounded_cost / 4,
            "budget ignored: the trace took {elapsed:?}"
        );
        let trace = outcome.unwrap();
        assert!(
            trace.budget_truncated || trace.destination_reached,
            "a scan cut short must say so (probed {} TTL(s))",
            trace.probed_ttls
        );
    }

    /// `0` keeps the classic traceroute behaviour for direct library/CLI users.
    #[test]
    fn the_default_config_is_unbounded() {
        assert_eq!(PathProbeConfig::default().total_budget_ms, 0);
    }
}
