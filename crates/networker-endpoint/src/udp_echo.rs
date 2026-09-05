/// UDP echo server.
///
/// Echoes every received datagram back to the sender verbatim.
/// Wire format expected by the client:
///   [4 bytes: seq u32 BE] [8 bytes: timestamp_us i64 BE] [payload...]
///
/// The server does not need to interpret the format; it just echoes bytes.
///
/// Replies leave from the address the request arrived on (see
/// `pktinfo_socket.rs`) so a client on a multihomed host's "other" interface,
/// or behind a stateful firewall, actually gets its echo back.
use crate::pktinfo_socket::PktInfoSocket;
use tracing::{debug, warn};

pub async fn run_udp_echo(socket: tokio::net::UdpSocket) {
    let socket = PktInfoSocket::new(socket);
    debug!(
        "UDP echo listening on {:?} (reply-source pinning: {})",
        socket.local_addr().ok(),
        socket.tracks_destination()
    );

    let mut buf = vec![0u8; 65_535];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok(rx) => {
                debug!("UDP echo: {} bytes from {}", rx.len, rx.from);
                if let Err(e) = socket.send_to(&buf[..rx.len], rx.from, rx.dst_ip).await {
                    warn!("UDP echo send error: {e}");
                }
            }
            Err(e) => {
                warn!("UDP echo recv error: {e}");
                // Avoid tight spin on persistent errors
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run_udp_echo;
    use tokio::net::UdpSocket;

    /// Exercises the REAL `run_udp_echo`. The previous version of this test
    /// spawned its own hand-rolled echo loop, so the production function
    /// could be stubbed to nothing and the test still passed — the mutation
    /// pilot flagged exactly that (`replace run_udp_echo with ()` survived).
    #[tokio::test]
    async fn udp_echo_server_reflects_packets() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();
        let task = tokio::spawn(run_udp_echo(server_sock));

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.connect(format!("127.0.0.1:{port}")).await.unwrap();

        let msg = b"hello udp echo";
        client.send(msg).await.unwrap();

        let mut recv = vec![0u8; 1024];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.recv(&mut recv))
            .await
            .expect("run_udp_echo never echoed the datagram back")
            .unwrap();

        assert_eq!(&recv[..n], msg, "echo must be verbatim");
        task.abort();
    }

    /// Multihomed regression (home lab, 2026-09-02): a wildcard-bound server
    /// addressed on one of its addresses must echo FROM that address, or a
    /// connected client never sees the reply. Linux routes all of
    /// 127.0.0.0/8 to `lo`, so 127.0.0.2 stands in for the "other" interface.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn udp_echo_replies_from_the_address_it_was_reached_on() {
        let server_sock = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();
        let task = tokio::spawn(run_udp_echo(server_sock));

        let client = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        client.connect(("127.0.0.2", port)).await.unwrap();
        client.send(b"via-secondary-address").await.unwrap();

        let mut recv = vec![0u8; 64];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.recv(&mut recv))
            .await
            .expect("echo never arrived — it was sent from the wrong source address")
            .unwrap();
        assert_eq!(&recv[..n], b"via-secondary-address");
        task.abort();
    }
}
