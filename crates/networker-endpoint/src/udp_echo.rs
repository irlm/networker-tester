/// UDP echo server.
///
/// Echoes every received datagram back to the sender verbatim.
/// Wire format expected by the client:
///   [4 bytes: seq u32 BE] [8 bytes: timestamp_us i64 BE] [payload...]
///
/// The server does not need to interpret the format; it just echoes bytes.
use tracing::{debug, warn};

pub async fn run_udp_echo(socket: tokio::net::UdpSocket) {
    debug!("UDP echo listening on {:?}", socket.local_addr().ok());

    let mut buf = vec![0u8; 65_535];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, addr)) => {
                debug!("UDP echo: {n} bytes from {addr}");
                if let Err(e) = socket.send_to(&buf[..n], addr).await {
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
}
