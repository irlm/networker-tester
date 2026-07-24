/// Integration tests for networker-tester.
///
/// These tests start the `networker-endpoint` in-process on random ports and
/// exercise the full probe pipeline end-to-end (DNS → TCP → TLS → HTTP → collect).
///
/// # Test layers
///
/// | Layer | Command | Requires |
/// |-------|---------|----------|
/// | Unit  | `cargo test --workspace --lib` | Nothing |
/// | **Integration** | `cargo test --test integration -p networker-tester` | Nothing (endpoint is in-process) |
/// | SQL   | `NETWORKER_SQL_CONN=… cargo test --workspace -- sql --include-ignored` | SQL Server |
///
/// Run just this layer:
///   cargo test --test integration -p networker-tester
use networker_tester::metrics::{ErrorCategory, Protocol};
use networker_tester::runner::curl::run_curl_probe;
use networker_tester::runner::dns::run_dns_probe;
use networker_tester::runner::http::{run_probe, RunConfig};
use networker_tester::runner::native::run_native_probe;
#[cfg(feature = "http3")]
use networker_tester::runner::pageload::run_pageload3_probe;
use networker_tester::runner::pageload::{run_pageload2_probe, run_pageload_probe, PageLoadConfig};
use networker_tester::runner::rpm::{run_rpm_probe, RpmProbeConfig};
use networker_tester::runner::throughput::{
    run_download_probe, run_upload_probe, run_webdownload_probe, run_webupload_probe,
    ThroughputConfig,
};
use networker_tester::runner::tls::{run_tls_probe, run_tls_resumption_probe};
use networker_tester::runner::udp::{run_udp_probe, UdpProbeConfig};
use networker_tester::runner::udp_throughput::{
    run_udpdownload_probe, run_udpupload_probe, UdpThroughputConfig,
};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn free_port() -> u16 {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Like `free_port` but finds a free *UDP* port by actually binding a UDP socket.
///
/// Binds `0.0.0.0:0` (same as the server) so the OS guarantees the port is
/// free for a subsequent `0.0.0.0:{port}` bind by the server.
fn free_udp_port() -> u16 {
    use std::net::UdpSocket;
    let s = UdpSocket::bind("0.0.0.0:0").unwrap();
    s.local_addr().unwrap().port()
}

fn init_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

// ─────────────────────────────────────────────────────────────────────────────
// Endpoint fixture
// ─────────────────────────────────────────────────────────────────────────────

struct Endpoint {
    pub http_port: u16,
    pub https_port: u16,
    pub udp_port: u16,
    pub udp_throughput_port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Endpoint {
    async fn start() -> Self {
        init_crypto();

        let http_port = free_port();
        let https_port = free_port();
        let udp_port = free_udp_port();
        let udp_throughput_port = free_udp_port();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let cfg = networker_endpoint::ServerConfig {
            http_port,
            https_port,
            udp_port,
            udp_throughput_port,
        };

        tokio::spawn(async move {
            networker_endpoint::run_with_shutdown(cfg, rx).await.ok();
        });

        // Wait for the HTTP server to become ready (poll up to 3 s)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{http_port}"))
                .await
                .is_ok()
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("Endpoint did not start within 10 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Also wait for HTTPS
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{https_port}"))
                .await
                .is_ok()
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("HTTPS endpoint did not start within 10 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Wait for the UDP echo server.
        //
        // Unlike TCP, there is no "connect" handshake to probe.  We send a
        // tiny echo-format packet (4-byte seq) and wait up to 100 ms for the
        // server to bounce it back.  On macOS / Linux, sending to a port with
        // nothing bound returns ICMP Port Unreachable (ECONNREFUSED), so
        // retrying until we actually get the echo is the correct readiness
        // check.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            probe
                .connect(format!("127.0.0.1:{udp_port}"))
                .await
                .unwrap();
            // seq=0, timestamp=0 — valid echo-format header
            let _ = probe.send(&[0u8; 12]).await;
            let mut buf = [0u8; 16];
            let echoed =
                tokio::time::timeout(std::time::Duration::from_millis(100), probe.recv(&mut buf))
                    .await
                    .map(|r| r.is_ok())
                    .unwrap_or(false);
            if echoed {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("UDP echo server did not start within 10 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Wait for the UDP throughput (NWKT) server to bind.
        // Same probe as the QUIC readiness check: send a byte, classify the result.
        //   ConnectionRefused → ICMP Port Unreachable → not bound yet, retry.
        //   timeout           → packet absorbed (no ICMP) → server is listening.
        //   Ok(data)          → server responded → definitely ready.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            sock.connect(format!("127.0.0.1:{udp_throughput_port}"))
                .await
                .unwrap();
            let _ = sock.send(&[0u8]).await;
            let mut buf = [0u8; 64];
            match tokio::time::timeout(std::time::Duration::from_millis(100), sock.recv(&mut buf))
                .await
            {
                Err(_timeout) => break,
                Ok(Ok(_)) => break,
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
                Ok(Err(_)) => break,
            }
            if std::time::Instant::now() >= deadline {
                panic!("UDP throughput server did not bind on port {udp_throughput_port} within 10 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        Endpoint {
            http_port,
            https_port,
            udp_port,
            udp_throughput_port,
            shutdown: Some(tx),
        }
    }

    fn http_url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("http://127.0.0.1:{}{}", self.http_port, path)).unwrap()
    }

    fn https_url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("https://127.0.0.1:{}{}", self.https_port, path)).unwrap()
    }

    /// Wait for the QUIC/HTTP3 server (UDP) to be ready.
    ///
    /// The QUIC server binds the same UDP port as the HTTPS TCP server, but is
    /// spawned concurrently and may lag behind.  We probe it the same way as the
    /// UDP echo server: send a single byte to the port and check the result.
    ///
    /// - `ConnectionRefused` → ICMP Port Unreachable → nothing bound yet, retry.
    /// - timeout            → Quinn absorbed the packet (no ICMP) → port is bound.
    /// - `Ok(data)`         → Quinn sent a response (e.g. version negotiation) → ready.
    #[cfg(feature = "http3")]
    async fn wait_for_quic(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            sock.connect(format!("127.0.0.1:{}", self.https_port))
                .await
                .unwrap();
            let _ = sock.send(&[0u8]).await;
            let mut buf = [0u8; 64];
            match tokio::time::timeout(std::time::Duration::from_millis(100), sock.recv(&mut buf))
                .await
            {
                Err(_timeout) => break, // no ICMP unreachable → Quinn is listening
                Ok(Ok(_)) => break,     // Quinn sent data back → definitely ready
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    // UDP port not bound yet — retry
                }
                Ok(Err(_)) => break, // unexpected error; let the probe handle it
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "QUIC server did not bind on UDP port {} within 5 seconds",
                    self.https_port
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn http1_health_returns_200() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: false,
        ..Default::default()
    };

    let url = ep.http_url("/health");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(
        attempt.success,
        "HTTP/1.1 probe failed: {:?}",
        attempt.error
    );
    let http = attempt.http.expect("http result missing");
    assert_eq!(http.status_code, 200);
    assert_eq!(http.negotiated_version, "HTTP/1.1");
    assert!(http.ttfb_ms >= 0.0);
    assert!(http.total_duration_ms >= http.ttfb_ms);

    // TCP timing must be present
    assert!(attempt.tcp.is_some());
    let tcp = attempt.tcp.unwrap();
    assert!(tcp.connect_duration_ms >= 0.0);
    assert!(tcp.local_addr.is_some());

    // No TLS for plain HTTP
    assert!(attempt.tls.is_none());
}

#[tokio::test]
async fn http1_echo_round_trips_payload() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        payload_size: 128,
        insecure: false,
        ..Default::default()
    };

    let url = ep.http_url("/echo");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;
    assert!(attempt.success, "Echo probe failed: {:?}", attempt.error);
    let http = attempt.http.unwrap();
    assert!(http.body_size_bytes >= 128);
}

#[tokio::test]
async fn http2_over_tls_negotiates_h2() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: true, // self-signed cert
        ..Default::default()
    };

    let url = ep.https_url("/health");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http2, &url, &cfg).await;

    assert!(attempt.success, "HTTP/2 probe failed: {:?}", attempt.error);

    let tls = attempt.tls.expect("TLS result missing for HTTPS request");
    assert_eq!(
        tls.alpn_negotiated.as_deref(),
        Some("h2"),
        "Expected ALPN 'h2', got {:?}",
        tls.alpn_negotiated
    );
    assert!(tls.handshake_duration_ms >= 0.0);

    let http = attempt.http.expect("http result missing");
    assert_eq!(http.negotiated_version, "HTTP/2");
    assert_eq!(http.status_code, 200);
}

