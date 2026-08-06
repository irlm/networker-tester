//! A UDP bind failure must REACH THE CALLER.
//!
//! The three UDP services used to bind inside their own spawned tasks and, on
//! failure, log a warning and return. The endpoint then reported success while
//! silently missing a service — and a caller waiting for that service saw only
//! a timeout. In CI that surfaced as "UDP echo server did not start within 40s"
//! (2026-08-06), which blamed slowness for what was an instant, nameable error.
//!
//! These pin the contract that makes the timeout message honest again.

use tokio::net::UdpSocket;
use tokio::sync::oneshot;

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Bind a UDP port and KEEP it, so the endpoint cannot have it.
async fn occupy_udp() -> (UdpSocket, u16) {
    let sock = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    let port = sock.local_addr().unwrap().port();
    (sock, port)
}

#[tokio::test]
async fn a_taken_udp_port_fails_startup_with_a_named_error() {
    let (_held, taken) = occupy_udp().await;

    let cfg = networker_endpoint::ServerConfig {
        http_port: free_tcp_port(),
        https_port: free_tcp_port(),
        udp_port: taken, // ← already ours
        udp_throughput_port: occupy_udp().await.1,
        stamp_port: occupy_udp().await.1,
    };

    let (_tx, rx) = oneshot::channel::<()>();

    // Bounded: if the bind failure is ever swallowed again, the endpoint starts
    // normally and this future NEVER returns. Without the timeout the test hangs
    // until the harness kills it (observed: 10 minutes) instead of failing with
    // a usable message — a test that hangs on regression is barely better than
    // one that passes on it.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        networker_endpoint::run_with_shutdown(cfg, rx),
    )
    .await;

    let err = match result {
        Ok(Err(e)) => format!("{e:#}"),
        Ok(Ok(())) => panic!(
            "endpoint reported SUCCESS with UDP port {taken} already bound — a caller would \
             wait out its whole readiness budget and then blame slowness"
        ),
        Err(_elapsed) => panic!(
            "endpoint KEPT RUNNING with UDP port {taken} already bound: the bind failure was \
             swallowed instead of returned, so the service is silently missing"
        ),
    };

    // The message has to name the service and the port, or the next person
    // debugging this is back to guessing.
    assert!(
        err.contains("UDP echo"),
        "error does not name the failing service: {err}"
    );
    assert!(
        err.contains(&taken.to_string()),
        "error does not name the port {taken}: {err}"
    );
}

#[tokio::test]
async fn startup_succeeds_when_the_ports_are_free() {
    // Guards the guard: if run_with_shutdown always errored, the test above
    // would pass for the wrong reason.
    let cfg = networker_endpoint::ServerConfig {
        http_port: free_tcp_port(),
        https_port: free_tcp_port(),
        udp_port: 0, // 0 = let the OS pick, so no collision is possible
        udp_throughput_port: 0,
        stamp_port: 0,
    };

    let (tx, rx) = oneshot::channel::<()>();
    let server = tokio::spawn(networker_endpoint::run_with_shutdown(cfg, rx));

    // Give it a moment to get past the binds, then shut it down cleanly.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!server.is_finished(), "endpoint exited early on free ports");

    let _ = tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), server).await;
}
