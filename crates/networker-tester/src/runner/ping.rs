//! ICMP echo probe (`ping` mode) — network-layer RTT without TCP/UDP.
//!
//! Uses UNPRIVILEGED ICMP datagram sockets (`SOCK_DGRAM` + `IPPROTO_ICMP` /
//! `IPPROTO_ICMPV6`), never raw sockets:
//!
//! - **Linux**: works when the process gid is inside
//!   `net.ipv4.ping_group_range` (many distros default it to `1 0` = nobody).
//!   A denied socket is classified as an [`ErrorCategory::Config`] error with
//!   the sysctl fix hint — it is an environment problem, not a network one.
//! - **macOS**: ICMP datagram sockets are unprivileged out of the box. Received
//!   datagrams include the IP header, which this module strips (and reads the
//!   reply TTL from).
//! - **Windows**: not yet supported — reported as a clean Config error
//!   (`IcmpSendEcho` support is a follow-up), never a bogus timeout.
//!
//! Probes are sent back-to-back like the `udp` probe (next probe fires when
//! the previous echo arrives or times out); late/reordered/duplicate echoes
//! are credited to the probe that sent them by their embedded ICMP sequence
//! id (trust audit V12 semantics). Aggregation (min/avg/p95, arrival-order
//! jitter, loss) reuses [`aggregate_udp_rtts`].

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

    /// Windows: honestly unsupported (no unprivileged BSD-style ICMP socket;
    /// `IcmpSendEcho` integration is a planned follow-up). Classified as a
    /// Config error upstream so it can never masquerade as packet loss.
    #[cfg(not(unix))]
    pub fn ping_blocking(
        _addr: IpAddr,
        _count: u32,
        _timeout_ms: u64,
        _payload_size: usize,
    ) -> Result<(Vec<Option<f64>>, Option<u32>), PingError> {
        Err(PingError::Unsupported(
            "ping mode is not supported on Windows yet (IcmpSendEcho integration pending) — \
             use the tcp or udp probes for RTT on this platform"
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
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// True when this environment cannot open unprivileged ICMP sockets
    /// (e.g. Linux CI with a restrictive ping_group_range, or Windows).
    fn env_lacks_icmp(attempt: &RequestAttempt) -> bool {
        !attempt.success
            && attempt
                .error
                .as_ref()
                .is_some_and(|e| e.category == ErrorCategory::Config)
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