#[tokio::test]
async fn http1_over_tls_negotiates_http11() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: true,
        ..Default::default()
    };

    let url = ep.https_url("/health");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(
        attempt.success,
        "HTTP/1.1+TLS probe failed: {:?}",
        attempt.error
    );
    let tls = attempt.tls.unwrap();
    assert_eq!(tls.alpn_negotiated.as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn tcp_only_mode_records_connect_time() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: false,
        ..Default::default()
    };

    let url = ep.http_url("/health");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Tcp, &url, &cfg).await;

    assert!(attempt.success);
    assert!(attempt.tcp.is_some());
    assert!(
        attempt.http.is_none(),
        "TCP mode should not send HTTP request"
    );
}

#[tokio::test]
async fn udp_probe_measures_rtt() {
    let ep = Endpoint::start().await;
    let cfg = UdpProbeConfig {
        target_host: "127.0.0.1".into(),
        target_port: ep.udp_port,
        probe_count: 5,
        timeout_ms: 3000,
        payload_size: 64,
    };

    let attempt = run_udp_probe(Uuid::new_v4(), 0, &cfg).await;
    assert!(attempt.success, "UDP probe failed: {:?}", attempt.error);

    let udp = attempt.udp.unwrap();
    assert_eq!(udp.probe_count, 5);
    assert!(udp.success_count > 0, "At least some probes should succeed");
    assert!(udp.rtt_avg_ms >= 0.0);
    assert!(udp.rtt_p95_ms >= udp.rtt_min_ms);
}

