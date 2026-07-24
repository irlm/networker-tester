//! Path-MTU discovery probe (`pmtud` mode) — DF-bit probing without raw
//! sockets.
//!
//! Sends UDP datagrams with the Don't-Fragment bit set at binary-searched
//! sizes toward the target and reads whatever fragmentation feedback the
//! platform exposes unprivileged:
//!
//! - **Linux**: `IP_MTU_DISCOVER = IP_PMTUDISC_DO` (kernel sets DF and
//!   enforces the cached path MTU) + `IP_RECVERR`, so every ICMP
//!   fragmentation-needed / ICMPv6 packet-too-big lands on the socket error
//!   queue WITH the next-hop MTU (`ee_info`) and its origin. Local `EMSGSIZE`
//!   (interface constraint) is distinguished from wire ICMP by
//!   `SO_EE_ORIGIN_*` — only real ICMP counts as path evidence.
//! - **macOS**: `IP_DONTFRAG` / `IPV6_DONTFRAG`. There is no error queue; an
//!   ICMP error surfaces as `EMSGSIZE` / `ECONNREFUSED` on a subsequent
//!   operation on the connected socket (no next-hop MTU value).
//! - **Windows**: winsock `IP_DONTFRAGMENT` / `IPV6_DONTFRAG`. A DF send
//!   larger than the local/route MTU fails with `WSAEMSGSIZE`; an ICMP
//!   port-unreachable surfaces as `WSAECONNRESET` on the connected socket
//!   (delivery confirmation, mirroring the macOS approach). Path ICMP
//!   fragmentation-needed is NOT reliably surfaced to UDP sockets on
//!   Windows, so: with delivery confirmation (echo or port-unreachable) the
//!   search still finds a path MTU below the local MTU (oversized DF probes
//!   simply never confirm); without any confirmation a path MTU below the
//!   local MTU is undetectable and the result is an honest `path_mtu: None`
//!   — never fake results.
//!
//! ## Delivery confirmation
//!
//! A size counts as *confirmed* when either:
//! - an **echo reply** comes back (the probe aims at the networker-endpoint's
//!   UDP echo port, :9999 by default), or
//! - an **ICMP port-unreachable** arrives — the destination only generates it
//!   for a datagram that actually reached it, so a DF datagram of that size
//!   provably traversed the path unfragmented. This makes the probe work
//!   against any live host, echo service or not.
//!
//! Port-unreachable attribution is heuristic (the error does not carry the
//! probe's sequence id): the error queue is drained before every send so a
//! stale error cannot be credited to a newer, larger probe.
//!
//! Without any confirmation the probe still concludes from ICMP
//! fragmentation-needed errors alone. When there is no confirmation, no
//! frag-needed and no send error, the result is an honest `path_mtu: None` —
//! a silent path and an ICMP black hole are indistinguishable, and we never
//! fabricate.

use crate::metrics::{ErrorCategory, ErrorRecord, PmtudResult, Protocol, RequestAttempt};
use chrono::Utc;
use std::net::IpAddr;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PmtudProbeConfig {
    /// Hostname or IP literal to probe toward.
    pub target_host: String,
    /// UDP port the DF probes are aimed at — the endpoint's UDP echo port
    /// when the target runs networker-endpoint (positive confirmation).
    pub target_port: u16,
    /// Per-probe wait for a confirmation (ms). Unconfirmed (ICMP-only) waits
    /// are capped at [`PMTUD_ICMP_WAIT_CAP_MS`] because errors arrive from
    /// nearby routers.
    pub probe_timeout_ms: u64,
    /// Extra sends per size before a silent probe is classified (confirmed
    /// mode only — absorbs isolated datagram loss).
    pub retries_per_size: u32,
    /// Search ceiling for the total IP MTU (bytes).
    pub max_mtu: u32,
}

pub const DEFAULT_PMTUD_TIMEOUT_MS: u64 = 1_000;
pub const DEFAULT_PMTUD_RETRIES: u32 = 2;
/// Jumbo-frame ceiling: paths above this are reported as a lower bound.
pub const DEFAULT_PMTUD_MAX_MTU: u32 = 9_216;
/// ICMP errors come from routers close by; waiting the full echo timeout for
/// every silent size would make the unconfirmed mode needlessly slow.
pub const PMTUD_ICMP_WAIT_CAP_MS: u64 = 300;

impl Default for PmtudProbeConfig {
    fn default() -> Self {
        Self {
            target_host: "127.0.0.1".to_string(),
            target_port: 9999,
            probe_timeout_ms: DEFAULT_PMTUD_TIMEOUT_MS,
            retries_per_size: DEFAULT_PMTUD_RETRIES,
            max_mtu: DEFAULT_PMTUD_MAX_MTU,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_pmtud_probe(
    run_id: Uuid,
    sequence_num: u32,
    cfg: &PmtudProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let addr: IpAddr = match resolve_host(&cfg.target_host).await {
        Ok(a) => a,
        Err(msg) => {
            return pmtud_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Dns,
                msg,
            )
        }
    };

    // Default-interface MTU for contrast (best-effort; subprocess-based on
    // macOS, so keep it off the async runtime).
    let host_for_ctx = cfg.target_host.clone();
    let port_for_ctx = cfg.target_port;
    let local_mtu = tokio::task::spawn_blocking(move || {
        crate::metrics::NetworkContext::collect(&host_for_ctx, port_for_ctx).mtu
    })
    .await
    .unwrap_or(None);

    let port = cfg.target_port;
    let probe_timeout_ms = cfg.probe_timeout_ms.max(1);
    let retries = cfg.retries_per_size;
    let max_mtu = cfg.max_mtu.clamp(576, 65_535);
    let outcome = tokio::task::spawn_blocking(move || {
        platform::discover_blocking(addr, port, probe_timeout_ms, retries, max_mtu)
    })
    .await;

    let d = match outcome {
        Ok(Ok(d)) => d,
        Ok(Err(msg)) => {
            return pmtud_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Other,
                msg,
            )
        }
        Err(e) => {
            return pmtud_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Other,
                format!("pmtud worker task failed: {e}"),
            )
        }
    };

