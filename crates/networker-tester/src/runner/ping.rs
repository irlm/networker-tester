//! ICMP echo probe (`ping` mode) — network-layer RTT without TCP/UDP.
//!
//! Unix uses UNPRIVILEGED ICMP datagram sockets (`SOCK_DGRAM` +
//! `IPPROTO_ICMP` / `IPPROTO_ICMPV6`), never raw sockets; Windows uses the
//! iphlpapi echo API. No platform needs elevation:
//!
//! - **Linux**: works when the process gid is inside
//!   `net.ipv4.ping_group_range` (many distros default it to `1 0` = nobody).
//!   A denied socket is classified as an [`ErrorCategory::Config`] error with
//!   the sysctl fix hint — it is an environment problem, not a network one.
//! - **macOS**: ICMP datagram sockets are unprivileged out of the box. Received
//!   datagrams include the IP header, which this module strips (and reads the
//!   reply TTL from).
//! - **Windows**: `IcmpSendEcho` (IPv4) / `Icmp6SendEcho2` (IPv6) — the
//!   kernel ICMP helper, unprivileged by design. The echo-reply TTL comes
//!   from the v4 reply options; the v6 reply structure carries no hop limit,
//!   so `reply_ttl` is honestly absent for IPv6 targets on Windows.
//!
//! Probes are sent back-to-back like the `udp` probe (next probe fires when
//! the previous echo arrives or times out); late/reordered/duplicate echoes
//! are credited to the probe that sent them by their embedded ICMP sequence
//! id (trust audit V12 semantics). Aggregation (min/avg/p95, mean inter-probe
//! delay variation as `jitter_ms`, loss) reuses [`aggregate_udp_rtts`].

use crate::metrics::{
    aggregate_udp_rtts, ErrorCategory, ErrorRecord, PingResult, Protocol, RequestAttempt,
};
use chrono::Utc;
use std::net::IpAddr;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PingProbeConfig {
    /// Hostname or IP literal to ping (the probe pings the first resolved
    /// address; use `--ipv4-only`/`--ipv6-only` upstream to steer families).
    pub target_host: String,
    /// Number of echo probes (reuses the `--udp-probes` count semantics).
    pub probe_count: u32,
    /// Per-probe echo timeout in ms.
    pub timeout_ms: u64,
    /// ICMP payload size in bytes (min 16: seq marker + timestamp + padding).
    pub payload_size: usize,
}

impl Default for PingProbeConfig {
    fn default() -> Self {
        Self {
            target_host: "127.0.0.1".to_string(),
            probe_count: 10,
            timeout_ms: 5000,
            payload_size: 56, // classic ping default payload
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_ping_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &PingProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    // ── Resolve the target to one address ────────────────────────────────────
    let addr: IpAddr = match resolve_host(&cfg.target_host).await {
        Ok(a) => a,
        Err(msg) => {
            return ping_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Dns,
                msg,
                None,
            )
        }
    };

    // ── Run the blocking ICMP loop off the async runtime ─────────────────────
    let count = cfg.probe_count.max(1);
    let timeout_ms = cfg.timeout_ms.max(1);
    let payload_size = cfg.payload_size;
    let outcome = tokio::task::spawn_blocking(move || {
        icmp::ping_blocking(addr, count, timeout_ms, payload_size)
    })
    .await;

    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            return ping_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Other,
                format!("ping worker task failed: {e}"),
                None,
            )
        }
    };

    let (probe_rtts, reply_ttl) = match outcome {
        Ok(v) => v,
        Err(icmp::PingError::Permission { message, hint }) => {
            return ping_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Config,
                message,
                Some(hint),
            )
        }
        Err(icmp::PingError::Unsupported(message)) => {
            return ping_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Config,
                message,
                None,
            )
        }
        Err(icmp::PingError::Io(message)) => {
            return ping_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Other,
                message,
                None,
            )
        }
    };

    let stats = aggregate_udp_rtts(&probe_rtts);
    let success_count = probe_rtts.iter().filter(|r| r.is_some()).count() as u32;

    let result = PingResult {
        remote_addr: addr.to_string(),
        probe_count: count,
        success_count,
        loss_percent: stats.loss_percent,
        rtt_min_ms: stats.min,
        rtt_avg_ms: stats.avg,
        rtt_p95_ms: stats.p95,
        jitter_ms: stats.jitter,
        probe_rtts_ms: probe_rtts,
        reply_ttl,
        started_at,
    };

    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Ping,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        // Same rule as the udp probe: all echoes lost = failure (loss is
        // still reported in the result), some echoes = success.
        success: success_count > 0,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
        ping: Some(result),
        path: None,
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

