//! Destination-aware UDP socket: reply from the address the request came to.
//!
//! Every UDP service in the endpoint (echo, STAMP reflector, throughput) binds
//! `0.0.0.0:<port>` and answers with `send_to`. On a multihomed host — a
//! Raspberry Pi with ethernet and wifi on the same subnet, a VM with a
//! management NIC and a data NIC, a laptop on ethernet with wifi still up —
//! the kernel picks the reply's SOURCE address from its routing table, not
//! from the address the request arrived on. When the two differ, the tester's
//! *connected* UDP sockets (`socket.connect(target)` in the udp, stamp, rpm and
//! udp_throughput probes) discard the reply as coming from a stranger, and
//! every UDP probe reports 100% loss while TCP and QUIC against the same host
//! are fine. Stateful firewalls and NATs drop such replies for the same
//! reason. (Home lab, 2026-09-02: probes sent to the Pi's wlan0 address, the
//! echo came back from eth0 — `tcpdump` was the only way to see it.)
//!
//! [`PktInfoSocket`] learns the local destination address of each datagram
//! (IP_PKTINFO / IP_RECVDSTADDR / IPV6_RECVPKTINFO) and stamps it back as the
//! source of the reply. The per-platform recvmsg/sendmsg plumbing comes from
//! `quinn-udp`, which the HTTP/3 server already pulls in. If that setup fails
//! the socket degrades to plain `recv_from` / `send_to`, so the service still
//! runs — it just keeps the old behaviour on that platform.
//!
//! `quinn-udp` configures sockets for QUIC (don't-fragment, GRO). Those are
//! wrong for a datagram echo service — a reply larger than the interface MTU
//! must still fragment like it always did, and one `recv` must stay one
//! datagram — so [`PktInfoSocket::new`] switches them back off after setup.

use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};

use quinn_udp::{RecvMeta, Transmit, UdpSocketState};
use tokio::io::Interest;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// A bound UDP socket whose receives report the local destination address and
/// whose sends can pin the source address.
pub struct PktInfoSocket {
    inner: UdpSocket,
    /// `None` when the platform setup failed — plain socket semantics apply.
    state: Option<UdpSocketState>,
}

/// One received datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Received {
    /// Payload length written into the caller's buffer.
    pub len: usize,
    /// Sender address — reply here.
    pub from: SocketAddr,
    /// Local address the datagram arrived on; `None` when the platform did not
    /// report one. Pass it straight back to [`PktInfoSocket::send_to`].
    pub dst_ip: Option<IpAddr>,
}

impl PktInfoSocket {
    /// Wrap an already-bound tokio socket. Never fails: on platforms (or
    /// kernels) where the destination-address plumbing cannot be enabled the
    /// socket keeps working with plain semantics and a warning is logged.
    pub fn new(inner: UdpSocket) -> Self {
        let state = match UdpSocketState::new((&inner).into()) {
            Ok(state) => {
                restore_datagram_semantics(&inner);
                Some(state)
            }
            Err(e) => {
                warn!(
                    "UDP {:?}: destination-address tracking unavailable ({e}); \
                     replies will use the kernel-chosen source address",
                    inner.local_addr().ok()
                );
                None
            }
        };
        Self { inner, state }
    }