    let header_bytes = if addr.is_ipv6() { 48 } else { 28 };
    let result = PmtudResult {
        remote_addr: addr.to_string(),
        path_mtu: d.path_mtu,
        max_unfragmented_payload: d.path_mtu.map(|m| m.saturating_sub(header_bytes)),
        probes_sent: d.probes_sent,
        method: d.method,
        icmp_mtu: d.icmp_mtu,
        local_mtu,
        header_bytes,
        lower_bound_only: d.lower_bound_only,
        started_at,
    };

    // A concluded MTU (even as a lower bound) is a successful measurement; no
    // feedback at all is an honest failure with the reason recorded.
    let success = result.path_mtu.is_some();
    let error = if success {
        None
    } else {
        Some(ErrorRecord {
            category: ErrorCategory::Udp,
            message: format!(
                "No path evidence: no echo replies and no wire ICMP across {} DF probes \
                 (local send limits alone are not path evidence) — silent path or ICMP \
                 black hole ({})",
                result.probes_sent, result.method
            ),
            detail: None,
            occurred_at: Utc::now(),
        })
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Pmtud,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error,
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
        pmtud: Some(result),
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

fn pmtud_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Pmtud,
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
// Shared search skeleton
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) struct DiscoverOutcome {
    /// Total IP-layer MTU; None when nothing was measured.
    pub path_mtu: Option<u32>,
    pub probes_sent: u32,
    pub method: String,
    /// Next-hop MTU from an ICMP frag-needed, when the platform exposes it.
    pub icmp_mtu: Option<u32>,
    /// True when the search ceiling fit without ever finding a bound.
    pub lower_bound_only: bool,
}

/// What one probe size told us.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SizeClass {
    /// Positively confirmed: an echo reply or an ICMP port-unreachable
    /// proved a DF datagram of this size reached the destination.
    Fit,
    /// No feedback within the window (unconfirmed mode treats this as
    /// tentatively-fits; confirmed mode retries then classifies TooBig).
    Silent,
    /// Fragmentation feedback. `mtu` = next-hop MTU when known;
    /// `wire_icmp` = true only for evidence that came off the wire (real
    /// ICMP), false for local interface-constraint errors.
    TooBig { mtu: Option<u32>, wire_icmp: bool },
}

/// Binary-search the largest payload in `[floor, ceil]` accepted by
/// `classify`, tracking evidence. Shared by both platform modules; the
/// platform supplies the per-size classifier. `header` converts the total-MTU
/// values inside `TooBig` feedback into payload-domain bounds.
pub(crate) fn search_largest_fit(
    floor: u32,
    ceil: u32,
    header: u32,
    mut classify: impl FnMut(u32) -> Result<SizeClass, String>,
) -> Result<SearchOutcome, String> {
    let mut confirmed_seen = false;
    let mut wire_icmp_seen = false;
    let mut bound_found = false;
    let mut icmp_mtu: Option<u32> = None;

    let mut note = |class: &SizeClass| match class {
        SizeClass::Fit => confirmed_seen = true,
        SizeClass::TooBig { mtu, wire_icmp } => {
            bound_found = true;
            if *wire_icmp {
                wire_icmp_seen = true;
                if let Some(m) = mtu {
                    icmp_mtu = Some(icmp_mtu.map_or(*m, |cur: u32| cur.min(*m)));
                }
            }
        }
        SizeClass::Silent => {}
    };

    // Establish a working floor: if even `floor` is too big, drop to a
    // minimal datagram before searching.
    let mut lo = floor;
    let mut hi = ceil;
    let mut lo_ok = false;
    for candidate in [floor, 32u32] {
        let class = classify(candidate)?;
        note(&class);
        match class {
            SizeClass::Fit | SizeClass::Silent => {
                lo = candidate;
                lo_ok = true;
                break;
            }
            SizeClass::TooBig { mtu, .. } => {
                hi = hi.min(candidate.saturating_sub(1));
                if let Some(m) = mtu {
                    hi = hi.min(m.saturating_sub(header));
                }
            }
        }
    }
    if !lo_ok {
        return Ok(SearchOutcome {
            largest_fit: None,
            confirmed_seen,
            wire_icmp_seen,
            bound_found,
            icmp_mtu,
        });
    }

    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let class = classify(mid)?;
        note(&class);
        match class {
            SizeClass::Fit | SizeClass::Silent => lo = mid,
            SizeClass::TooBig { mtu, .. } => {
                hi = mid - 1;
                if let Some(m) = mtu {
                    hi = hi.min(m.saturating_sub(header));
                }
            }
        }
    }

    Ok(SearchOutcome {
        largest_fit: Some(lo),
        confirmed_seen,
        wire_icmp_seen,
        bound_found,
        icmp_mtu,
    })
}

pub(crate) struct SearchOutcome {
    /// Largest payload the search settled on; None when even a minimal
    /// datagram was rejected.
    pub largest_fit: Option<u32>,
    /// Any size was positively confirmed (echo / port-unreachable).
    pub confirmed_seen: bool,
    pub wire_icmp_seen: bool,
    pub bound_found: bool,
    pub icmp_mtu: Option<u32>,
}