/// Latency-under-load (`rpm`): unloaded UDP echo baseline, then paced UDP
/// echo probes while sustained /download transfers saturate the link.
///
/// On loopback there is no real queue to bloat, so this asserts the STRUCTURE
/// of the result (both phases sampled, load actually transferred bytes, RPM
/// and factor derivable) — not the magnitude of the bufferbloat factor.
#[tokio::test]
async fn rpm_probe_reports_latency_under_load() {
    let ep = Endpoint::start().await;
    let cfg = RpmProbeConfig {
        udp: UdpProbeConfig {
            target_host: "127.0.0.1".into(),
            target_port: ep.udp_port,
            probe_count: 5,
            timeout_ms: 3000,
            payload_size: 64,
        },
        throughput: ThroughputConfig {
            run_cfg: RunConfig {
                dns_enabled: false,
                timeout_ms: 10_000,
                insecure: false,
                ..Default::default()
            },
            base_url: ep.http_url("/health"),
        },
        // Small + short: loopback moves 4 MiB in milliseconds, and the load
        // loop repeats downloads back-to-back for the whole window.
        download_bytes: 4 * 1024 * 1024,
        load_duration_ms: 1_200,
        probe_interval_ms: 100,
    };

    let attempt = run_rpm_probe(Uuid::new_v4(), 0, &cfg).await;

    assert!(attempt.success, "rpm probe failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::Rpm);
    assert_eq!(attempt.protocol.to_string(), "rpm");

    let r = attempt.rpm.expect("rpm result missing");
    // Phase 1: unloaded baseline sampled.
    assert_eq!(r.unloaded_probe_count, 5);
    assert!(r.unloaded_success_count > 0, "unloaded probes all lost");
    assert!(r.unloaded_rtt_avg_ms > 0.0);
    assert!(r.unloaded_rtt_p95_ms >= r.unloaded_rtt_min_ms);
    // Phase 2: paced probes fired during the load window.
    assert_eq!(r.loaded_probe_count, 12, "1200ms / 100ms cadence");
    assert!(r.loaded_success_count > 0, "loaded probes all lost");
    assert!(r.loaded_rtt_avg_ms > 0.0);
    assert!(r.loaded_rtt_p95_ms >= r.loaded_rtt_min_ms);
    // The load generator really saturated the link (≥1 completed download).
    assert!(r.load_downloads_completed > 0, "no download completed");
    assert!(
        r.load_bytes_transferred >= 4 * 1024 * 1024,
        "load moved only {} bytes",
        r.load_bytes_transferred
    );
    assert!(r.load_duration_ms >= 1_000.0, "load window cut short");
    assert!(
        r.load_throughput_mbps.unwrap_or(0.0) > 0.0,
        "load throughput should be measured"
    );
    // Derived headline metrics exist and are internally consistent.
    let rpm = r.rpm.expect("rpm should be derivable");
    assert!(
        (rpm - 60_000.0 / r.loaded_rtt_avg_ms).abs() < 1e-6,
        "rpm must equal 60000 / loaded avg RTT"
    );
    let factor = r.bufferbloat_factor.expect("factor should be derivable");
    assert!(
        (factor - r.loaded_rtt_avg_ms / r.unloaded_rtt_avg_ms).abs() < 1e-6,
        "factor must equal loaded/unloaded avg"
    );
    assert!(factor > 0.0);
}

#[tokio::test]
async fn http1_delay_endpoint_respected() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: false,
        ..Default::default()
    };

    let url = ep.http_url("/delay?ms=100");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(attempt.success);
    let http = attempt.http.unwrap();
    // Two-sided timing-accuracy bound (trust audit T1): the server delays
    // 100 ms, so TTFB must be ≥ ~100 ms AND must not be grossly inflated by
    // self-measurement overhead. The +400 ms upper slack absorbs CI jitter
    // while still catching systematic inflation (e.g. setup work timed as
    // network time, as in audit finding V5).
    assert!(
        http.ttfb_ms >= 90.0, // slight tolerance for CI
        "TTFB was {:.1}ms, expected ≥100ms",
        http.ttfb_ms
    );
    assert!(
        http.ttfb_ms <= 500.0,
        "TTFB was {:.1}ms for a 100ms delay — measurement is inflated",
        http.ttfb_ms
    );
    assert!(
        http.total_duration_ms <= 600.0,
        "total was {:.1}ms for a 100ms delay on loopback — measurement is inflated",
        http.total_duration_ms
    );
}

#[tokio::test]
async fn http1_status_endpoint_returns_correct_code() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: false,
        ..Default::default()
    };

    let url = ep.http_url("/status/404");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    let http = attempt.http.expect("http result missing");
    assert_eq!(http.status_code, 404);
}

// ─────────────────────────────────────────────────────────────────────────────
// Error-path classification (T3) and unified HTTP success rule (V6)
// ─────────────────────────────────────────────────────────────────────────────

/// Nothing listens on a freshly freed port — the probe must fail in the TCP
/// phase with `ErrorCategory::Tcp` and no partial HttpResult.
#[tokio::test]
async fn connection_refused_classified_as_tcp() {
    init_crypto();
    let port = free_port(); // bound then dropped — nothing listening
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 3000,
        ..Default::default()
    };
    let url = url::Url::parse(&format!("http://127.0.0.1:{port}/health")).unwrap();
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(!attempt.success, "probe to a closed port must fail");
    let err = attempt.error.expect("error must be set");
    assert_eq!(
        err.category,
        ErrorCategory::Tcp,
        "connection refused must be classified Tcp, got {:?} ({})",
        err.category,
        err.message
    );
    assert!(
        attempt.http.is_none(),
        "no HttpResult may leak from a refused connection"
    );
}

/// The endpoint delays 3 s before responding while the probe timeout is
/// 300 ms — the failure must be classified `Timeout`, not `Http`, and no
/// partial HttpResult may leak into statistics.
#[tokio::test]
async fn request_timeout_classified_as_timeout() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 300,
        ..Default::default()
    };
    let url = ep.http_url("/delay?ms=3000");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(!attempt.success, "delayed response must exceed the timeout");
    let err = attempt.error.expect("error must be set");
    assert_eq!(
        err.category,
        ErrorCategory::Timeout,
        "request timeout must be classified Timeout, got {:?} ({})",
        err.category,
        err.message
    );
    assert!(
        attempt.http.is_none(),
        "no partial HttpResult may leak into stats on timeout"
    );
}

/// An `https://` probe pointed at the plain-HTTP port fails the TLS handshake
/// (the server answers the ClientHello with plaintext HTTP) — must be `Tls`.
#[tokio::test]
async fn tls_to_plain_http_port_classified_as_tls() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: true, // rule out certificate verification as the cause
        ..Default::default()
    };
    let url = url::Url::parse(&format!("https://127.0.0.1:{}/health", ep.http_port)).unwrap();
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(!attempt.success, "TLS to a plaintext port must fail");
    let err = attempt.error.expect("error must be set");
    assert_eq!(
        err.category,
        ErrorCategory::Tls,
        "handshake against a plaintext port must be classified Tls, got {:?} ({})",
        err.category,
        err.message
    );
    // TCP connect succeeded — that phase should still be recorded.
    assert!(
        attempt.tcp.is_some(),
        "TCP phase result should be preserved"
    );
}

