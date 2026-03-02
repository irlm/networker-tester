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
use networker_tester::metrics::Protocol;
use networker_tester::runner::dns::run_dns_probe;
use networker_tester::runner::http::{run_probe, RunConfig};
use networker_tester::runner::tls::run_tls_probe;
#[cfg(feature = "http3")]
use networker_tester::runner::pageload::run_pageload3_probe;
use networker_tester::runner::pageload::{run_pageload2_probe, run_pageload_probe, PageLoadConfig};
use networker_tester::runner::throughput::{
    run_download_probe, run_upload_probe, run_webdownload_probe, run_webupload_probe,
    ThroughputConfig,
};
use networker_tester::runner::udp_throughput::{
    run_udpdownload_probe, run_udpupload_probe, UdpThroughputConfig,
};
use networker_tester::runner::udp::{run_udp_probe, UdpProbeConfig};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn free_port() -> u16 {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
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
        let udp_port = free_port();
        let udp_throughput_port = free_port();

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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{http_port}"))
                .await
                .is_ok()
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("Endpoint did not start within 3 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Also wait for HTTPS
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{https_port}"))
                .await
                .is_ok()
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("HTTPS endpoint did not start within 3 seconds");
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
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
                panic!("UDP echo server did not start within 3 seconds");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Wait for the UDP throughput (NWKT) server to bind.
        // Same probe as the QUIC readiness check: send a byte, classify the result.
        //   ConnectionRefused → ICMP Port Unreachable → not bound yet, retry.
        //   timeout           → packet absorbed (no ICMP) → server is listening.
        //   Ok(data)          → server responded → definitely ready.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            sock.connect(format!("127.0.0.1:{udp_throughput_port}"))
                .await
                .unwrap();
            let _ = sock.send(&[0u8]).await;
            let mut buf = [0u8; 64];
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                sock.recv(&mut buf),
            )
            .await
            {
                Err(_timeout) => break,
                Ok(Ok(_)) => break,
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
                Ok(Err(_)) => break,
            }
            if std::time::Instant::now() >= deadline {
                panic!("UDP throughput server did not bind on port {udp_throughput_port} within 3 seconds");
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
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                sock.recv(&mut buf),
            )
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
    // Server delays 100ms, so TTFB should be ≥ 100ms
    assert!(
        http.ttfb_ms >= 90.0, // slight tolerance for CI
        "TTFB was {:.1}ms, expected ≥100ms",
        http.ttfb_ms
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

    let ut = attempt.udp_throughput.expect("udp_throughput result missing");
    assert_eq!(ut.payload_bytes, 65_536);
    assert!(ut.datagrams_received > 0, "should have received datagrams");
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

    let ut = attempt.udp_throughput.expect("udp_throughput result missing");
    assert_eq!(ut.payload_bytes, 65_536);
    assert!(ut.datagrams_sent > 0, "should have sent datagrams");
    // Server reports bytes_acked via CMD_REPORT
    assert_eq!(
        ut.bytes_acked,
        Some(65_536),
        "server should have acknowledged all bytes"
    );
    assert!(
        ut.throughput_mbps.unwrap_or(0.0) > 0.0,
        "throughput should be positive"
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

    assert!(
        attempt.success,
        "DNS probe failed: {:?}",
        attempt.error
    );
    assert_eq!(attempt.protocol, Protocol::Dns);

    let dns = attempt.dns.expect("dns result missing");
    assert_eq!(dns.query_name, "localhost");
    assert!(
        !dns.resolved_ips.is_empty(),
        "should have resolved at least one IP"
    );
    assert!(dns.duration_ms >= 0.0);

    // Standalone DNS probe — no TCP/TLS/HTTP phases
    assert!(attempt.tcp.is_none(), "DNS probe should not open a TCP connection");
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

    assert!(
        attempt.success,
        "TLS probe failed: {:?}",
        attempt.error
    );
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
    assert!(attempt.http.is_none(), "TLS probe should not send an HTTP request");
}
