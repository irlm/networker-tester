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
use networker_tester::runner::http::{run_probe, RunConfig};
#[cfg(feature = "http3")]
use networker_tester::runner::pageload::run_pageload3_probe;
use networker_tester::runner::pageload::{run_pageload2_probe, run_pageload_probe, PageLoadConfig};
use networker_tester::runner::throughput::{
    run_download_probe, run_upload_probe, ThroughputConfig,
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
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Endpoint {
    async fn start() -> Self {
        init_crypto();

        let http_port = free_port();
        let https_port = free_port();
        let udp_port = free_port();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let cfg = networker_endpoint::ServerConfig {
            http_port,
            https_port,
            udp_port,
            udp_throughput_port: free_port(),
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

        Endpoint {
            http_port,
            https_port,
            udp_port,
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
    /// The HTTP/3 server is spawned concurrently with the TCP servers and binds
    /// immediately.  A brief sleep after TCP readiness is sufficient on loopback.
    #[cfg(feature = "http3")]
    async fn wait_for_quic(&self) {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
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