/// The endpoint serves a self-signed certificate; without `--insecure` the
/// verification failure must be classified `Tls`.
#[tokio::test]
async fn tls_cert_verification_failure_classified_as_tls() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: false, // verify the self-signed cert → must fail
        ..Default::default()
    };
    let url = ep.https_url("/health");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(!attempt.success, "self-signed cert must fail verification");
    let err = attempt.error.expect("error must be set");
    assert_eq!(
        err.category,
        ErrorCategory::Tls,
        "certificate verification failure must be classified Tls, got {:?} ({})",
        err.category,
        err.message
    );
}

/// 5xx responses are failures with `ErrorCategory::Http`; the HttpResult
/// (status, timings) must still be captured for diagnosis.
#[tokio::test]
async fn http_500_is_failure_with_http_category() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        ..Default::default()
    };
    let url = ep.http_url("/status/500");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(!attempt.success, "HTTP 500 must be a failed attempt");
    let http = attempt.http.expect("HttpResult must still be present");
    assert_eq!(http.status_code, 500);
    let err = attempt.error.expect("error must be set");
    assert_eq!(err.category, ErrorCategory::Http);
    assert!(
        err.message.contains("500"),
        "error message should carry the status, got: {}",
        err.message
    );
}

/// Pins the unified success rule (V6): 4xx is a failure on http1 and http2
/// alike (2xx/3xx = success, i.e. status < 400 — same rule as native/curl).
#[tokio::test]
async fn http_4xx_is_failure_across_modes() {
    let ep = Endpoint::start().await;

    let cfg_h1 = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        ..Default::default()
    };
    let attempt = run_probe(
        Uuid::new_v4(),
        0,
        Protocol::Http1,
        &ep.http_url("/status/404"),
        &cfg_h1,
    )
    .await;
    assert!(!attempt.success, "http1: 404 must be a failure");
    assert_eq!(attempt.http.expect("http result").status_code, 404);
    assert_eq!(
        attempt.error.expect("error must be set").category,
        ErrorCategory::Http
    );

    let cfg_h2 = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        insecure: true,
        ..Default::default()
    };
    let attempt2 = run_probe(
        Uuid::new_v4(),
        0,
        Protocol::Http2,
        &ep.https_url("/status/404"),
        &cfg_h2,
    )
    .await;
    assert!(!attempt2.success, "http2: 404 must be a failure");
    assert_eq!(attempt2.http.expect("http result").status_code, 404);
}

/// 3xx responses count as success under the unified rule (redirects are
/// recorded, not followed).
#[tokio::test]
async fn http_3xx_counts_as_success() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5000,
        ..Default::default()
    };
    let url = ep.http_url("/status/301");
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::Http1, &url, &cfg).await;

    assert!(attempt.success, "3xx is success: {:?}", attempt.error);
    let http = attempt.http.expect("http result missing");
    assert_eq!(http.status_code, 301);
}

// ─────────────────────────────────────────────────────────────────────────────
// Throughput probes
// ─────────────────────────────────────────────────────────────────────────────

/// GET /download?bytes=65536 → endpoint streams 64 KiB of zeros.
/// Verifies the full download probe pipeline: URL rewriting, payload delivery,
/// throughput field population.
#[tokio::test]
async fn download_probe_reports_throughput() {
    let ep = Endpoint::start().await;
    let cfg = ThroughputConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: false,
            ..Default::default()
        },
        base_url: ep.http_url("/health"),
    };

    let attempt = run_download_probe(Uuid::new_v4(), 0, 65_536, &cfg).await;

    assert!(
        attempt.success,
        "download probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::Download);

    let http = attempt.http.expect("http result missing");
    assert_eq!(
        http.payload_bytes, 65_536,
        "payload_bytes should equal requested download size"
    );
    assert_eq!(http.status_code, 200);

    let mbps = http
        .throughput_mbps
        .expect("throughput_mbps should be Some for a successful download");
    assert!(mbps > 0.0, "throughput should be positive, got {mbps}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Page-load probes
// ─────────────────────────────────────────────────────────────────────────────

/// HTTP/1.1 page-load: fetches `/page` manifest then downloads N assets
/// over up to 6 parallel keep-alive connections.
#[tokio::test]
async fn pageload_h1_fetches_assets() {
    let ep = Endpoint::start().await;
    let cfg = PageLoadConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: false,
            ..Default::default()
        },
        base_url: ep.http_url("/health"),
        asset_sizes: vec![1024; 5], // 5 × 1 KB assets
        preset_name: None,
    };

    let attempt = run_pageload_probe(Uuid::new_v4(), 0, &cfg).await;

    assert!(attempt.success, "pageload H1 failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::PageLoad);

    let pl = attempt.page_load.expect("page_load result missing");
    assert_eq!(pl.asset_count, 5, "should have 5 assets");
    assert!(pl.assets_fetched > 0, "should have fetched some assets");
    assert!(pl.total_ms > 0.0);
    assert!(pl.total_bytes > 0);
    assert!(pl.connections_opened >= 1);
}

/// HTTP/2 page-load: all assets multiplexed over a single TLS connection.
#[tokio::test]
async fn pageload_h2_multiplexes_assets() {
    let ep = Endpoint::start().await;
    let cfg = PageLoadConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: true, // self-signed cert
            ..Default::default()
        },
        base_url: ep.https_url("/health"),
        asset_sizes: vec![1024; 5],
        preset_name: None,
    };

    let attempt = run_pageload2_probe(Uuid::new_v4(), 0, &cfg).await;

    assert!(attempt.success, "pageload H2 failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::PageLoad2);

    let pl = attempt.page_load.expect("page_load result missing");
    assert_eq!(pl.asset_count, 5);
    assert!(pl.assets_fetched > 0);
    assert_eq!(pl.connections_opened, 1, "H2 uses a single connection");
    assert!(pl.tls_setup_ms >= 0.0);
}