/// How delivery got confirmed — feeds the `method` string.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ConfirmKind {
    UdpEcho,
    PortUnreach,
}

impl ConfirmKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ConfirmKind::UdpEcho => "udp-echo",
            ConfirmKind::PortUnreach => "port-unreach",
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux_impl as platform;
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) use portable_impl as platform;
#[cfg(windows)]
pub(crate) use windows_impl as platform;

// ─────────────────────────────────────────────────────────────────────────────
// Linux: IP_PMTUDISC_DO + IP_RECVERR — full ICMP feedback with next-hop MTU
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub(crate) mod linux_impl {
    use super::{search_largest_fit, ConfirmKind, DiscoverOutcome, SizeClass};
    use std::io;
    use std::mem;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    pub fn discover_blocking(
        addr: IpAddr,
        port: u16,
        probe_timeout_ms: u64,
        retries: u32,
        max_mtu: u32,
    ) -> Result<DiscoverOutcome, String> {
        let bind: SocketAddr = match addr {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(bind).map_err(|e| format!("UDP bind failed: {e}"))?;
        socket
            .connect(SocketAddr::new(addr, port))
            .map_err(|e| format!("UDP connect failed: {e}"))?;
        enable_df_and_recverr(&socket, &addr)
            .map_err(|e| format!("DF/RECVERR setsockopt failed: {e}"))?;

        let header: u32 = if addr.is_ipv6() { 48 } else { 28 };
        let floor_mtu: u32 = if addr.is_ipv6() { 1280 } else { 576 };
        let floor = floor_mtu - header;
        let ceil = max_mtu.saturating_sub(header).max(floor);

        let mut probes_sent: u32 = 0;
        let mut buf = vec![0u8; ceil as usize];
        let mut confirm_kind: Option<ConfirmKind> = None;

        // Capability probe at floor size with the full timeout: does the
        // destination answer (echo or port-unreachable) at all?
        {
            drain_all(&socket);
            probes_sent += 1;
            stamp(&mut buf, 0);
            let _ = send_draining(&socket, &buf[..floor as usize]);
            match wait_feedback(&socket, probe_timeout_ms) {
                Feedback::Echo => confirm_kind = Some(ConfirmKind::UdpEcho),
                Feedback::PortUnreach => confirm_kind = Some(ConfirmKind::PortUnreach),
                _ => {}
            }
        }

        let mut seq: u32 = 0;
        let outcome = search_largest_fit(floor, ceil, header, |payload| {
            let confirmed_mode = confirm_kind.is_some();
            let wait_ms = if confirmed_mode {
                probe_timeout_ms
            } else {
                probe_timeout_ms.min(super::PMTUD_ICMP_WAIT_CAP_MS)
            };
            let tries = if confirmed_mode { retries + 1 } else { 1 };
            for _ in 0..tries {
                // Stale errors must not be credited to this (possibly
                // larger) probe — see module docs on attribution.
                drain_all(&socket);
                seq = seq.wrapping_add(1);
                stamp(&mut buf, seq);
                probes_sent += 1;
                match send_draining(&socket, &buf[..payload as usize]) {
                    SendOutcome::Sent => {}
                    SendOutcome::Emsgsize => {
                        // Kernel refused: cached PMTU or interface limit.
                        // Ask the kernel for its current estimate. Not wire
                        // evidence by itself (wire ICMP is tracked when it
                        // arrives through wait_feedback).
                        let mtu = current_kernel_mtu(&socket, &addr);
                        return Ok(SizeClass::TooBig {
                            mtu,
                            wire_icmp: false,
                        });
                    }
                    SendOutcome::Fatal(msg) => return Err(msg),
                }
                match wait_feedback(&socket, wait_ms) {
                    Feedback::Echo => {
                        confirm_kind.get_or_insert(ConfirmKind::UdpEcho);
                        return Ok(SizeClass::Fit);
                    }
                    Feedback::PortUnreach => {
                        confirm_kind.get_or_insert(ConfirmKind::PortUnreach);
                        return Ok(SizeClass::Fit);
                    }
                    Feedback::FragNeeded { mtu, wire_icmp } => {
                        return Ok(SizeClass::TooBig { mtu, wire_icmp })
                    }
                    Feedback::Nothing => {}
                }
            }
            if confirmed_mode {
                // Confirmations flowed at other sizes but not this one after
                // retries: with DF set, fragmentation drop is the by-far
                // likeliest cause.
                Ok(SizeClass::TooBig {
                    mtu: None,
                    wire_icmp: false,
                })
            } else {
                Ok(SizeClass::Silent)
            }
        })?;

        // Honest verdict per evidence class — see PmtudResult docs.
        let (path_mtu, method, lower_bound_only) = if let Some(kind) = confirm_kind {
            let mtu = outcome.largest_fit.map(|p| p + header);
            let lb =
                outcome.largest_fit == Some(ceil) && !outcome.bound_found && outcome.confirmed_seen;
            (mtu, format!("df-{}/ip-recverr", kind.label()), lb)
        } else if outcome.wire_icmp_seen {
            let mtu = outcome.largest_fit.map(|p| p + header);
            (mtu, "df-icmp/ip-recverr".to_string(), false)
        } else {
            (None, "df-no-feedback".to_string(), false)
        };

        Ok(DiscoverOutcome {
            path_mtu,
            probes_sent,
            method,
            icmp_mtu: outcome.icmp_mtu,
            lower_bound_only,
        })
    }

    fn stamp(buf: &mut [u8], seq: u32) {
        if buf.len() >= 12 {
            buf[0..4].copy_from_slice(&seq.to_be_bytes());
            let now_us = chrono::Utc::now().timestamp_micros();
            buf[4..12].copy_from_slice(&now_us.to_be_bytes());
        }
    }

    enum SendOutcome {
        Sent,
        /// Datagram larger than the kernel's current path-MTU estimate.
        Emsgsize,
        Fatal(String),
    }

    /// send() that retries through pending-async-error failures: with
    /// IP_RECVERR enabled a queued ICMP error fails the NEXT syscall with the
    /// stale errno (e.g. ECONNREFUSED) — drain and retry a bounded number of
    /// times.
    fn send_draining(socket: &UdpSocket, payload: &[u8]) -> SendOutcome {
        for _ in 0..4 {
            match socket.send(payload) {
                Ok(_) => return SendOutcome::Sent,
                Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => return SendOutcome::Emsgsize,
                Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => {
                    // Stale port-unreachable being reported; clear and retry.
                    drain_all(socket);
                    continue;
                }
                Err(e) => return SendOutcome::Fatal(format!("UDP send failed: {e}")),
            }
        }
        SendOutcome::Fatal("UDP send kept failing with pending async errors".to_string())
    }

    enum Feedback {
        /// An echo datagram came back.
        Echo,
        /// ICMP destination-unreachable / port — the datagram REACHED the
        /// destination (or the loopback fast path refused it locally).
        PortUnreach,
        /// ICMP fragmentation-needed / packet-too-big, or a queued local
        /// EMSGSIZE event.
        FragNeeded {
            mtu: Option<u32>,
            wire_icmp: bool,
        },
        Nothing,
    }

    fn wait_feedback(socket: &UdpSocket, timeout_ms: u64) -> Feedback {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Feedback::Nothing;
            }
            let wait = (deadline - now).as_millis().min(100) as libc::c_int;
            let mut pfd = libc::pollfd {
                fd: socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: single valid pollfd; POLLERR is reported regardless of
            // the requested events and signals error-queue readiness.
            let rc = unsafe { libc::poll(&mut pfd, 1, wait) };
            if rc < 0 {
                return Feedback::Nothing;
            }
            // Error queue first: a queued ICMP error otherwise fails the
            // next recv with a stale errno.
            match read_errqueue(socket) {
                ErrEntry::Frag { mtu, wire_icmp } => {
                    return Feedback::FragNeeded { mtu, wire_icmp }
                }
                ErrEntry::PortUnreach => return Feedback::PortUnreach,
                ErrEntry::Other => continue, // drained; keep waiting
                ErrEntry::Empty => {}
            }
            if pfd.revents & libc::POLLIN != 0 {
                let mut rbuf = [0u8; 65_535];
                // SAFETY: plain nonblocking recv into a stack buffer.
                let n = unsafe {
                    libc::recv(
                        socket.as_raw_fd(),
                        rbuf.as_mut_ptr() as *mut libc::c_void,
                        rbuf.len(),
                        libc::MSG_DONTWAIT,
                    )
                };
                if n > 0 {
                    return Feedback::Echo;
                }
            }
        }
    }

    enum ErrEntry {
        Frag { mtu: Option<u32>, wire_icmp: bool },
        PortUnreach,
        Other,
        Empty,
    }

    /// Read one error-queue entry (never blocks).
    fn read_errqueue(socket: &UdpSocket) -> ErrEntry {
        let mut data = [0u8; 512];
        let mut iov = libc::iovec {
            iov_base: data.as_mut_ptr() as *mut libc::c_void,
            iov_len: data.len(),
        };
        let mut name = [0u8; 128];
        let mut ctrl = [0u8; 512];
        // SAFETY: zeroed msghdr pointed at live buffers.
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_name = name.as_mut_ptr() as *mut libc::c_void;
        msg.msg_namelen = name.len() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = ctrl.as_mut_ptr() as *mut libc::c_void;
        // `as _`: msg_controllen is usize on glibc but u32 (socklen_t) on musl
        // (the release target — this exact pattern broke the v0.28.76 release
        // in path.rs; now also guarded by the CI musl-check job).
        msg.msg_controllen = ctrl.len() as _;

        // SAFETY: MSG_ERRQUEUE never blocks; -1/EAGAIN when empty.
        let n = unsafe {
            libc::recvmsg(
                socket.as_raw_fd(),
                &mut msg,
                libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT,
            )
        };
        if n < 0 {
            return ErrEntry::Empty;
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
                    return classify_err(ee);
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        ErrEntry::Other
    }

    /// Discard everything currently queued (stale-attribution guard).
    fn drain_all(socket: &UdpSocket) {
        for _ in 0..64 {
            if matches!(read_errqueue(socket), ErrEntry::Empty) {
                break;
            }
        }
    }

    fn classify_err(ee: &libc::sock_extended_err) -> ErrEntry {
        const ICMP_DEST_UNREACH: u8 = 3;
        const ICMP_FRAG_NEEDED_CODE: u8 = 4;
        const ICMP_PORT_UNREACH_CODE: u8 = 3;
        const ICMPV6_DEST_UNREACH: u8 = 1;
        const ICMPV6_PORT_UNREACH_CODE: u8 = 4;
        const ICMPV6_PACKET_TOO_BIG: u8 = 2;

        let origin = u32::from(ee.ee_origin);
        let mtu = (ee.ee_info > 0).then_some(ee.ee_info);
        if origin == libc::SO_EE_ORIGIN_ICMP as u32 {
            return match (ee.ee_type, ee.ee_code) {
                (ICMP_DEST_UNREACH, ICMP_FRAG_NEEDED_CODE) => ErrEntry::Frag {
                    mtu,
                    wire_icmp: true,
                },
                (ICMP_DEST_UNREACH, ICMP_PORT_UNREACH_CODE) => ErrEntry::PortUnreach,
                _ => ErrEntry::Other,
            };
        }
        if origin == libc::SO_EE_ORIGIN_ICMP6 as u32 {
            return match (ee.ee_type, ee.ee_code) {
                (ICMPV6_PACKET_TOO_BIG, _) => ErrEntry::Frag {
                    mtu,
                    wire_icmp: true,
                },
                (ICMPV6_DEST_UNREACH, ICMPV6_PORT_UNREACH_CODE) => ErrEntry::PortUnreach,
                _ => ErrEntry::Other,
            };
        }
        // Local origin: loopback fast-path connection-refused proves arrival
        // (same-host delivery); a local EMSGSIZE event is the kernel
        // enforcing a route/interface MTU — a bound, but not wire evidence.
        if ee.ee_errno == libc::ECONNREFUSED as u32 {
            return ErrEntry::PortUnreach;
        }
        if ee.ee_errno == libc::EMSGSIZE as u32 {
            return ErrEntry::Frag {
                mtu,
                wire_icmp: false,
            };
        }
        ErrEntry::Other
    }

    /// getsockopt(IP_MTU/IPV6_MTU): the kernel's current path-MTU estimate
    /// for this connected socket (interface MTU until an ICMP tightens it).
    fn current_kernel_mtu(socket: &UdpSocket, addr: &IpAddr) -> Option<u32> {
        let (level, opt) = match addr {
            IpAddr::V4(_) => (libc::IPPROTO_IP, libc::IP_MTU),
            IpAddr::V6(_) => (libc::IPPROTO_IPV6, libc::IPV6_MTU),
        };
        let mut mtu: libc::c_int = 0;
        let mut len = mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: valid fd, c_int out-param.
        let rc = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                level,
                opt,
                &mut mtu as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        (rc == 0 && mtu > 0).then_some(mtu as u32)
    }

    fn enable_df_and_recverr(socket: &UdpSocket, addr: &IpAddr) -> io::Result<()> {
        let fd = socket.as_raw_fd();
        let set = |level: libc::c_int, opt: libc::c_int, val: libc::c_int| -> io::Result<()> {
            // SAFETY: valid fd, c_int option value.
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    level,
                    opt,
                    &val as *const _ as *const libc::c_void,
                    mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            if rc < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        match addr {
            IpAddr::V4(_) => {
                set(
                    libc::IPPROTO_IP,
                    libc::IP_MTU_DISCOVER,
                    libc::IP_PMTUDISC_DO,
                )?;
                set(libc::IPPROTO_IP, libc::IP_RECVERR, 1)
            }
            IpAddr::V6(_) => {
                set(
                    libc::IPPROTO_IPV6,
                    libc::IPV6_MTU_DISCOVER,
                    libc::IPV6_PMTUDISC_DO,
                )?;
                set(libc::IPPROTO_IPV6, libc::IPV6_RECVERR, 1)
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// macOS / other unix: IP_DONTFRAG — EMSGSIZE-based, honest degradation
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) mod portable_impl {
    use super::{search_largest_fit, ConfirmKind, DiscoverOutcome, SizeClass};
    use std::io::ErrorKind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
    use std::time::{Duration, Instant};

    pub fn discover_blocking(
        addr: IpAddr,
        port: u16,
        probe_timeout_ms: u64,
        retries: u32,
        max_mtu: u32,
    ) -> Result<DiscoverOutcome, String> {
        let bind: SocketAddr = match addr {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(bind).map_err(|e| format!("UDP bind failed: {e}"))?;
        socket
            .connect(SocketAddr::new(addr, port))
            .map_err(|e| format!("UDP connect failed: {e}"))?;
        set_dontfrag(&socket, &addr)?;

        let header: u32 = if addr.is_ipv6() { 48 } else { 28 };
        let floor_mtu: u32 = if addr.is_ipv6() { 1280 } else { 576 };
        let floor = floor_mtu - header;
        let ceil = max_mtu.saturating_sub(header).max(floor);

        let mut probes_sent: u32 = 0;
        let mut buf = vec![0u8; ceil as usize];
        let mut confirm_kind: Option<ConfirmKind> = None;

        // Capability probe at floor size.
        {
            probes_sent += 1;
            stamp(&mut buf, 0);
            let _ = socket.send(&buf[..floor as usize]);
            match recv_within(&socket, probe_timeout_ms) {
                RecvOutcome::Data => confirm_kind = Some(ConfirmKind::UdpEcho),
                RecvOutcome::ConnRefused => confirm_kind = Some(ConfirmKind::PortUnreach),
                _ => {}
            }
        }

        let mut seq: u32 = 0;
        let outcome = search_largest_fit(floor, ceil, header, |payload| {
            let confirmed_mode = confirm_kind.is_some();
            let wait_ms = if confirmed_mode {
                probe_timeout_ms
            } else {
                probe_timeout_ms.min(super::PMTUD_ICMP_WAIT_CAP_MS)
            };
            let tries = if confirmed_mode { retries + 1 } else { 1 };
            for _ in 0..tries {
                seq = seq.wrapping_add(1);
                stamp(&mut buf, seq);
                probes_sent += 1;
                match send_clearing(&socket, &buf[..payload as usize]) {
                    SendOutcome::Sent => {}
                    SendOutcome::Emsgsize => {
                        // Local interface constraint OR a previously received
                        // ICMP being reported — macOS gives no MTU value and
                        // no origin, so this is never counted as wire ICMP.
                        return Ok(SizeClass::TooBig {
                            mtu: None,
                            wire_icmp: false,
                        });
                    }
                    SendOutcome::Fatal(msg) => return Err(msg),
                }
                match recv_within(&socket, wait_ms) {
                    RecvOutcome::Data => {
                        confirm_kind.get_or_insert(ConfirmKind::UdpEcho);
                        return Ok(SizeClass::Fit);
                    }
                    // An ICMP port-unreachable delivered on the connected
                    // socket: the datagram reached the destination.
                    RecvOutcome::ConnRefused => {
                        confirm_kind.get_or_insert(ConfirmKind::PortUnreach);
                        return Ok(SizeClass::Fit);
                    }
                    // ICMP frag-needed delivered asynchronously — real wire
                    // evidence, though without a next-hop MTU value.
                    RecvOutcome::Emsgsize => {
                        return Ok(SizeClass::TooBig {
                            mtu: None,
                            wire_icmp: true,
                        })
                    }
                    RecvOutcome::Timeout => {}
                }
            }
            if confirmed_mode {
                Ok(SizeClass::TooBig {
                    mtu: None,
                    wire_icmp: false,
                })
            } else {
                Ok(SizeClass::Silent)
            }
        })?;

        let (path_mtu, method, lower_bound_only) = if let Some(kind) = confirm_kind {
            let mtu = outcome.largest_fit.map(|p| p + header);
            let lb =
                outcome.largest_fit == Some(ceil) && !outcome.bound_found && outcome.confirmed_seen;
            (mtu, format!("df-dontfrag/{}", kind.label()), lb)
        } else if outcome.wire_icmp_seen {
            // EMSGSIZE surfaced on recv: an ICMP told the kernel the packet
            // was too big — real path evidence, but with no MTU value.
            let mtu = outcome.largest_fit.map(|p| p + header);
            (mtu, "df-dontfrag/emsgsize".to_string(), false)
        } else {
            // Only local send errors (or nothing at all): the interface
            // constraint is already reported via local_mtu — no path claim.
            (None, "df-no-feedback".to_string(), false)
        };

        Ok(DiscoverOutcome {
            path_mtu,
            probes_sent,
            method,
            // Never observable without an error queue — always None here.
            icmp_mtu: outcome.icmp_mtu,
            lower_bound_only,
        })
    }

    fn stamp(buf: &mut [u8], seq: u32) {
        if buf.len() >= 12 {
            buf[0..4].copy_from_slice(&seq.to_be_bytes());
            let now_us = chrono::Utc::now().timestamp_micros();
            buf[4..12].copy_from_slice(&now_us.to_be_bytes());
        }
    }

    enum SendOutcome {
        Sent,
        Emsgsize,
        Fatal(String),
    }

    /// send() that retries through a pending async ECONNREFUSED (BSD reports
    /// a delivered ICMP error on the next socket op, then clears it).
    fn send_clearing(socket: &UdpSocket, payload: &[u8]) -> SendOutcome {
        for _ in 0..4 {
            match socket.send(payload) {
                Ok(_) => return SendOutcome::Sent,
                Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => return SendOutcome::Emsgsize,
                Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => continue,
                Err(e) => return SendOutcome::Fatal(format!("UDP send failed: {e}")),
            }
        }
        SendOutcome::Fatal("UDP send kept failing with pending async errors".to_string())
    }

    enum RecvOutcome {
        /// An echo datagram came back.
        Data,
        /// EMSGSIZE reported on recv — asynchronously delivered ICMP.
        Emsgsize,
        /// ECONNREFUSED reported on recv — ICMP port-unreachable, i.e. the
        /// probe datagram reached the destination.
        ConnRefused,
        Timeout,
    }

    /// Wait up to `timeout_ms` for a readable echo or an async socket error.
    fn recv_within(socket: &UdpSocket, timeout_ms: u64) -> RecvOutcome {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        let mut rbuf = [0u8; 65_535];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return RecvOutcome::Timeout;
            }
            if socket.set_read_timeout(Some(deadline - now)).is_err() {
                return RecvOutcome::Timeout;
            }
            match socket.recv(&mut rbuf) {
                Ok(_) => return RecvOutcome::Data,
                Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => {
                    return RecvOutcome::Emsgsize;
                }
                Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => {
                    return RecvOutcome::ConnRefused;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    return RecvOutcome::Timeout;
                }
                // Other async errors are drained and the wait continues.
                Err(_) => continue,
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn set_dontfrag(socket: &UdpSocket, addr: &IpAddr) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        let one: libc::c_int = 1;
        let (level, opt) = match addr {
            IpAddr::V4(_) => (libc::IPPROTO_IP, libc::IP_DONTFRAG),
            IpAddr::V6(_) => (libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG),
        };
        // SAFETY: valid fd, c_int option value.
        let rc = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                level,
                opt,
                &one as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            Err(format!(
                "DONTFRAG setsockopt failed: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    fn set_dontfrag(_socket: &UdpSocket, _addr: &IpAddr) -> Result<(), String> {
        // Windows has its own module (windows_impl); other unixes without a
        // known DF socket option refuse honestly rather than probe without
        // DF (which would measure nothing).
        Err("pmtud mode has no DF socket-option support on this platform".to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows: winsock IP_DONTFRAGMENT — WSAEMSGSIZE on send, honest degradation
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
pub(crate) mod windows_impl {
    use super::{search_largest_fit, ConfirmKind, DiscoverOutcome, SizeClass};
    use std::io::ErrorKind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
    use std::os::windows::io::AsRawSocket;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, IPPROTO_IP, IPPROTO_IPV6, IPV6_DONTFRAG, IP_DONTFRAGMENT, SOCKET,
        WSAECONNRESET, WSAEMSGSIZE,
    };

    pub fn discover_blocking(
        addr: IpAddr,
        port: u16,
        probe_timeout_ms: u64,
        retries: u32,
        max_mtu: u32,
    ) -> Result<DiscoverOutcome, String> {
        let bind: SocketAddr = match addr {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(bind).map_err(|e| format!("UDP bind failed: {e}"))?;
        socket
            .connect(SocketAddr::new(addr, port))
            .map_err(|e| format!("UDP connect failed: {e}"))?;
        set_dontfragment(&socket, &addr)?;

        let header: u32 = if addr.is_ipv6() { 48 } else { 28 };
        let floor_mtu: u32 = if addr.is_ipv6() { 1280 } else { 576 };
        let floor = floor_mtu - header;
        let ceil = max_mtu.saturating_sub(header).max(floor);

        let mut probes_sent: u32 = 0;
        let mut buf = vec![0u8; ceil as usize];
        let mut confirm_kind: Option<ConfirmKind> = None;

        // Capability probe at floor size: does the destination answer (echo
        // or port-unreachable) at all?
        {
            probes_sent += 1;
            stamp(&mut buf, 0);
            let _ = socket.send(&buf[..floor as usize]);
            match recv_within(&socket, probe_timeout_ms) {
                RecvOutcome::Data => confirm_kind = Some(ConfirmKind::UdpEcho),
                RecvOutcome::ConnReset => confirm_kind = Some(ConfirmKind::PortUnreach),
                RecvOutcome::Timeout => {}
            }
        }

        let mut seq: u32 = 0;
        let outcome = search_largest_fit(floor, ceil, header, |payload| {
            let confirmed_mode = confirm_kind.is_some();
            let wait_ms = if confirmed_mode {
                probe_timeout_ms
            } else {
                probe_timeout_ms.min(super::PMTUD_ICMP_WAIT_CAP_MS)
            };
            let tries = if confirmed_mode { retries + 1 } else { 1 };
            for _ in 0..tries {
                seq = seq.wrapping_add(1);
                stamp(&mut buf, seq);
                probes_sent += 1;
                match send_clearing(&socket, &buf[..payload as usize]) {
                    SendOutcome::Sent => {}
                    SendOutcome::Msgsize => {
                        // WSAEMSGSIZE with DF set: the datagram exceeds the
                        // local/route MTU. Winsock gives no next-hop MTU and
                        // no origin — never counted as wire ICMP.
                        return Ok(SizeClass::TooBig {
                            mtu: None,
                            wire_icmp: false,
                        });
                    }
                    SendOutcome::Fatal(msg) => return Err(msg),
                }
                match recv_within(&socket, wait_ms) {
                    RecvOutcome::Data => {
                        confirm_kind.get_or_insert(ConfirmKind::UdpEcho);
                        return Ok(SizeClass::Fit);
                    }
                    // ICMP port-unreachable delivered on the connected
                    // socket: the datagram of this size REACHED the
                    // destination unfragmented.
                    RecvOutcome::ConnReset => {
                        confirm_kind.get_or_insert(ConfirmKind::PortUnreach);
                        return Ok(SizeClass::Fit);
                    }
                    RecvOutcome::Timeout => {}
                }
            }
            if confirmed_mode {
                // Confirmations flowed at other sizes but not this one after
                // retries: with DF set, fragmentation drop is the by-far
                // likeliest cause.
                Ok(SizeClass::TooBig {
                    mtu: None,
                    wire_icmp: false,
                })
            } else {
                Ok(SizeClass::Silent)
            }
        })?;

        // Honest verdict: on Windows there is no wire-ICMP evidence class at
        // all (frag-needed is not surfaced to UDP sockets), so a conclusion
        // REQUIRES delivery confirmation.
        let (path_mtu, method, lower_bound_only) = if let Some(kind) = confirm_kind {
            let mtu = outcome.largest_fit.map(|p| p + header);
            let lb =
                outcome.largest_fit == Some(ceil) && !outcome.bound_found && outcome.confirmed_seen;
            (mtu, format!("df-dontfragment/{}", kind.label()), lb)
        } else {
            // Only local send errors (or nothing at all): the interface
            // constraint is already reported via local_mtu — no path claim.
            (None, "df-no-feedback".to_string(), false)
        };

        Ok(DiscoverOutcome {
            path_mtu,
            probes_sent,
            method,
            // Never observable without an error queue — always None here.
            icmp_mtu: outcome.icmp_mtu,
            lower_bound_only,
        })
    }

    fn stamp(buf: &mut [u8], seq: u32) {
        if buf.len() >= 12 {
            buf[0..4].copy_from_slice(&seq.to_be_bytes());
            let now_us = chrono::Utc::now().timestamp_micros();
            buf[4..12].copy_from_slice(&now_us.to_be_bytes());
        }
    }

    enum SendOutcome {
        Sent,
        /// WSAEMSGSIZE: datagram larger than the local/route MTU with DF set.
        Msgsize,
        Fatal(String),
    }

    /// send() that retries through a pending async WSAECONNRESET (winsock
    /// reports a delivered ICMP port-unreachable on the next socket op,
    /// then clears it).
    fn send_clearing(socket: &UdpSocket, payload: &[u8]) -> SendOutcome {
        for _ in 0..4 {
            match socket.send(payload) {
                Ok(_) => return SendOutcome::Sent,
                Err(e) if e.raw_os_error() == Some(WSAEMSGSIZE) => return SendOutcome::Msgsize,
                Err(e) if e.raw_os_error() == Some(WSAECONNRESET) => continue,
                Err(e) => return SendOutcome::Fatal(format!("UDP send failed: {e}")),
            }
        }
        SendOutcome::Fatal("UDP send kept failing with pending async errors".to_string())
    }

    enum RecvOutcome {
        /// An echo datagram came back.
        Data,
        /// WSAECONNRESET reported on recv — ICMP port-unreachable, i.e. the
        /// probe datagram reached the destination.
        ConnReset,
        Timeout,
    }

    /// Wait up to `timeout_ms` for a readable echo or an async socket error.
    fn recv_within(socket: &UdpSocket, timeout_ms: u64) -> RecvOutcome {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        let mut rbuf = [0u8; 65_535];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return RecvOutcome::Timeout;
            }
            if socket.set_read_timeout(Some(deadline - now)).is_err() {
                return RecvOutcome::Timeout;
            }
            match socket.recv(&mut rbuf) {
                Ok(_) => return RecvOutcome::Data,
                Err(e) if e.raw_os_error() == Some(WSAECONNRESET) => {
                    return RecvOutcome::ConnReset;
                }
                // WSAEMSGSIZE on recv means a datagram ARRIVED but was
                // truncated (unlike unix, it is not ICMP feedback) — with a
                // 65535-byte buffer this cannot trigger, but if it ever did,
                // arrival is still delivery confirmation.
                Err(e) if e.raw_os_error() == Some(WSAEMSGSIZE) => return RecvOutcome::Data,
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    return RecvOutcome::Timeout;
                }
                // Other async errors are drained and the wait continues.
                Err(_) => continue,
            }
        }
    }

    fn set_dontfragment(socket: &UdpSocket, addr: &IpAddr) -> Result<(), String> {
        let one: u32 = 1;
        let (level, opt) = match addr {
            IpAddr::V4(_) => (IPPROTO_IP, IP_DONTFRAGMENT),
            IpAddr::V6(_) => (IPPROTO_IPV6, IPV6_DONTFRAG),
        };
        // SAFETY: valid live socket; optval points at a 4-byte DWORD for the
        // duration of the call.
        let rc = unsafe {
            setsockopt(
                socket.as_raw_socket() as SOCKET,
                level,
                opt,
                &one as *const u32 as *const u8,
                std::mem::size_of::<u32>() as i32,
            )
        };
        if rc != 0 {
            Err(format!(
                "IP_DONTFRAGMENT setsockopt failed: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared binary search converges on the boundary the classifier
    /// encodes, independent of platform signal plumbing.
    #[test]
    fn search_converges_on_exact_boundary() {
        // Path accepts ≤ 1472, rejects above (classic 1500-byte v4 path).
        let out = search_largest_fit(548, 9188, 28, |size| {
            Ok(if size <= 1472 {
                SizeClass::Fit
            } else {
                SizeClass::TooBig {
                    mtu: Some(1500),
                    wire_icmp: true,
                }
            })
        })
        .unwrap();
        assert_eq!(out.largest_fit, Some(1472));
        assert!(out.confirmed_seen);
        assert!(out.wire_icmp_seen);
        assert!(out.bound_found);
        assert_eq!(out.icmp_mtu, Some(1500));
    }

    #[test]
    fn search_ceiling_fit_reports_no_bound() {
        let out = search_largest_fit(548, 9188, 28, |_| Ok(SizeClass::Fit)).unwrap();
        assert_eq!(out.largest_fit, Some(9188));
        assert!(!out.bound_found, "no TooBig was ever observed");
    }

    #[test]
    fn search_all_silent_settles_on_ceiling_without_evidence() {
        // ICMP-only mode with a black-holed path: everything silent.
        let out = search_largest_fit(548, 9188, 28, |_| Ok(SizeClass::Silent)).unwrap();
        assert_eq!(out.largest_fit, Some(9188));
        assert!(!out.confirmed_seen);
        assert!(!out.wire_icmp_seen);
        assert!(!out.bound_found);
    }

    #[test]
    fn search_even_minimal_datagram_rejected() {
        let out = search_largest_fit(548, 9188, 28, |_| {
            Ok(SizeClass::TooBig {
                mtu: None,
                wire_icmp: false,
            })
        })
        .unwrap();
        assert_eq!(out.largest_fit, None);
        assert!(out.bound_found);
    }

    #[test]
    fn search_icmp_mtu_takes_the_tightest_bound() {
        let out = search_largest_fit(548, 9188, 28, |size| {
            Ok(if size <= 1252 {
                SizeClass::Silent
            } else {
                SizeClass::TooBig {
                    // Routers report their next-hop MTUs; keep the min.
                    mtu: Some(if size > 4000 { 4352 } else { 1280 }),
                    wire_icmp: true,
                }
            })
        })
        .unwrap();
        assert_eq!(out.largest_fit, Some(1252));
        assert_eq!(out.icmp_mtu, Some(1280));
    }

    #[test]
    fn confirm_kind_labels() {
        assert_eq!(ConfirmKind::UdpEcho.label(), "udp-echo");
        assert_eq!(ConfirmKind::PortUnreach.label(), "port-unreach");
    }

    #[tokio::test]
    async fn pmtud_unresolvable_host_is_dns_error() {
        let cfg = PmtudProbeConfig {
            target_host: "this-hostname-does-not-exist.invalid".into(),
            ..Default::default()
        };
        let attempt = run_pmtud_probe(Uuid::new_v4(), 0, &cfg).await;
        assert!(!attempt.success);
        assert!(attempt.pmtud.is_none());
        // Every platform (incl. Windows, since the IP_DONTFRAGMENT backend
        // landed) reaches the resolver first and reports Dns.
        assert_eq!(
            attempt.error.expect("error must be set").category,
            ErrorCategory::Dns
        );
    }
}
