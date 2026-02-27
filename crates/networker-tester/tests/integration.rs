/// Integration tests for networker-tester.
///
/// These tests start the `networker-endpoint` in-process and exercise the
/// full probe pipeline (DNS → TCP → TLS → HTTP → collect).
///
/// Run with:
///   cargo test --test integration -p networker-tester
use networker_tester::runner::http::{run_probe, RunConfig};
use networker_tester::runner::udp::{run_udp_probe, UdpProbeConfig};
use networker_tester::metrics::Protocol;
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
    pub http_port:  u16,
    pub https_port: u16,
    pub udp_port:   u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Endpoint {
    async fn start() -> Self {
        init_crypto();

        let http_port  = free_port();
        let https_port = free_port();
        let udp_port   = free_port();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let cfg = networker_endpoint::ServerConfig {
            http_port,
            https_port,
            udp_port,
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

        Endpoint { http_port, https_port, udp_port, shutdown: Some(tx) }
    }

    fn http_url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("http://127.0.0.1:{}{}", self.http_port, path)).unwrap()
    }

    fn https_url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("https://127.0.0.1:{}{}", self.https_port, path)).unwrap()
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

    assert!(attempt.success, "HTTP/1.1 probe failed: {:?}", attempt.error);
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

    assert!(attempt.success, "HTTP/1.1+TLS probe failed: {:?}", attempt.error);
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
    assert!(attempt.http.is_none(), "TCP mode should not send HTTP request");
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