/// HTTP/3 page-load: all assets multiplexed over a single QUIC connection.
#[cfg(feature = "http3")]
#[tokio::test]
async fn pageload_h3_multiplexes_assets() {
    let ep = Endpoint::start().await;
    // Wait for the QUIC server to bind its UDP port before probing.
    ep.wait_for_quic().await;

    let cfg = PageLoadConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: true,
            ..Default::default()
        },
        base_url: ep.https_url("/health"),
        asset_sizes: vec![1024; 5],
        preset_name: None,
    };

    let attempt = run_pageload3_probe(Uuid::new_v4(), 0, &cfg).await;

    assert!(attempt.success, "pageload H3 failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::PageLoad3);

    let pl = attempt.page_load.expect("page_load result missing");
    assert_eq!(pl.asset_count, 5);
    assert!(pl.assets_fetched > 0);
    assert_eq!(pl.connections_opened, 1, "H3 uses a single QUIC connection");
}

/// POST /upload with a 64 KiB body → endpoint reads and acknowledges it.
/// Verifies the full upload probe pipeline: URL rewriting, body send,
/// throughput field population using TTFB as the time window.
#[tokio::test]
async fn upload_probe_reports_throughput() {
    let ep = Endpoint::start().await;
    let cfg = ThroughputConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: false,
            ..Default::default()
        },
        base_url: ep.http_url("/health"),
    };

    let attempt = run_upload_probe(Uuid::new_v4(), 0, 65_536, &cfg).await;

    assert!(attempt.success, "upload probe failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::Upload);

    let http = attempt.http.expect("http result missing");
    assert_eq!(
        http.payload_bytes, 65_536,
        "payload_bytes should equal the uploaded body size"
    );
    assert_eq!(http.status_code, 200);

    let mbps = http
        .throughput_mbps
        .expect("throughput_mbps should be Some for a successful upload");
    assert!(mbps > 0.0, "throughput should be positive, got {mbps}");
}

/// GET `/download?bytes=65536` using the `webdownload` protocol label.
/// Identical URL construction to `download` — differs only in the protocol
/// recorded on the attempt (enables side-by-side comparison in reports).
#[tokio::test]
async fn webdownload_probe_reports_throughput() {
    let ep = Endpoint::start().await;
    let cfg = ThroughputConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: false,
            ..Default::default()
        },
        base_url: ep.http_url("/health"),
    };

    let attempt = run_webdownload_probe(Uuid::new_v4(), 0, 65_536, &cfg).await;

    assert!(
        attempt.success,
        "webdownload probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::WebDownload);

    let http = attempt.http.expect("http result missing");
    assert_eq!(http.payload_bytes, 65_536);
    assert_eq!(http.status_code, 200);
    assert!(
        http.throughput_mbps.unwrap_or(0.0) > 0.0,
        "throughput should be positive"
    );
}