    /// Whether replies can be pinned to the arrival address on this socket.
    pub fn tracks_destination(&self) -> bool {
        self.state.is_some()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Receive one datagram into `buf`.
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<Received> {
        let Some(state) = &self.state else {
            let (len, from) = self.inner.recv_from(buf).await?;
            return Ok(Received {
                len,
                from,
                dst_ip: None,
            });
        };
        loop {
            self.inner.readable().await?;
            let mut meta = [RecvMeta::default()];
            let res = self.inner.try_io(Interest::READABLE, || {
                let mut bufs = [IoSliceMut::new(buf)];
                state.recv((&self.inner).into(), &mut bufs, &mut meta)
            });
            match res {
                Ok(0) => continue,
                Ok(_) => {
                    let m = meta[0];
                    return Ok(Received {
                        len: m.len,
                        from: m.addr,
                        dst_ip: m.dst_ip,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// Send `buf` to `to`, using `from_ip` as the source address when given.
    ///
    /// A pinned source that the kernel rejects (the interface went away between
    /// request and reply) falls back to an unpinned send rather than dropping
    /// the reply.
    pub async fn send_to(
        &self,
        buf: &[u8],
        to: SocketAddr,
        from_ip: Option<IpAddr>,
    ) -> io::Result<()> {
        let Some(state) = &self.state else {
            self.inner.send_to(buf, to).await?;
            return Ok(());
        };
        match self.send_pinned(state, buf, to, from_ip).await {
            Err(e) if from_ip.is_some() => {
                debug!("UDP reply to {to} from {from_ip:?} rejected ({e}); retrying unpinned");
                self.send_pinned(state, buf, to, None).await
            }
            other => other,
        }
    }

    async fn send_pinned(
        &self,
        state: &UdpSocketState,
        buf: &[u8],
        to: SocketAddr,
        from_ip: Option<IpAddr>,
    ) -> io::Result<()> {
        let transmit = Transmit {
            destination: to,
            ecn: None,
            contents: buf,
            segment_size: None,
            src_ip: from_ip,
        };
        loop {
            self.inner.writable().await?;
            let res = self.inner.try_io(Interest::WRITABLE, || {
                state.try_send((&self.inner).into(), &transmit)
            });
            match res {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Undo quinn-udp's QUIC-oriented socket options
// ─────────────────────────────────────────────────────────────────────────────

/// `UdpSocketState::new` turns on don't-fragment (IP_MTU_DISCOVER=PROBE on
/// Linux, IP_DONTFRAG on Apple/BSD, IP_DONTFRAGMENT on Windows) and, on Linux,
/// UDP_GRO. Both change datagram semantics the echo services rely on: replies
/// larger than the interface MTU must fragment as before, and one receive must
/// be one datagram (GRO coalesces a burst into one buffer). Turn them back off.
/// The endpoint binds IPv4 sockets only; IPv6 counterparts are left as set.
#[cfg(unix)]
fn restore_datagram_semantics(sock: &UdpSocket) {
    use std::os::fd::AsRawFd;
    let fd = sock.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // Same value quinn-udp uses; libc does not export it on every target.
        const UDP_GRO: libc::c_int = 104;
        set_int_opt(fd, libc::SOL_UDP, UDP_GRO, 0, "UDP_GRO");
        set_int_opt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_MTU_DISCOVER,
            libc::IP_PMTUDISC_DONT,
            "IP_MTU_DISCOVER",
        );
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    {
        set_int_opt(fd, libc::IPPROTO_IP, libc::IP_DONTFRAG, 0, "IP_DONTFRAG");
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd"
    )))]
    {
        let _ = fd;
    }
}

#[cfg(unix)]
fn set_int_opt(
    fd: libc::c_int,
    level: libc::c_int,
    name: libc::c_int,
    value: libc::c_int,
    what: &str,
) {
    // SAFETY: plain setsockopt on a live fd owned by the tokio socket, with a
    // correctly sized c_int pointer; the kernel copies the value.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const libc::c_int).cast::<libc::c_void>(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        debug!(
            "UDP socket: could not reset {what} ({}) — leaving quinn-udp's setting",
            io::Error::last_os_error()
        );
    }
}

#[cfg(windows)]
fn restore_datagram_semantics(sock: &UdpSocket) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, IPPROTO_IP, IP_DONTFRAGMENT, SOCKET,
    };
    let off: u32 = 0; // DWORD-sized boolean
                      // SAFETY: plain setsockopt on the live socket handle with a correctly
                      // sized DWORD pointer; WinSock copies the value.
    let rc = unsafe {
        setsockopt(
            sock.as_raw_socket() as SOCKET,
            IPPROTO_IP,
            IP_DONTFRAGMENT,
            (&off as *const u32).cast::<u8>(),
            std::mem::size_of::<u32>() as i32,
        )
    };
    if rc != 0 {
        debug!(
            "UDP socket: could not reset IP_DONTFRAGMENT ({}) — leaving quinn-udp's setting",
            io::Error::last_os_error()
        );
    }
}

#[cfg(not(any(unix, windows)))]
fn restore_datagram_semantics(_sock: &UdpSocket) {}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — real sockets over loopback
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DEADLINE: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn reports_destination_and_echoes_verbatim() {
        let server = PktInfoSocket::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let server_addr = server.local_addr().unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"ping", server_addr).await.unwrap();

        let mut buf = [0u8; 64];
        let got = tokio::time::timeout(DEADLINE, server.recv_from(&mut buf))
            .await
            .expect("server never received the datagram")
            .unwrap();
        assert_eq!(&buf[..got.len], b"ping");
        assert_eq!(got.from, client.local_addr().unwrap());
        if server.tracks_destination() {
            assert_eq!(got.dst_ip, Some("127.0.0.1".parse().unwrap()));
        }

        server
            .send_to(&buf[..got.len], got.from, got.dst_ip)
            .await
            .unwrap();
        let mut back = [0u8; 64];
        let (n, from) = tokio::time::timeout(DEADLINE, client.recv_from(&mut back))
            .await
            .expect("client never received the reply")
            .unwrap();
        assert_eq!(&back[..n], b"ping");
        assert_eq!(from, server_addr);
    }

    /// The multihomed bug on a single host: Linux routes the whole
    /// 127.0.0.0/8 to `lo`, so a wildcard-bound server addressed as
    /// 127.0.0.2 would answer from 127.0.0.1 (the kernel's pick) and a client
    /// CONNECTED to 127.0.0.2 never sees the reply. With the arrival address
    /// pinned as the source, it does.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reply_comes_from_the_address_the_request_was_sent_to() {
        let server = PktInfoSocket::new(UdpSocket::bind("0.0.0.0:0").await.unwrap());
        assert!(
            server.tracks_destination(),
            "IP_PKTINFO must be available on Linux"
        );
        let port = server.local_addr().unwrap().port();

        let client = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        client.connect(("127.0.0.2", port)).await.unwrap();
        client.send(b"who-are-you").await.unwrap();

        let mut buf = [0u8; 64];
        let got = tokio::time::timeout(DEADLINE, server.recv_from(&mut buf))
            .await
            .expect("server never received the datagram")
            .unwrap();
        assert_eq!(got.dst_ip, Some("127.0.0.2".parse().unwrap()));

        server
            .send_to(&buf[..got.len], got.from, got.dst_ip)
            .await
            .unwrap();
        let mut back = [0u8; 64];
        let n = tokio::time::timeout(DEADLINE, client.recv(&mut back))
            .await
            .expect("connected client dropped the reply — it came from the wrong source address")
            .unwrap();
        assert_eq!(&back[..n], b"who-are-you");
    }

    /// Replies larger than the loopback MTU must still go out: quinn-udp's
    /// don't-fragment setting is undone in `new`. (Loopback MTU is 65536 on
    /// Linux, so this exercises the option reset rather than real
    /// fragmentation; the send simply must not fail with EMSGSIZE.)
    #[tokio::test]
    async fn large_reply_is_not_rejected() {
        let server = PktInfoSocket::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let big = vec![7u8; 8000];
        server
            .send_to(
                &big,
                client.local_addr().unwrap(),
                Some("127.0.0.1".parse().unwrap()),
            )
            .await
            .expect("8000-byte datagram must be sendable");
        let mut back = vec![0u8; 9000];
        let n = tokio::time::timeout(DEADLINE, client.recv(&mut back))
            .await
            .expect("client never received the large datagram")
            .unwrap();
        assert_eq!(n, 8000);
    }
}