#[allow(clippy::too_many_arguments)]
fn ping_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
) -> RequestAttempt {
    RequestAttempt {
        phase: None,
        attempt_id,
        run_id,
        protocol: Protocol::Ping,
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
            detail,
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
        responsiveness: None,
        stamp: None,
        mthroughput: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform ICMP implementation
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) mod icmp {
    use std::net::IpAddr;

    #[derive(Debug)]
    pub enum PingError {
        /// The OS denied the unprivileged ICMP socket — configuration, not
        /// network. `hint` carries the per-platform fix.
        Permission { message: String, hint: String },
        /// The platform has no unprivileged ICMP path in this build.
        /// (Never constructed on Windows — IcmpSendEcho always exists there —
        /// hence the target-scoped allow.)
        #[cfg_attr(windows, allow(dead_code))]
        Unsupported(String),
        /// Everything else (bind/send errors, ...).
        Io(String),
    }

    /// Send `count` ICMP echo probes to `addr` back-to-back and return
    /// per-probe RTTs (None = lost) plus the reply TTL when observable.
    #[cfg(unix)]
    pub fn ping_blocking(
        addr: IpAddr,
        count: u32,
        timeout_ms: u64,
        payload_size: usize,
    ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
        unix_impl::ping(addr, count, timeout_ms, payload_size)
    }

    /// Windows: the iphlpapi `IcmpSendEcho` / `Icmp6SendEcho2` family — no
    /// raw sockets, no privileges required (the API is backed by the kernel
    /// ICMP driver and works for standard users, unlike BSD-style ICMP
    /// sockets which Windows does not offer unprivileged).
    #[cfg(windows)]
    pub fn ping_blocking(
        addr: IpAddr,
        count: u32,
        timeout_ms: u64,
        payload_size: usize,
    ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
        windows_impl::ping(addr, count, timeout_ms, payload_size)
    }

    /// Exotic targets (neither unix nor windows): honestly unsupported.
    #[cfg(not(any(unix, windows)))]
    pub fn ping_blocking(
        _addr: IpAddr,
        _count: u32,
        _timeout_ms: u64,
        _payload_size: usize,
    ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
        Err(PingError::Unsupported(
            "ping mode has no ICMP backend on this platform — \
             use the tcp or udp probes for RTT"
                .to_string(),
        ))
    }

    #[cfg(unix)]
    mod unix_impl {
        use super::PingError;
        use std::io;
        use std::mem;
        use std::net::IpAddr;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::time::{Duration, Instant};

        const ICMP_ECHO_REQUEST: u8 = 8;
        const ICMP_ECHO_REPLY: u8 = 0;
        const ICMPV6_ECHO_REQUEST: u8 = 128;
        const ICMPV6_ECHO_REPLY: u8 = 129;

        pub fn ping(
            addr: IpAddr,
            count: u32,
            timeout_ms: u64,
            payload_size: usize,
        ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
            let fd = open_icmp_socket(&addr)?;

            // Identifier for the echo request. Linux rewrites it to the
            // socket's kernel-assigned id on send AND demuxes replies per
            // socket; macOS preserves it. Either way replies are matched by
            // sequence id below.
            let ident = (std::process::id() & 0xffff) as u16;

            let mut send_times: Vec<Option<Instant>> = vec![None; count as usize];
            let mut probe_rtts: Vec<Option<f64>> = vec![None; count as usize];
            let mut reply_ttl: Option<u32> = None;

            // ICMP header (8) + payload. Payload floor of 8 keeps room for a
            // send-timestamp marker (useful in packet captures; RTTs are
            // timed from the send Instant, not the wire timestamp).
            let payload_size = payload_size.max(8);
            let mut packet = vec![0u8; 8 + payload_size];

            for seq in 0..count {
                build_echo_request(&mut packet, &addr, ident, seq as u16);

                let sent_at = Instant::now();
                if send_to(&fd, &packet, &addr).is_err() {
                    // Send failure (e.g. transient ENOBUFS): count as lost.
                    continue;
                }
                send_times[seq as usize] = Some(sent_at);

                let deadline = sent_at + Duration::from_millis(timeout_ms);
                recv_until(
                    &fd,
                    &addr,
                    seq,
                    deadline,
                    &send_times,
                    &mut probe_rtts,
                    &mut reply_ttl,
                );
            }

            Ok((probe_rtts, reply_ttl))
        }

        fn open_icmp_socket(addr: &IpAddr) -> Result<OwnedFd, PingError> {
            let (domain, proto) = match addr {
                IpAddr::V4(_) => (libc::AF_INET, libc::IPPROTO_ICMP),
                IpAddr::V6(_) => (libc::AF_INET6, libc::IPPROTO_ICMPV6),
            };
            // SAFETY: plain socket(2) call; the fd is owned immediately.
            let fd = unsafe { libc::socket(domain, libc::SOCK_DGRAM, proto) };
            if fd < 0 {
                let err = io::Error::last_os_error();
                return Err(match err.raw_os_error() {
                    Some(libc::EACCES) | Some(libc::EPERM) => PingError::Permission {
                        message: format!("OS denied the unprivileged ICMP datagram socket ({err})"),
                        hint: if cfg!(target_os = "linux") {
                            "Linux gates ICMP datagram sockets by group: add this process's \
                             gid to net.ipv4.ping_group_range, e.g. \
                             `sudo sysctl -w net.ipv4.ping_group_range='0 2147483647'` \
                             (persist in /etc/sysctl.d/)."
                                .to_string()
                        } else {
                            "This platform denied SOCK_DGRAM ICMP — run with elevated \
                             privileges or use the tcp/udp probes."
                                .to_string()
                        },
                    },
                    Some(libc::EPROTONOSUPPORT) | Some(libc::EAFNOSUPPORT) => {
                        PingError::Unsupported(format!(
                            "unprivileged ICMP datagram sockets are not supported here ({err})"
                        ))
                    }
                    _ => PingError::Io(format!("ICMP socket creation failed: {err}")),
                });
            }
            // SAFETY: fd is a freshly created, valid socket fd.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };

            // Short receive timeout so the credit loop can poll the deadline;
            // per-probe timing is enforced by `recv_until`'s deadline.
            let tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 100_000, // 100 ms slices
            };
            // SAFETY: fd valid; tv is a properly sized timeval.
            unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &tv as *const _ as *const libc::c_void,
                    mem::size_of::<libc::timeval>() as libc::socklen_t,
                );
            }

            // Best-effort reply-TTL visibility. Linux: request the TTL as a
            // cmsg (read via recvmsg). macOS: the ICMP dgram socket already
            // returns the full IP header, which carries the TTL.
            #[cfg(target_os = "linux")]
            if addr.is_ipv4() {
                let one: libc::c_int = 1;
                // SAFETY: fd valid; one is a c_int.
                unsafe {
                    libc::setsockopt(
                        fd.as_raw_fd(),
                        libc::IPPROTO_IP,
                        libc::IP_RECVTTL,
                        &one as *const _ as *const libc::c_void,
                        mem::size_of::<libc::c_int>() as libc::socklen_t,
                    );
                }
            }
            let _ = addr; // used only on linux above
            Ok(fd)
        }

        fn build_echo_request(packet: &mut [u8], addr: &IpAddr, ident: u16, seq: u16) {
            packet.fill(0);
            packet[0] = match addr {
                IpAddr::V4(_) => ICMP_ECHO_REQUEST,
                IpAddr::V6(_) => ICMPV6_ECHO_REQUEST,
            };
            packet[1] = 0; // code
            packet[4..6].copy_from_slice(&ident.to_be_bytes());
            packet[6..8].copy_from_slice(&seq.to_be_bytes());
            // Payload marker: micros-since-epoch (diagnostic only).
            let now_us = chrono::Utc::now().timestamp_micros();
            packet[8..16].copy_from_slice(&now_us.to_be_bytes());
            if addr.is_ipv4() {
                // ICMPv4 checksum is ours to compute (the Linux kernel
                // recomputes it for ping sockets, macOS forwards it as-is).
                let sum = icmp_checksum(packet);
                packet[2..4].copy_from_slice(&sum.to_be_bytes());
            }
            // ICMPv6 checksum (pseudo-header) is always filled by the kernel.
        }

        fn icmp_checksum(data: &[u8]) -> u16 {
            let mut sum: u32 = 0;
            let mut chunks = data.chunks_exact(2);
            for c in &mut chunks {
                sum += u32::from(u16::from_be_bytes([c[0], c[1]]));
            }
            if let [last] = chunks.remainder() {
                sum += u32::from(u16::from_be_bytes([*last, 0]));
            }
            while sum >> 16 != 0 {
                sum = (sum & 0xffff) + (sum >> 16);
            }
            !(sum as u16)
        }

        fn send_to(fd: &OwnedFd, packet: &[u8], addr: &IpAddr) -> io::Result<()> {
            let sent = match addr {
                IpAddr::V4(v4) => {
                    let sa = libc::sockaddr_in {
                        sin_family: libc::AF_INET as libc::sa_family_t,
                        sin_port: 0,
                        sin_addr: libc::in_addr {
                            s_addr: u32::from_ne_bytes(v4.octets()),
                        },
                        sin_zero: [0; 8],
                        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
                        sin_len: mem::size_of::<libc::sockaddr_in>() as u8,
                    };
                    // SAFETY: fd valid, packet lives for the call, sa is a
                    // properly initialized sockaddr_in.
                    unsafe {
                        libc::sendto(
                            fd.as_raw_fd(),
                            packet.as_ptr() as *const libc::c_void,
                            packet.len(),
                            0,
                            &sa as *const _ as *const libc::sockaddr,
                            mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                        )
                    }
                }
                IpAddr::V6(v6) => {
                    let mut sa: libc::sockaddr_in6 = unsafe { mem::zeroed() };
                    sa.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                    sa.sin6_addr.s6_addr = v6.octets();
                    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
                    {
                        sa.sin6_len = mem::size_of::<libc::sockaddr_in6>() as u8;
                    }
                    // SAFETY: as above, with sockaddr_in6.
                    unsafe {
                        libc::sendto(
                            fd.as_raw_fd(),
                            packet.as_ptr() as *const libc::c_void,
                            packet.len(),
                            0,
                            &sa as *const _ as *const libc::sockaddr,
                            mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
                        )
                    }
                }
            };
            if sent < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Receive echo replies until `current_seq` is credited or `deadline`
        /// passes; every reply is credited to the probe its sequence id names
        /// (late/reordered/duplicate handling identical to the udp probe).
        #[allow(clippy::too_many_arguments)]
        fn recv_until(
            fd: &OwnedFd,
            addr: &IpAddr,
            current_seq: u32,
            deadline: Instant,
            send_times: &[Option<Instant>],
            probe_rtts: &mut [Option<f64>],
            reply_ttl: &mut Option<u32>,
        ) {
            let mut buf = vec![0u8; 4096];
            while probe_rtts[current_seq as usize].is_none() {
                if Instant::now() >= deadline {
                    return;
                }
                let (n, ttl) = match recv_with_ttl(fd, &mut buf) {
                    Ok(v) => v,
                    Err(ref e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut =>
                    {
                        continue; // 100 ms slice elapsed — re-check deadline
                    }
                    Err(_) => return, // hard socket error — give up on window
                };
                let Some((kind, _ident, seq, hdr_ttl)) = parse_echo_reply(addr, &buf[..n]) else {
                    continue; // not an echo reply for us
                };
                if kind != EchoKind::Reply {
                    continue;
                }
                let idx = seq as usize;
                if idx < probe_rtts.len() {
                    if let (Some(sent_at), None) = (send_times[idx], probe_rtts[idx]) {
                        probe_rtts[idx] = Some(sent_at.elapsed().as_secs_f64() * 1000.0);
                        // cmsg TTL (Linux) or IP-header TTL (macOS v4).
                        if let Some(t) = ttl.or(hdr_ttl) {
                            *reply_ttl = Some(t);
                        }
                    }
                    // else: duplicate or unknown seq — ignore.
                }
            }
        }

        #[derive(PartialEq)]
        enum EchoKind {
            Reply,
            Other,
        }

        /// Parse a received datagram into (kind, ident, seq, ip_header_ttl).
        /// Handles the macOS quirk where ICMPv4 datagrams arrive with the IP
        /// header attached; the header TTL is returned when visible that way.
        fn parse_echo_reply(
            addr: &IpAddr,
            data: &[u8],
        ) -> Option<(EchoKind, u16, u16, Option<u32>)> {
            let (icmp, hdr_ttl) = strip_ip_header(addr, data)?;
            if icmp.len() < 8 {
                return None;
            }
            let expected_reply = match addr {
                IpAddr::V4(_) => ICMP_ECHO_REPLY,
                IpAddr::V6(_) => ICMPV6_ECHO_REPLY,
            };
            let kind = if icmp[0] == expected_reply {
                EchoKind::Reply
            } else {
                EchoKind::Other
            };
            let ident = u16::from_be_bytes([icmp[4], icmp[5]]);
            let seq = u16::from_be_bytes([icmp[6], icmp[7]]);
            Some((kind, ident, seq, hdr_ttl))
        }

        /// BSD ICMPv4 dgram sockets deliver the IP header before the ICMP
        /// message; Linux and all ICMPv6 sockets deliver bare ICMP. Returns
        /// the ICMP slice plus the IP-header TTL when one was present.
        fn strip_ip_header<'a>(addr: &IpAddr, data: &'a [u8]) -> Option<(&'a [u8], Option<u32>)> {
            if addr.is_ipv4() && data.len() >= 20 && data[0] >> 4 == 4 {
                let ihl = ((data[0] & 0x0f) as usize) * 4;
                if ihl >= 20 && data.len() > ihl {
                    return Some((&data[ihl..], Some(u32::from(data[8]))));
                }
            }
            Some((data, None))
        }

        /// recvmsg wrapper that also extracts the TTL/hop-limit cmsg when the
        /// platform provides one (Linux IP_RECVTTL). Falls back to the IP
        /// header TTL (macOS v4) via `strip_ip_header` in the parse step.
        fn recv_with_ttl(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<(usize, Option<u32>)> {
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr() as *mut libc::c_void,
                iov_len: buf.len(),
            };
            let mut cmsg_space = [0u8; 64];
            // SAFETY: zeroed msghdr filled with valid pointers below.
            let mut msg: libc::msghdr = unsafe { mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = cmsg_space.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = cmsg_space.len() as _;

            // SAFETY: fd valid; msg points at live buffers for the call.
            let n = unsafe { libc::recvmsg(fd.as_raw_fd(), &mut msg, 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }

            let mut ttl = None;
            // SAFETY: cmsg iteration over the kernel-filled control buffer,
            // using the libc CMSG_* accessors as documented.
            unsafe {
                let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
                while !cmsg.is_null() {
                    let c = &*cmsg;
                    let is_ttl = c.cmsg_level == libc::IPPROTO_IP
                        && (c.cmsg_type == libc::IP_TTL || {
                            #[cfg(target_os = "linux")]
                            {
                                c.cmsg_type == libc::IP_RECVTTL
                            }
                            #[cfg(not(target_os = "linux"))]
                            {
                                false
                            }
                        });
                    let is_hoplimit =
                        c.cmsg_level == libc::IPPROTO_IPV6 && c.cmsg_type == libc::IPV6_HOPLIMIT;
                    if is_ttl || is_hoplimit {
                        let p = libc::CMSG_DATA(cmsg) as *const libc::c_int;
                        if !p.is_null() {
                            ttl = Some((*p).max(0) as u32);
                        }
                    }
                    cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
                }
            }
            Ok((n as usize, ttl))
        }
    }

    #[cfg(windows)]
    mod windows_impl {
        use super::PingError;
        use std::mem;
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        use std::time::Instant;
        use windows_sys::Win32::Foundation::{
            GetLastError, ERROR_ACCESS_DENIED, HANDLE, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::NetworkManagement::IpHelper::{
            Icmp6CreateFile, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
            ICMPV6_ECHO_REPLY_LH, ICMP_ECHO_REPLY, IP_SUCCESS,
        };
        use windows_sys::Win32::Networking::WinSock::{AF_INET6, SOCKADDR_IN6};

        /// Owned iphlpapi ICMP handle, closed on drop.
        struct IcmpFile(HANDLE);
        impl Drop for IcmpFile {
            fn drop(&mut self) {
                // SAFETY: the handle came from Icmp(6)CreateFile and is
                // closed exactly once.
                unsafe {
                    IcmpCloseHandle(self.0);
                }
            }
        }

        fn open(v6: bool) -> Result<IcmpFile, PingError> {
            // SAFETY: plain handle-creating API call; ownership is taken
            // immediately by IcmpFile.
            let h = unsafe {
                if v6 {
                    Icmp6CreateFile()
                } else {
                    IcmpCreateFile()
                }
            };
            if h == INVALID_HANDLE_VALUE || h.is_null() {
                // SAFETY: reads the calling thread's last-error value.
                let code = unsafe { GetLastError() };
                return Err(if code == ERROR_ACCESS_DENIED {
                    PingError::Permission {
                        message: format!("Windows denied the ICMP helper handle (error {code})"),
                        hint: "IcmpCreateFile normally needs no privileges — check endpoint \
                               security policies, or use the tcp/udp probes."
                            .to_string(),
                    }
                } else {
                    PingError::Io(format!("IcmpCreateFile failed (error {code})"))
                });
            }
            Ok(IcmpFile(h))
        }

        pub fn ping(
            addr: IpAddr,
            count: u32,
            timeout_ms: u64,
            payload_size: usize,
        ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
            // Payload floor of 8 mirrors the unix path (room for the
            // send-timestamp marker); ceiling keeps the u16 request-size and
            // reply-buffer arithmetic safe.
            let payload_size = payload_size.clamp(8, 65_000);
            let timeout = u32::try_from(timeout_ms).unwrap_or(u32::MAX).max(1);
            match addr {
                IpAddr::V4(v4) => ping_v4(v4, count, timeout, payload_size),
                IpAddr::V6(v6) => ping_v6(v6, count, timeout, payload_size),
            }
        }

        fn ping_v4(
            dest: Ipv4Addr,
            count: u32,
            timeout_ms: u32,
            payload_size: usize,
        ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
            let handle = open(false)?;
            let mut payload = vec![0u8; payload_size];
            // Per the IcmpSendEcho docs: one ICMP_ECHO_REPLY + the echoed
            // payload + 8 bytes for an ICMP error message. Backed by u64s so
            // the reply-struct read below is properly aligned.
            let reply_len = mem::size_of::<ICMP_ECHO_REPLY>() + payload_size + 8;
            let mut reply_buf = vec![0u64; reply_len.div_ceil(8)];

            let mut probe_rtts: Vec<Option<f64>> = Vec::with_capacity(count as usize);
            let mut reply_ttl: Option<u32> = None;
            for _ in 0..count {
                stamp(&mut payload);
                let sent_at = Instant::now();
                // SAFETY: handle is live; payload/reply buffers outlive the
                // synchronous call and the passed sizes match them.
                let replies = unsafe {
                    IcmpSendEcho(
                        handle.0,
                        u32::from_ne_bytes(dest.octets()), // network order in memory
                        payload.as_ptr().cast(),
                        payload.len() as u16,
                        std::ptr::null(),
                        reply_buf.as_mut_ptr().cast(),
                        reply_len as u32,
                        timeout_ms,
                    )
                };
                if replies > 0 {
                    // SAFETY: replies > 0 guarantees an ICMP_ECHO_REPLY at
                    // the start of the (aligned) reply buffer.
                    let reply = unsafe { &*(reply_buf.as_ptr() as *const ICMP_ECHO_REPLY) };
                    if reply.Status == IP_SUCCESS {
                        // Timed from our own send Instant (sub-ms) like the
                        // unix path; the API's RoundTripTime is whole ms.
                        probe_rtts.push(Some(sent_at.elapsed().as_secs_f64() * 1000.0));
                        // Echo-reply IP TTL from the reply options.
                        reply_ttl = Some(u32::from(reply.Options.Ttl));
                    } else {
                        // Network-level status (unreachable, TTL expired, …):
                        // an honest lost probe.
                        probe_rtts.push(None);
                    }
                } else {
                    classify_send_error(&mut probe_rtts)?;
                }
            }
            Ok((probe_rtts, reply_ttl))
        }

        fn ping_v6(
            dest: Ipv6Addr,
            count: u32,
            timeout_ms: u32,
            payload_size: usize,
        ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
            let handle = open(true)?;

            // SAFETY: SOCKADDR_IN6 is plain old data; zeroed is a valid
            // "unspecified" init (:: source lets the stack pick).
            let mut src: SOCKADDR_IN6 = unsafe { mem::zeroed() };
            src.sin6_family = AF_INET6;
            // SAFETY: same POD zero-init as above.
            let mut dst: SOCKADDR_IN6 = unsafe { mem::zeroed() };
            dst.sin6_family = AF_INET6;
            dst.sin6_addr.u.Byte = dest.octets();

            let mut payload = vec![0u8; payload_size];
            let reply_len = mem::size_of::<ICMPV6_ECHO_REPLY_LH>() + payload_size + 8;
            let mut reply_buf = vec![0u64; reply_len.div_ceil(8)];

            let mut probe_rtts: Vec<Option<f64>> = Vec::with_capacity(count as usize);
            for _ in 0..count {
                stamp(&mut payload);
                let sent_at = Instant::now();
                // SAFETY: handle is live; sockaddrs and buffers outlive the
                // synchronous call (no event handle, no APC routine).
                let replies = unsafe {
                    Icmp6SendEcho2(
                        handle.0,
                        std::ptr::null_mut(), // no event — fully synchronous
                        None,                 // no APC routine
                        std::ptr::null(),     // no APC context
                        &src,
                        &dst,
                        payload.as_ptr().cast(),
                        payload.len() as u16,
                        std::ptr::null(),
                        reply_buf.as_mut_ptr().cast(),
                        reply_len as u32,
                        timeout_ms,
                    )
                };
                if replies > 0 {
                    // SAFETY: replies > 0 guarantees an ICMPV6_ECHO_REPLY_LH
                    // at the start of the (aligned) reply buffer.
                    let reply = unsafe { &*(reply_buf.as_ptr() as *const ICMPV6_ECHO_REPLY_LH) };
                    if reply.Status == IP_SUCCESS {
                        probe_rtts.push(Some(sent_at.elapsed().as_secs_f64() * 1000.0));
                    } else {
                        probe_rtts.push(None);
                    }
                } else {
                    classify_send_error(&mut probe_rtts)?;
                }
            }
            // ICMPV6_ECHO_REPLY_LH carries no hop limit — the reply TTL is
            // honestly unobservable for IPv6 through this API.
            Ok((probe_rtts, None))
        }

        /// Icmp(6)SendEcho returned 0 replies. IP_STATUS codes (the 11000
        /// range: IP_REQ_TIMED_OUT = 11010, host/net unreachable, TTL
        /// expired, …) are network outcomes — the probe is honestly lost.
        /// Anything else is an API/environment failure and must abort rather
        /// than masquerade as packet loss.
        fn classify_send_error(probe_rtts: &mut Vec<Option<f64>>) -> Result<(), PingError> {
            // SAFETY: reads the calling thread's last-error value.
            let code = unsafe { GetLastError() };
            if (11_000..=11_999).contains(&code) {
                probe_rtts.push(None);
                Ok(())
            } else {
                Err(PingError::Io(format!("IcmpSendEcho failed (error {code})")))
            }
        }

        /// Same diagnostic payload marker as the unix path: micros-since-
        /// epoch in the first 8 bytes (useful in captures; RTTs are timed
        /// from the send Instant, not this wire timestamp).
        fn stamp(payload: &mut [u8]) {
            let now_us = chrono::Utc::now().timestamp_micros();
            payload[0..8].copy_from_slice(&now_us.to_be_bytes());
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// True when this environment cannot open unprivileged ICMP sockets
    /// (e.g. Linux CI with a restrictive ping_group_range). Windows must
    /// never land here: IcmpSendEcho needs no privileges, so a Config error
    /// there is a probe bug, not an environment limitation.
    fn env_lacks_icmp(attempt: &RequestAttempt) -> bool {
        let config_denied = !attempt.success
            && attempt
                .error
                .as_ref()
                .is_some_and(|e| e.category == ErrorCategory::Config);
        if config_denied && cfg!(windows) {
            panic!(
                "Windows ping must not report a Config error: {:?}",
                attempt.error
            );
        }
        config_denied
    }

    #[tokio::test]
    async fn ping_loopback_reports_rtts() {
        let cfg = PingProbeConfig {
            target_host: "127.0.0.1".into(),
            probe_count: 3,
            timeout_ms: 2000,
            payload_size: 56,
        };
        let attempt = run_ping_probe(Uuid::new_v4(), 0, &cfg).await;
        if env_lacks_icmp(&attempt) {
            eprintln!(
                "SKIP ping_loopback_reports_rtts: no unprivileged ICMP here: {:?}",
                attempt.error
            );
            return;
        }
        assert!(attempt.success, "ping failed: {:?}", attempt.error);
        assert_eq!(attempt.protocol, Protocol::Ping);
        let p = attempt.ping.expect("ping result");
        assert_eq!(p.probe_count, 3);
        assert!(p.success_count > 0);
        assert_eq!(p.probe_rtts_ms.len(), 3);
        assert!(p.rtt_avg_ms > 0.0);
        assert!(p.rtt_p95_ms >= p.rtt_min_ms);
        assert_eq!(p.remote_addr, "127.0.0.1");
    }

    #[tokio::test]
    async fn ping_unresolvable_host_is_dns_error() {
        let cfg = PingProbeConfig {
            target_host: "this-hostname-does-not-exist.invalid".into(),
            probe_count: 1,
            timeout_ms: 200,
            payload_size: 56,
        };
        let attempt = run_ping_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!attempt.success);
        assert!(attempt.ping.is_none());
        let err = attempt.error.expect("error must be set");
        assert_eq!(err.category, ErrorCategory::Dns);
    }

    /// A permission-denied ICMP socket must surface as Config (with a fix
    /// hint), and success must never be reported without echoes. This test
    /// only checks classification invariants — it passes both in permissive
    /// and locked-down environments.
    #[tokio::test]
    async fn ping_never_fabricates_success() {
        let cfg = PingProbeConfig {
            target_host: "127.0.0.1".into(),
            probe_count: 1,
            timeout_ms: 500,
            payload_size: 56,
        };
        let attempt = run_ping_probe(Uuid::new_v4(), 0, &cfg).await;
        if attempt.success {
            let p = attempt.ping.expect("successful ping carries a result");
            assert!(p.success_count > 0, "success without echoes is fabrication");
        } else {
            assert!(attempt.error.is_some(), "failure must carry an error");
        }
    }
}