/// POST a 64 KiB body using the `webupload` protocol label.
/// Same upload mechanics as `upload` — protocol label differs for report comparison.
#[tokio::test]
async fn webupload_probe_reports_throughput() {
    let ep = Endpoint::start().await;
    let cfg = ThroughputConfig {
        run_cfg: RunConfig {
            dns_enabled: false,
            timeout_ms: 10_000,
            insecure: false,
            ..Default::default()
        },
        base_url: ep.http_url("/health"),
    };

    let attempt = run_webupload_probe(Uuid::new_v4(), 0, 65_536, &cfg).await;

    assert!(
        attempt.success,
        "webupload probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::WebUpload);

    let http = attempt.http.expect("http result missing");
    assert_eq!(http.payload_bytes, 65_536);
    assert_eq!(http.status_code, 200);
    assert!(
        http.throughput_mbps.unwrap_or(0.0) > 0.0,
        "throughput should be positive"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// UDP throughput probes
// ─────────────────────────────────────────────────────────────────────────────

/// Send CMD_DOWNLOAD to the NWKT server and receive 64 KiB of datagrams.
/// Verifies the full UDP download pipeline: handshake, data transfer, throughput.
#[tokio::test]
async fn udpdownload_probe_reports_throughput() {
    let ep = Endpoint::start().await;
    let cfg = UdpThroughputConfig {
        target_host: "127.0.0.1".into(),
        target_port: ep.udp_throughput_port,
        timeout_ms: 10_000,
    };

    let attempt = run_udpdownload_probe(Uuid::new_v4(), 0, 65_536, &cfg).await;

    assert!(
        attempt.success,
        "udpdownload probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::UdpDownload);

    let ut = attempt
        .udp_throughput
        .expect("udp_throughput result missing");
    assert_eq!(ut.payload_bytes, 65_536);
    assert!(
        ut.datagrams_received.unwrap_or(0) > 0,
        "should have received datagrams"
    );
    assert!(
        ut.throughput_mbps.unwrap_or(0.0) > 0.0,
        "throughput should be positive"
    );
}

/// Send CMD_UPLOAD to the NWKT server with a 64 KiB payload.
/// Verifies CMD_REPORT bytes_acked matches the sent size and throughput is measured.
#[tokio::test]
async fn udpupload_probe_reports_throughput() {
    let ep = Endpoint::start().await;
    let cfg = UdpThroughputConfig {
        target_host: "127.0.0.1".into(),
        target_port: ep.udp_throughput_port,
        timeout_ms: 10_000,
    };

    let attempt = run_udpupload_probe(Uuid::new_v4(), 0, 65_536, &cfg).await;

    assert!(
        attempt.success,
        "udpupload probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::UdpUpload);

    let ut = attempt
        .udp_throughput
        .expect("udp_throughput result missing");
    assert_eq!(ut.payload_bytes, 65_536);
    assert!(ut.datagrams_sent > 0, "should have sent datagrams");
    // Server reports bytes_acked via CMD_REPORT
    assert_eq!(
        ut.bytes_acked,
        Some(65_536),
        "server should have acknowledged all bytes"
    );
    // Loss accuracy (trust audit V3): a clean loopback run must report ~0%
    // loss, and that figure must be DERIVED from the server's CMD_REPORT byte
    // count — not assumed. With bytes_acked == payload_bytes the derived loss
    // is exactly 0.
    assert_eq!(
        ut.loss_percent, 0.0,
        "loopback upload should report 0% loss (derived from CMD_REPORT)"
    );
    // The client cannot know the received datagram count for uploads — it
    // must be reported as unknown, never fabricated as `sent` (audit V3).
    assert_eq!(
        ut.datagrams_received, None,
        "upload datagrams_received must be unknown, not fabricated"
    );
    assert!(
        ut.throughput_mbps.unwrap_or(0.0) > 0.0,
        "throughput should be positive"
    );
    // Transfer-window accuracy (trust audit V4): 64 KiB on loopback takes
    // milliseconds; the window must not include the CMD_REPORT round-trip
    // wait (timeout_ms is 10s here — any report stall would blow this bound).
    assert!(
        ut.transfer_ms < 2_000.0,
        "transfer window {}ms is inflated — does it include the CMD_REPORT wait?",
        ut.transfer_ms
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// DNS and TLS probes
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve `localhost` → should return at least one IP (127.0.0.1 or ::1).
/// Verifies the probe returns a populated DnsResult with no TCP/TLS/HTTP.
#[tokio::test]
async fn dns_probe_resolves_localhost() {
    init_crypto();

    let attempt = run_dns_probe(Uuid::new_v4(), 0, "localhost", false, false).await;

    assert!(attempt.success, "DNS probe failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::Dns);

    let dns = attempt.dns.expect("dns result missing");
    assert_eq!(dns.query_name, "localhost");
    assert!(
        !dns.resolved_ips.is_empty(),
        "should have resolved at least one IP"
    );
    assert!(dns.duration_ms >= 0.0);
    // Trust audit V1: the resolver identity must be recorded so the report
    // states which resolver produced the timing (system vs labeled fallback).
    let resolver = dns.resolver.expect("resolver identity must be recorded");
    assert!(
        resolver.starts_with("system") || resolver.contains("fallback"),
        "resolver must be the system resolver or a labeled fallback, got: {resolver}"
    );

    // Standalone DNS probe — no TCP/TLS/HTTP phases
    assert!(
        attempt.tcp.is_none(),
        "DNS probe should not open a TCP connection"
    );
    assert!(attempt.tls.is_none());
    assert!(attempt.http.is_none());
}

/// TLS handshake against the local HTTPS endpoint.
/// Verifies the cert chain is captured (SANs include 127.0.0.1), cipher suite
/// and TLS version are populated, and no HTTP request is made.
#[tokio::test]
async fn tls_probe_captures_cert_chain() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: true, // self-signed cert
        ..Default::default()
    };

    let url = ep.https_url("/health");
    let attempt = run_tls_probe(Uuid::new_v4(), 0, &url, &cfg).await;

    assert!(attempt.success, "TLS probe failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::Tls);

    let tls = attempt.tls.expect("tls result missing");
    assert!(tls.success);
    assert!(tls.handshake_duration_ms >= 0.0);
    assert!(
        !tls.protocol_version.is_empty(),
        "protocol_version should be populated"
    );
    assert!(
        !tls.cipher_suite.is_empty(),
        "cipher_suite should be populated"
    );

    // The endpoint advertises h2 + http/1.1; one should be negotiated
    assert!(
        tls.alpn_negotiated.is_some(),
        "ALPN should be negotiated on HTTPS"
    );

    // Full cert chain should be captured (rcgen self-signed = 1 cert)
    assert!(
        !tls.cert_chain.is_empty(),
        "cert_chain should contain at least the leaf cert"
    );
    let leaf = &tls.cert_chain[0];

    // The endpoint cert includes 127.0.0.1 as a SAN
    assert!(
        leaf.sans.iter().any(|s| s == "127.0.0.1"),
        "leaf cert SANs should include 127.0.0.1, got: {:?}",
        leaf.sans
    );

    // Standalone TLS probe — no HTTP request
    assert!(
        attempt.http.is_none(),
        "TLS probe should not send an HTTP request"
    );
}

/// Timing-accuracy regression test for trust-audit V5: TLS handshake time on
/// loopback must be well under a sane bound. Runs the standalone TLS probe
/// with `insecure: false` and a CA bundle so the FULL trust-store path
/// (webpki roots + OS native certs + bundle file I/O) is exercised — that
/// setup work used to run inside the handshake stopwatch and inflated
/// reported TLS times by the cost of an OS keychain/disk read per attempt.
#[tokio::test]
async fn tls_handshake_time_excludes_trust_store_construction() {
    init_crypto();

    // Self-signed end-entity cert for 127.0.0.1; used both as the server cert
    // and as the client's trust anchor (rustls accepts a self-signed leaf in
    // the root store, but rejects CA-flagged certs presented as end-entity).
    let key_pair = rcgen::KeyPair::generate().expect("keypair");
    let params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).expect("cert params");
    let cert = params.self_signed(&key_pair).expect("self-signed cert");

    // Write the cert PEM to a temp CA bundle for the client trust store.
    let mut bundle = tempfile::NamedTempFile::new().expect("temp file");
    std::io::Write::write_all(&mut bundle, cert.pem().as_bytes()).expect("write pem");

    // Minimal in-test TLS server: accept connections, complete the handshake.
    let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
    let key_der = rustls::pki_types::PrivateKeyDer::try_from(key_pair.serialize_der())
        .expect("private key der");
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("server config");
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(server_config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    // Drain until the client closes so close_notify is clean.
                    let mut buf = [0u8; 256];
                    let _ = tokio::io::AsyncReadExt::read(&mut tls, &mut buf).await;
                }
            });
        }
    });

    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: false,
        ca_bundle: Some(bundle.path().to_string_lossy().into_owned()),
        ..Default::default()
    };
    let url = url::Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap();

    // Warm-up attempt: populates the process-wide trust-store cache so the
    // measured attempt reflects steady-state behavior.
    let _ = run_tls_probe(Uuid::new_v4(), 0, &url, &cfg).await;

    let attempt = run_tls_probe(Uuid::new_v4(), 1, &url, &cfg).await;
    assert!(attempt.success, "TLS probe failed: {:?}", attempt.error);
    let tls = attempt.tls.expect("tls result missing");
    assert!(
        tls.handshake_duration_ms > 0.0,
        "handshake time must be positive"
    );
    assert!(
        tls.handshake_duration_ms < 100.0,
        "loopback TLS handshake took {:.1}ms — trust-store construction is \
         leaking into the handshake timing window (audit V5)",
        tls.handshake_duration_ms
    );
}

