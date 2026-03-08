/// HTTP/3 over QUIC probe (optional, requires `--features http3`).
///
/// # Implementation notes
/// HTTP/3 combines the transport (QUIC / UDP) and security (TLS 1.3 inside QUIC)
/// layers, so the timing model differs from HTTP/1.1 and HTTP/2:
///
///   quic_handshake_ms  = QUIC 1-RTT or 0-RTT handshake (includes TLS 1.3)
///   stream_open_ms     = time to open first QUIC stream
///   ttfb_ms            = time from first HEADERS frame sent to first response frame
///   total_ms           = quic_handshake + stream + ttfb + body
///
/// There is no separate TCP or plain-TLS phase.
///
/// # Status
/// HTTP/3 support is gated behind `--features http3`.  The endpoint also needs
/// HTTP/3 support (see `networker-endpoint` docs).  In CI, HTTP/3 tests are
/// skipped unless the `H3_TEST` environment variable is set.
#[cfg(not(feature = "http3"))]
pub use stub::run_http3_probe;

#[cfg(not(feature = "http3"))]
mod stub {
    use crate::metrics::{ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
    use chrono::Utc;
    use uuid::Uuid;

    pub async fn run_http3_probe(
        run_id: Uuid,
        sequence_num: u32,
        _target: &url::Url,
        _timeout_ms: u64,
        _insecure: bool,
        _ca_bundle: Option<&str>,
    ) -> RequestAttempt {
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Http3,
            sequence_num,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(ErrorRecord {
                category: ErrorCategory::Config,
                message:
                    "HTTP/3 support was excluded at compile time (built with --no-default-features)"
                        .into(),
                detail: Some("cargo build (without --no-default-features) to enable HTTP/3".into()),
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Real HTTP/3 implementation (feature-gated)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "http3")]
pub use real::run_http3_probe;

#[cfg(feature = "http3")]
mod real {
    use crate::metrics::{ErrorCategory, HttpResult, Protocol, RequestAttempt, TlsResult};
    use bytes::Buf;
    use chrono::Utc;
    use h3_quinn::Connection as QuinnH3Connection;
    use quinn::{ClientConfig as QuinnClientConfig, Endpoint};
    use std::sync::Arc;
    use std::time::Instant;
    use uuid::Uuid;

    #[cfg(unix)]
    fn get_rusage_csw() -> (i64, i64) {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        (u.ru_nvcsw, u.ru_nivcsw)
    }

    /// Build a QUIC endpoint configured for HTTP/3 with the given TLS settings.
    ///
    /// Returns `(Endpoint, host, port)` on success, or an error message on failure.
    pub fn build_quic_endpoint(
        target: &url::Url,
        insecure: bool,
        ca_bundle: Option<&str>,
    ) -> Result<(Endpoint, String, u16), String> {
        let host = target
            .host_str()
            .ok_or_else(|| "No host in URL".to_string())?
            .to_string();
        let port = target.port().unwrap_or(443);

        let mut tls_config = crate::runner::http::build_tls_config(
            &crate::metrics::Protocol::Http1,
            insecure,
            ca_bundle,
        )
        .map_err(|e| format!("TLS config error: {e}"))?;
        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let quinn_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| format!("QUIC TLS config error: {e}"))?;

        let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())
            .map_err(|e| format!("QUIC endpoint creation failed: {e}"))?;
        endpoint.set_default_client_config(QuinnClientConfig::new(Arc::new(quinn_tls)));

        Ok((endpoint, host, port))
    }

    /// Resolve the target address, trying direct parse first then DNS lookup.
    async fn resolve_addr(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
        let addr_str = format!("{host}:{port}");
        match addr_str.parse() {
            Ok(a) => Ok(a),
            Err(_) => match tokio::net::lookup_host(&addr_str).await {
                Ok(mut a) => a
                    .next()
                    .ok_or_else(|| format!("No addresses resolved for {host}")),
                Err(e) => Err(format!("DNS error: {e}")),
            },
        }
    }

    pub async fn run_http3_probe(
        run_id: Uuid,
        sequence_num: u32,
        target: &url::Url,
        timeout_ms: u64,
        insecure: bool,
        ca_bundle: Option<&str>,
    ) -> RequestAttempt {
        let attempt_id = Uuid::new_v4();
        let started_at = Utc::now();
        let t0 = Instant::now();
        let cpu_start = cpu_time::ProcessTime::now();
        #[cfg(unix)]
        let (csw_v0, csw_i0) = get_rusage_csw();

        let (endpoint, host, port) = match build_quic_endpoint(target, insecure, ca_bundle) {
            Ok(v) => v,
            Err(msg) => {
                return h3_failed(run_id, attempt_id, sequence_num, started_at, &msg);
            }
        };

        let server_addr = match resolve_addr(&host, port).await {
            Ok(a) => a,
            Err(msg) => {
                return h3_failed(run_id, attempt_id, sequence_num, started_at, &msg);
            }
        };

        // QUIC handshake — pass the Connecting future to timeout directly;
        // do NOT .await it inline or it resolves before timeout can race it.
        let t_handshake = Instant::now();
        let connecting = match endpoint.connect(server_addr, &host) {
            Ok(c) => c,
            Err(e) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    &format!("QUIC connect error: {e}"),
                );
            }
        };
        let conn =
            match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), connecting)
                .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    return h3_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        started_at,
                        &format!("QUIC connect: {e}"),
                    );
                }
                Err(_) => {
                    return h3_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        started_at,
                        "QUIC handshake timeout",
                    );
                }
            };
        let handshake_ms = t_handshake.elapsed().as_secs_f64() * 1000.0;

        // Build h3 connection
        let h3_conn = match h3::client::new(QuinnH3Connection::new(conn)).await {
            Ok((driver, send_req)) => (driver, send_req),
            Err(e) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    &format!("h3 handshake: {e}"),
                );
            }
        };
        let (mut driver, mut send_req) = h3_conn;

        tokio::spawn(async move {
            let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });

        // Send request
        let path = if target.path().is_empty() {
            "/"
        } else {
            target.path()
        };
        let req = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}:{port}{path}"))
            .header("user-agent", "networker-tester/0.1 (h3)")
            .body(())
            .unwrap();

        let t_sent = Instant::now();
        let mut stream = match send_req.send_request(req).await {
            Ok(s) => s,
            Err(e) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    &format!("h3 send_request: {e}"),
                );
            }
        };
        stream.finish().await.ok();

        let resp = match stream.recv_response().await {
            Ok(r) => r,
            Err(e) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    &format!("h3 recv_response: {e}"),
                );
            }
        };
        let ttfb_ms = t_sent.elapsed().as_secs_f64() * 1000.0;
        let status_code = resp.status().as_u16();

        let headers = resp.headers().clone();
        let headers_size: usize = headers
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len() + 4)
            .sum();
        let response_headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let mut body_size = 0;
        while let Some(chunk) = stream.recv_data().await.ok().flatten() {
            body_size += chunk.remaining();
        }

        let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let cpu_time_ms = Some(cpu_start.elapsed().as_secs_f64() * 1000.0);
        #[cfg(unix)]
        let (csw_voluntary, csw_involuntary) = {
            let (v1, i1) = get_rusage_csw();
            (Some((v1 - csw_v0) as u64), Some((i1 - csw_i0) as u64))
        };
        #[cfg(not(unix))]
        let (csw_voluntary, csw_involuntary) = (None::<u64>, None::<u64>);
        let http_started_at = Utc::now();

        let tls_result = TlsResult {
            protocol_version: "TLSv1.3 (QUIC)".into(),
            cipher_suite: "QUIC-embedded".into(),
            alpn_negotiated: Some("h3".into()),
            cert_subject: None,
            cert_issuer: None,
            cert_expiry: None,
            handshake_duration_ms: handshake_ms,
            started_at: http_started_at,
            success: true,
            cert_chain: vec![],
            tls_backend: Some("rustls".into()),
        };

        RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::Http3,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: status_code < 500,
            dns: None,
            tcp: None,
            tls: Some(tls_result),
            http: Some(HttpResult {
                negotiated_version: "HTTP/3".into(),
                status_code,
                headers_size_bytes: headers_size,
                body_size_bytes: body_size,
                ttfb_ms,
                total_duration_ms: total_ms,
                redirect_count: 0,
                started_at: http_started_at,
                response_headers,
                payload_bytes: 0,
                throughput_mbps: None,
                goodput_mbps: None,
                cpu_time_ms,
                csw_voluntary,
                csw_involuntary,
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        }
    }

    fn h3_failed(
        run_id: Uuid,
        attempt_id: Uuid,
        sequence_num: u32,
        started_at: chrono::DateTime<Utc>,
        message: &str,
    ) -> RequestAttempt {
        RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::Http3,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(crate::metrics::ErrorRecord {
                category: ErrorCategory::Http,
                message: message.to_string(),
                detail: None,
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn init_crypto() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        fn free_port() -> u16 {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        }

        fn free_udp_port() -> u16 {
            std::net::UdpSocket::bind("0.0.0.0:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        }

        struct TestEndpoint {
            https_port: u16,
            _shutdown: tokio::sync::oneshot::Sender<()>,
        }

        impl TestEndpoint {
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
                // Wait for HTTPS TCP
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if tokio::net::TcpStream::connect(format!("127.0.0.1:{https_port}"))
                        .await
                        .is_ok()
                    {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Endpoint did not start"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Self {
                    https_port,
                    _shutdown: tx,
                }
            }

            fn https_url(&self, path: &str) -> url::Url {
                format!("https://127.0.0.1:{}{path}", self.https_port)
                    .parse()
                    .unwrap()
            }

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
                        Err(_timeout) => break,
                        Ok(Ok(_)) => break,
                        Ok(Err(e))
                            if e.kind() == std::io::ErrorKind::ConnectionRefused =>
                        {
                            // not bound yet
                        }
                        Ok(Err(_)) => break,
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "QUIC server did not start"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }

        // ── build_quic_endpoint tests ────────────────────────────────────────

        #[tokio::test]
        async fn build_quic_endpoint_success() {
            init_crypto();
            let url: url::Url = "https://127.0.0.1:8443/health".parse().unwrap();
            let (ep, host, port) = build_quic_endpoint(&url, true, None).unwrap();
            assert_eq!(host, "127.0.0.1");
            assert_eq!(port, 8443);
            drop(ep);
        }

        #[tokio::test]
        async fn build_quic_endpoint_default_port() {
            init_crypto();
            let url: url::Url = "https://example.com/path".parse().unwrap();
            let (_, host, port) = build_quic_endpoint(&url, true, None).unwrap();
            assert_eq!(host, "example.com");
            assert_eq!(port, 443);
        }

        #[test]
        fn build_quic_endpoint_no_host() {
            let url: url::Url = "data:text/html,x".parse().unwrap();
            let err = build_quic_endpoint(&url, true, None).unwrap_err();
            assert!(err.contains("No host"), "got: {err}");
        }

        #[tokio::test]
        async fn build_quic_endpoint_bad_ca_bundle() {
            init_crypto();
            let url: url::Url = "https://127.0.0.1:8443/health".parse().unwrap();
            let err =
                build_quic_endpoint(&url, false, Some("/nonexistent/ca.pem")).unwrap_err();
            assert!(err.contains("TLS config error"), "got: {err}");
        }

        #[tokio::test]
        async fn build_quic_endpoint_insecure_vs_secure() {
            init_crypto();
            let url: url::Url = "https://127.0.0.1:8443/".parse().unwrap();
            assert!(build_quic_endpoint(&url, true, None).is_ok());
            assert!(build_quic_endpoint(&url, false, None).is_ok());
        }

        // ── resolve_addr tests ───────────────────────────────────────────────

        #[tokio::test]
        async fn resolve_addr_ip_literal() {
            let addr = resolve_addr("127.0.0.1", 8443).await.unwrap();
            assert_eq!(addr, "127.0.0.1:8443".parse().unwrap());
        }

        #[tokio::test]
        async fn resolve_addr_ipv6_literal() {
            let addr = resolve_addr("::1", 443).await.unwrap();
            assert_eq!(addr, "[::1]:443".parse().unwrap());
        }

        #[tokio::test]
        async fn resolve_addr_hostname_localhost() {
            let addr = resolve_addr("localhost", 9999).await.unwrap();
            assert_eq!(addr.port(), 9999);
            assert!(addr.ip().is_loopback());
        }

        #[tokio::test]
        async fn resolve_addr_unresolvable() {
            let err = resolve_addr("this-does-not-exist-xyz.invalid", 443)
                .await
                .unwrap_err();
            assert!(
                err.contains("DNS") || err.contains("resolve") || err.contains("No address"),
                "got: {err}"
            );
        }

        // ── Integration: full probe ──────────────────────────────────────────

        #[tokio::test]
        async fn h3_probe_success() {
            let ep = TestEndpoint::start().await;
            ep.wait_for_quic().await;
            let target = ep.https_url("/health");
            let a = run_http3_probe(Uuid::new_v4(), 0, &target, 10_000, true, None).await;
            assert!(a.success, "H3 probe failed: {:?}", a.error);
            assert_eq!(a.protocol, Protocol::Http3);
            assert!(a.tls.is_some());
            let tls = a.tls.unwrap();
            assert_eq!(tls.alpn_negotiated.as_deref(), Some("h3"));
            assert!(tls.handshake_duration_ms > 0.0);
            assert!(a.http.is_some());
            let http = a.http.unwrap();
            assert_eq!(http.negotiated_version, "HTTP/3");
            assert_eq!(http.status_code, 200);
            assert!(http.ttfb_ms > 0.0);
            assert!(http.total_duration_ms > 0.0);
            assert!(http.body_size_bytes > 0);
            assert!(http.cpu_time_ms.is_some());
            #[cfg(unix)]
            {
                assert!(http.csw_voluntary.is_some());
                assert!(http.csw_involuntary.is_some());
            }
        }

        #[tokio::test]
        async fn h3_probe_no_host() {
            let target: url::Url = "data:text/html,hello".parse().unwrap();
            let a = run_http3_probe(Uuid::new_v4(), 1, &target, 5_000, true, None).await;
            assert!(!a.success);
            assert_eq!(a.protocol, Protocol::Http3);
            let err = a.error.unwrap();
            assert!(err.message.contains("No host"));
        }

        #[tokio::test]
        async fn h3_probe_unresolvable_host() {
            init_crypto();
            let target: url::Url = "https://this-does-not-exist-xyz.invalid:9999/health"
                .parse()
                .unwrap();
            let a = run_http3_probe(Uuid::new_v4(), 2, &target, 5_000, true, None).await;
            assert!(!a.success);
            assert_eq!(a.protocol, Protocol::Http3);
            let err = a.error.unwrap();
            assert!(
                err.message.contains("DNS") || err.message.contains("resolve"),
                "got: {}",
                err.message
            );
        }

        #[tokio::test]
        async fn h3_probe_connection_refused() {
            init_crypto();
            let target: url::Url = "https://127.0.0.1:1/health".parse().unwrap();
            let a = run_http3_probe(Uuid::new_v4(), 3, &target, 3_000, true, None).await;
            assert!(!a.success);
            assert_eq!(a.protocol, Protocol::Http3);
        }

        #[tokio::test]
        async fn h3_probe_records_sequence_num() {
            init_crypto();
            let target: url::Url = "data:text/html,x".parse().unwrap();
            let a = run_http3_probe(Uuid::new_v4(), 42, &target, 5_000, true, None).await;
            assert_eq!(a.sequence_num, 42);
        }

        #[tokio::test]
        async fn h3_failed_helper_sets_fields() {
            let run_id = Uuid::new_v4();
            let attempt_id = Uuid::new_v4();
            let a = h3_failed(run_id, attempt_id, 7, Utc::now(), "test error");
            assert!(!a.success);
            assert_eq!(a.protocol, Protocol::Http3);
            assert_eq!(a.run_id, run_id);
            assert_eq!(a.attempt_id, attempt_id);
            assert_eq!(a.sequence_num, 7);
            let err = a.error.unwrap();
            assert_eq!(err.category, ErrorCategory::Http);
            assert_eq!(err.message, "test error");
            assert!(a.dns.is_none());
            assert!(a.tcp.is_none());
            assert!(a.tls.is_none());
            assert!(a.http.is_none());
        }

        #[test]
        fn get_rusage_csw_returns_non_negative() {
            let (v, i) = get_rusage_csw();
            assert!(v >= 0);
            assert!(i >= 0);
        }
    }
}
