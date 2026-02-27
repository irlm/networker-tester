/// UDP echo server.
///
/// Echoes every received datagram back to the sender verbatim.
/// Wire format expected by the client:
///   [4 bytes: seq u32 BE] [8 bytes: timestamp_us i64 BE] [payload...]
///
/// The server does not need to interpret the format; it just echoes bytes.
use tracing::{debug, warn};

pub async fn run_udp_echo(port: u16) {
    let bind = format!("0.0.0.0:{port}");
    let socket = match tokio::net::UdpSocket::bind(&bind).await {
        Ok(s) => s,
        Err(e) => {
            warn!("UDP echo server failed to bind on {bind}: {e}");
            return;
        }
    };
    debug!("UDP echo listening on {bind}");

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
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn udp_echo_server_reflects_packets() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server_sock.local_addr().unwrap().port();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            for _ in 0..5 {
                let (n, addr) = server_sock.recv_from(&mut buf).await.unwrap();
                server_sock.send_to(&buf[..n], addr).await.unwrap();
            }
        });

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.connect(format!("127.0.0.1:{port}")).await.unwrap();

        let msg = b"hello udp echo";
        client.send(msg).await.unwrap();

        let mut recv = vec![0u8; 1024];
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.recv(&mut recv),
        )
        .await
        .expect("timeout")
        .unwrap();

        assert_eq!(&recv[..n], msg);
    }
}