/// TLS resumption probe against the local HTTPS endpoint.
/// Verifies the first connection is full, the second is resumed, and a real
/// HTTP request succeeds on both connections.
#[tokio::test]
async fn tls_resumption_probe_resumes_second_handshake() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: true,
        ..Default::default()
    };

    let url = ep.https_url("/health");
    let attempt = run_tls_resumption_probe(Uuid::new_v4(), 0, &url, &cfg).await;

    assert!(
        attempt.success,
        "TLS resumption probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::TlsResume);

    let tls = attempt.tls.expect("tls result missing");
    assert_eq!(tls.previous_handshake_kind.as_deref(), Some("full"));
    assert_eq!(tls.handshake_kind.as_deref(), Some("resumed"));
    assert_eq!(tls.resumed, Some(true));
    assert_eq!(tls.previous_http_status_code, Some(200));
    assert_eq!(tls.http_status_code, Some(200));
    assert!(tls.previous_handshake_duration_ms.unwrap_or_default() >= 0.0);
    assert!(tls.handshake_duration_ms >= 0.0);
    assert!(tls.tls13_tickets_received.unwrap_or_default() >= 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Native-TLS and curl probes
// ─────────────────────────────────────────────────────────────────────────────

/// Native-TLS probe against the local HTTPS endpoint.
///
/// Without `--features native` the probe returns a graceful stub error — this
/// test verifies that behavior.  With the feature enabled it verifies the full
/// pipeline: DNS + TCP + TLS (platform backend) + HTTP/1.1.
#[tokio::test]
async fn native_probe_uses_platform_tls() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: true, // self-signed cert
        ..Default::default()
    };

    let url = ep.https_url("/health");
    let attempt = run_native_probe(Uuid::new_v4(), 0, &url, &cfg).await;

    if cfg!(not(feature = "native")) {
        // Without the feature the probe returns a compile-time stub error.
        assert!(!attempt.success, "stub should report failure");
        let msg = attempt.error.expect("stub should set error").message;
        assert!(
            msg.contains("--features native"),
            "stub error should mention the feature flag, got: {msg}"
        );
        return;
    }

    // Feature is enabled — expect a real successful probe.
    assert!(attempt.success, "native probe failed: {:?}", attempt.error);
    assert_eq!(attempt.protocol, Protocol::Native);

    let http = attempt.http.expect("http result missing");
    assert_eq!(http.status_code, 200);
    assert!(http.ttfb_ms >= 0.0);

    let tls = attempt.tls.expect("tls result missing");
    assert!(tls.success);
    assert!(tls.handshake_duration_ms >= 0.0);
    assert!(
        tls.tls_backend
            .as_deref()
            .map(|b| b.starts_with("native/"))
            .unwrap_or(false),
        "tls_backend should start with 'native/', got: {:?}",
        tls.tls_backend
    );
}

/// Curl probe against the local HTTP endpoint.
///
/// Self-skips gracefully when `curl` is not found on `$PATH`; verifies the
/// "not found" error in that case.  When curl is present it checks HTTP timing
/// and status code.
#[tokio::test]
async fn curl_probe_measures_timing() {
    let ep = Endpoint::start().await;
    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: false,
        ..Default::default()
    };

    let url = ep.http_url("/health");
    let attempt = run_curl_probe(Uuid::new_v4(), 0, &url, &cfg).await;

    // If curl is not installed, the probe returns a graceful error — not a test failure.
    if !attempt.success {
        if let Some(ref e) = attempt.error {
            if e.message.contains("curl binary not found") {
                return; // curl not on PATH — skip gracefully
            }
        }
        panic!("curl probe failed unexpectedly: {:?}", attempt.error);
    }

    assert_eq!(attempt.protocol, Protocol::Curl);

    let http = attempt.http.expect("http result missing");
    assert_eq!(http.status_code, 200);
    assert!(http.total_duration_ms > 0.0);
    assert!(http.ttfb_ms >= 0.0);

    // curl connects to 127.0.0.1 (IP) — dns_ms may be zero; TCP must be present
    assert!(
        attempt.tcp.is_some(),
        "TCP result should be present for HTTP probe"
    );
    // Plain HTTP — no TLS result
    assert!(attempt.tls.is_none(), "no TLS expected for plain HTTP");
}

// ─────────────────────────────────────────────────────────────────────────────
// sdkprobe — LagHound network-vs-server split
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal LagHound-contract mock: a raw HTTP/1.1 server on 127.0.0.1 that
/// serves `GET /laghound/echo` ONLY when `X-LagHound-Token: <token>` matches,
/// answering `200 {"contract":"v1","ok":true}` with a
/// `Server-Timing: app;dur=<app_ms>, total;dur=<app_ms>` header after sleeping
/// `app_ms` to make the server-processing leg real. Any other path, or a
/// missing/wrong token, gets a bare 404 (contract §5). Returns the bound port.
async fn start_laghound_mock(token: &'static str, app_ms: u64) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let has_echo = first_line.contains("/laghound/echo");
                let has_token = req.lines().any(|l| {
                    l.to_ascii_lowercase().starts_with("x-laghound-token:")
                        && l.trim_end().ends_with(token)
                });
                let resp = if has_echo && has_token {
                    // Simulate server-side processing so the split is non-trivial.
                    tokio::time::sleep(std::time::Duration::from_millis(app_ms)).await;
                    let body = "{\"contract\":\"v1\",\"ok\":true}";
                    format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {len}\r\n\
                         Cache-Control: no-store, no-cache, must-revalidate\r\n\
                         Server-Timing: app;dur={app}.0, total;dur={app}.0\r\n\
                         Connection: close\r\n\r\n{body}",
                        len = body.len(),
                        app = app_ms,
                        body = body,
                    )
                } else {
                    // Bare 404 — no LagHound headers, no envelope (contract §5).
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                };
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    // Readiness: wait until the listener accepts a TCP connection.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("LagHound mock did not start within 5 seconds");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    port
}

/// sdkprobe against a valid LagHound endpoint computes the network/server split:
/// server_ms == the reported `app;dur`, and server_ms + network_ms ≈ wall
/// (TTFB), with network_ms ≥ 0.
#[tokio::test]
async fn sdkprobe_computes_network_server_split() {
    init_crypto();
    let app_ms = 40;
    let port = start_laghound_mock("valid-token", app_ms).await;

    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: false,
        laghound_token: Some("valid-token".to_string()),
        laghound_route: "/laghound/echo".to_string(),
        ..Default::default()
    };

    // The sdkprobe overrides the request path with laghound_route, so the target
    // path here is irrelevant — use "/".
    let url = url::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::SdkProbe, &url, &cfg).await;

    assert!(
        attempt.success,
        "sdkprobe should succeed against a valid LagHound endpoint: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::SdkProbe);

    let http = attempt.http.as_ref().expect("http result missing");
    assert_eq!(http.status_code, 200);

    let st = attempt
        .server_timing
        .as_ref()
        .expect("server_timing must be present (Server-Timing header parsed)");

    // app;dur was parsed.
    let app = st.app_ms.expect("app_ms must be parsed from Server-Timing");
    assert!(
        (app - app_ms as f64).abs() < 1e-6,
        "app_ms should equal the reported app;dur ({app_ms}), got {app}"
    );

    // server_ms == app (app takes precedence over total).
    let server = st.server_ms.expect("server_ms must be computed");
    assert!(
        (server - app).abs() < 1e-6,
        "server_ms ({server}) must equal app ({app})"
    );

    // network_ms is non-negative and equals ttfb − server.
    let network = st.network_ms.expect("network_ms must be computed");
    assert!(
        network >= 0.0,
        "network_ms must be non-negative, got {network}"
    );
    assert!(
        !st.split_anomaly,
        "no split anomaly expected: server_ms ({server}) must not exceed ttfb ({})",
        http.ttfb_ms
    );

    // server_ms + network_ms ≈ wall (TTFB). The mock sleeps app_ms server-side,
    // so ttfb ≥ app; the split must reconstruct the wall within a small epsilon.
    let reconstructed = server + network;
    assert!(
        (reconstructed - http.ttfb_ms).abs() < 1.0,
        "server_ms ({server}) + network_ms ({network}) = {reconstructed} \
         must reconstruct TTFB ({})",
        http.ttfb_ms
    );

    // The server leg must dominate here (mock sleeps 40ms; loopback network is
    // sub-millisecond), proving the split actually attributes time correctly.
    assert!(
        server > network,
        "server leg ({server}ms) should dominate the loopback network leg ({network}ms)"
    );
}

/// sdkprobe against a LagHound endpoint with a missing/bad token gets the
/// contract's bare 404 and is classified as a config/auth error with the
/// actionable message — not a generic HTTP 404.
#[tokio::test]
async fn sdkprobe_bad_token_is_config_error() {
    init_crypto();
    let port = start_laghound_mock("valid-token", 5).await;

    let cfg = RunConfig {
        dns_enabled: false,
        timeout_ms: 5_000,
        insecure: false,
        laghound_token: Some("WRONG-token".to_string()),
        laghound_route: "/laghound/echo".to_string(),
        ..Default::default()
    };

    let url = url::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
    let attempt = run_probe(Uuid::new_v4(), 0, Protocol::SdkProbe, &url, &cfg).await;

    assert!(!attempt.success, "bad-token sdkprobe must not succeed");
    let http = attempt.http.as_ref().expect("http result present on 404");
    assert_eq!(http.status_code, 404);

    let err = attempt.error.as_ref().expect("error record present");
    assert_eq!(
        err.category,
        ErrorCategory::Config,
        "bad-token 404 must be a config error, not {:?}",
        err.category
    );
    assert!(
        err.message.contains("SDK endpoint returned 404"),
        "message must be the actionable SDK 404 hint, got: {}",
        err.message
    );
}
