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
pub use stub::{run_http3_probe, run_http3_request_probe};

#[cfg(not(feature = "http3"))]
mod stub {
    use crate::metrics::{ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
    use chrono::Utc;
    use uuid::Uuid;

    pub async fn run_http3_probe(
        run_id: Uuid,
        sequence_num: u32,
        target: &url::Url,
        timeout_ms: u64,
        insecure: bool,
        ca_bundle: Option<&str>,
    ) -> RequestAttempt {
        run_http3_request_probe(
            run_id,
            sequence_num,
            Protocol::Http3,
            target,
            0,
            &crate::runner::http::RunConfig {
                timeout_ms,
                insecure,
                ca_bundle: ca_bundle.map(str::to_string),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn run_http3_request_probe(
        run_id: Uuid,
        sequence_num: u32,
        protocol: Protocol,
        _target: &url::Url,
        _payload_size: usize,
        _cfg: &crate::runner::http::RunConfig,
    ) -> RequestAttempt {
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol,
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
            http_stack: None,
            rpm: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Real HTTP/3 implementation (feature-gated)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "http3")]
pub use real::{run_http3_probe, run_http3_request_probe};

#[cfg(feature = "http3")]
mod real {
    use crate::metrics::{
        DnsResult, ErrorCategory, HttpResult, Protocol, RequestAttempt, TlsResult,
    };
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

    /// Classify a `build_quic_endpoint` failure message into the phase that
    /// actually failed. (Trust audit V10: everything used to collapse to
    /// `ErrorCategory::Http`.)
    fn classify_endpoint_build_error(msg: &str) -> ErrorCategory {
        if msg.contains("TLS config") || msg.contains("QUIC TLS config") {
            ErrorCategory::Tls
        } else if msg.starts_with("No host") {
            ErrorCategory::Config
        } else {
            ErrorCategory::Other
        }
    }

    /// Classify a QUIC connection failure. QUIC has no TCP phase — a failure
    /// to establish the connection is the connect-equivalent (`Tcp`), unless
    /// it is a handshake/crypto failure (`Tls`) or an idle/handshake timeout
    /// (`Timeout`). (Trust audit V10.)
    fn classify_quic_connection_error(e: &quinn::ConnectionError) -> ErrorCategory {
        match e {
            quinn::ConnectionError::TimedOut => ErrorCategory::Timeout,
            quinn::ConnectionError::TransportError(te) => {
                let msg = te.to_string().to_ascii_lowercase();
                if msg.contains("crypto")
                    || msg.contains("tls")
                    || msg.contains("certificate")
                    || msg.contains("handshake")
                {
                    ErrorCategory::Tls
                } else {
                    ErrorCategory::Tcp
                }
            }
            _ => ErrorCategory::Tcp,
        }
    }

    /// Resolve the target address, trying direct parse first then DNS lookup.
    ///
    /// Returns the address plus a `DnsResult` timing record when an actual
    /// DNS lookup happened (`None` for IP literals), so HTTP/3 attempts carry
    /// the same DNS phase timing as HTTP/1.1 and HTTP/2 — previously `dns`
    /// was always absent and H3 goodput/overhead omitted DNS while H1/H2
    /// included it. (Trust audit V10.)
    async fn resolve_addr(
        host: &str,
        port: u16,
    ) -> Result<(std::net::SocketAddr, Option<DnsResult>), String> {
        // Parse the bare host as an IP literal first — formatting "{host}:{port}"
        // misses IPv6 (needs brackets: [::1]:443) and would fall through to a
        // spurious DNS lookup for it.
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            return Ok((std::net::SocketAddr::new(ip, port), None));
        }
        let addr_str = format!("{host}:{port}");
        let started_at = Utc::now();
        let t0 = Instant::now();
        let addrs: Vec<std::net::SocketAddr> = match tokio::net::lookup_host(&addr_str).await {
            Ok(a) => a.collect(),
            Err(e) => return Err(format!("DNS error: {e}")),
        };
        let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let addr = *addrs
            .first()
            .ok_or_else(|| format!("No addresses resolved for {host}"))?;
        let dns = DnsResult {
            query_name: host.to_string(),
            resolved_ips: addrs.iter().map(|a| a.ip().to_string()).collect(),
            duration_ms,
            started_at,
            success: true,
            // lookup_host goes through the OS (getaddrinfo); the concrete
            // nameserver is not observable from here.
            resolver: Some("system (OS getaddrinfo)".to_string()),
        };
        Ok((addr, Some(dns)))
    }

    pub async fn run_http3_probe(
        run_id: Uuid,
        sequence_num: u32,
        target: &url::Url,
        timeout_ms: u64,
        insecure: bool,
        ca_bundle: Option<&str>,
    ) -> RequestAttempt {
        run_http3_request_probe(
            run_id,
            sequence_num,
            Protocol::Http3,
            target,
            0,
            &crate::runner::http::RunConfig {
                timeout_ms,
                insecure,
                ca_bundle: ca_bundle.map(str::to_string),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn run_http3_request_probe(
        run_id: Uuid,
        sequence_num: u32,
        protocol: Protocol,
        target: &url::Url,
        payload_size: usize,
        cfg: &crate::runner::http::RunConfig,
    ) -> RequestAttempt {
        let attempt_id = Uuid::new_v4();
        let started_at = Utc::now();
        let t0 = Instant::now();
        // Single per-probe deadline: every post-connect HTTP/3 phase races the
        // *remaining* budget, so a server that completes the QUIC handshake and
        // then stalls (or dribbles the body) cannot extend the probe past
        // `timeout_ms` and contaminate `total_ms`.
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(cfg.timeout_ms);
        let cpu_start = cpu_time::ProcessTime::now();
        #[cfg(unix)]
        let (csw_v0, csw_i0) = get_rusage_csw();

        let (endpoint, host, port) =
            match build_quic_endpoint(target, cfg.insecure, cfg.ca_bundle.as_deref()) {
                Ok(v) => v,
                Err(msg) => {
                    return h3_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        protocol.clone(),
                        started_at,
                        classify_endpoint_build_error(&msg),
                        &msg,
                    );
                }
            };

        let (server_addr, dns_result) = match resolve_addr(&host, port).await {
            Ok(v) => v,
            Err(msg) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Dns,
                    &msg,
                );
            }
        };

        // QUIC handshake — pass the Connecting future to timeout directly;
        // do NOT .await it inline or it resolves before timeout can race it.
        let t_handshake = Instant::now();
        let connecting = match endpoint.connect(server_addr, &host) {
            Ok(c) => c,
            Err(e) => {
                // quinn::ConnectError variants are all local setup problems
                // (invalid server name, no client config, CIDs exhausted…).
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Config,
                    &format!("QUIC connect error: {e}"),
                );
            }
        };
        let conn = match tokio::time::timeout(
            std::time::Duration::from_millis(cfg.timeout_ms),
            connecting,
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    classify_quic_connection_error(&e),
                    &format!("QUIC connect: {e}"),
                );
            }
            Err(_) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Timeout,
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
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Http,
                    &format!("h3 handshake: {e}"),
                );
            }
        };
        let (mut driver, mut send_req) = h3_conn;

        tokio::spawn(async move {
            let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });

        // Send request
        let mut path = if target.path().is_empty() {
            "/".to_string()
        } else {
            target.path().to_string()
        };
        if let Some(query) = target.query() {
            path.push('?');
            path.push_str(query);
        }
        let method = if payload_size > 0 { "POST" } else { "GET" };
        let req = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}:{port}{path}"))
            .header("user-agent", "networker-tester/0.1 (h3)")
            .body(())
            .unwrap();

        let t_sent = Instant::now();
        let mut stream = match tokio::time::timeout_at(deadline, send_req.send_request(req)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Http,
                    &format!("h3 send_request: {e}"),
                );
            }
            Err(_) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Timeout,
                    "h3 send_request timeout",
                );
            }
        };
        if payload_size > 0 {
            let chunk = vec![0u8; 16 * 1024];
            let mut remaining = payload_size;
            while remaining > 0 {
                let n = remaining.min(chunk.len());
                if let Err(e) = stream
                    .send_data(bytes::Bytes::copy_from_slice(&chunk[..n]))
                    .await
                {
                    return h3_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        protocol.clone(),
                        started_at,
                        ErrorCategory::Http,
                        &format!("h3 send_data: {e}"),
                    );
                }
                remaining -= n;
            }
        }
        stream.finish().await.ok();

        let resp = match tokio::time::timeout_at(deadline, stream.recv_response()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Http,
                    &format!("h3 recv_response: {e}"),
                );
            }
            Err(_) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol.clone(),
                    started_at,
                    ErrorCategory::Timeout,
                    "h3 recv_response timeout",
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
        loop {
            match tokio::time::timeout_at(deadline, stream.recv_data()).await {
                Ok(Ok(Some(chunk))) => body_size += chunk.remaining(),
                // End of body, or a stream error — treated as end of body,
                // matching the previous `.ok().flatten()` semantics.
                Ok(_) => break,
                Err(_) => {
                    return h3_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        protocol.clone(),
                        started_at,
                        ErrorCategory::Timeout,
                        "h3 body read timeout",
                    );
                }
            }
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
            resumed: None,
            handshake_kind: None,
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
        };

        RequestAttempt {
            attempt_id,
            run_id,
            protocol,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: status_code < 500,
            // DNS phase timing now recorded for H3 like H1/H2 (audit V10);
            // `tcp` stays None — QUIC has no TCP phase by design.
            dns: dns_result,
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
                http_handshake_ms: None,
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
            rpm: None,
        }
    }

    /// Build a failed HTTP/3 attempt. `category` reflects the phase that
    /// failed: `Dns` for resolution, `Tls` for QUIC/TLS crypto failures,
    /// `Tcp` for the QUIC connect-equivalent, `Timeout` for handshake/idle
    /// timeouts, and `Http` only for actual HTTP/3-layer errors — previously
    /// everything collapsed to `Http`. (Trust audit V10.)
    fn h3_failed(
        run_id: Uuid,
        attempt_id: Uuid,
        sequence_num: u32,
        protocol: Protocol,
        started_at: chrono::DateTime<Utc>,
        category: ErrorCategory,
        message: &str,
    ) -> RequestAttempt {
        RequestAttempt {
            attempt_id,
            run_id,
            protocol,
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
                category,
                message: message.to_string(),
                detail: None,
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
            rpm: None,
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
                        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
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
            let err = build_quic_endpoint(&url, false, Some("/nonexistent/ca.pem")).unwrap_err();
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
            let (addr, dns) = resolve_addr("127.0.0.1", 8443).await.unwrap();
            assert_eq!(addr, "127.0.0.1:8443".parse().unwrap());
            assert!(dns.is_none(), "IP literal must not record a DNS phase");
        }

        #[tokio::test]
        async fn resolve_addr_ipv6_literal() {
            let (addr, dns) = resolve_addr("::1", 443).await.unwrap();
            assert_eq!(addr, "[::1]:443".parse().unwrap());
            assert!(dns.is_none());
        }

        #[tokio::test]
        async fn resolve_addr_hostname_localhost() {
            let (addr, dns) = resolve_addr("localhost", 9999).await.unwrap();
            assert_eq!(addr.port(), 9999);
            assert!(addr.ip().is_loopback());
            // Trust audit V10: an actual DNS lookup must emit phase timing.
            let dns = dns.expect("hostname resolution must record a DnsResult");
            assert_eq!(dns.query_name, "localhost");
            assert!(dns.success);
            assert!(dns.duration_ms >= 0.0);
            assert!(!dns.resolved_ips.is_empty());
            assert_eq!(dns.resolver.as_deref(), Some("system (OS getaddrinfo)"));
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

        #[cfg(not(target_os = "windows"))]
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
            // Trust audit V10: a resolution failure must be classified Dns,
            // not collapsed to Http.
            assert_eq!(
                err.category,
                ErrorCategory::Dns,
                "H3 DNS failure must classify as Dns, got {:?} ({})",
                err.category,
                err.message
            );
        }

        /// Regression test for trust-audit V10: an HTTPS URL pointing at a
        /// port with no QUIC listener is a connect-phase failure. It must be
        /// classified connect-ish (Tcp — the QUIC connect-equivalent — or
        /// Timeout on platforms that swallow ICMP unreachable), never `Http`:
        /// no HTTP exchange ever happened.
        #[tokio::test]
        async fn h3_probe_connection_refused() {
            init_crypto();
            let target: url::Url = "https://127.0.0.1:1/health".parse().unwrap();
            let a = run_http3_probe(Uuid::new_v4(), 3, &target, 3_000, true, None).await;
            assert!(!a.success);
            assert_eq!(a.protocol, Protocol::Http3);
            let err = a.error.unwrap();
            assert!(
                matches!(err.category, ErrorCategory::Tcp | ErrorCategory::Timeout),
                "QUIC connect failure must be Tcp/Timeout, not {:?} ({})",
                err.category,
                err.message
            );
        }

        // ── error-classification unit tests (trust audit V10) ────────────────

        #[test]
        fn classify_quic_timeout_is_timeout() {
            assert_eq!(
                classify_quic_connection_error(&quinn::ConnectionError::TimedOut),
                ErrorCategory::Timeout
            );
        }

        #[test]
        fn classify_quic_reset_is_connectish() {
            assert_eq!(
                classify_quic_connection_error(&quinn::ConnectionError::Reset),
                ErrorCategory::Tcp
            );
        }

        #[test]
        fn classify_endpoint_build_errors_by_phase() {
            assert_eq!(
                classify_endpoint_build_error("TLS config error: bad CA bundle"),
                ErrorCategory::Tls
            );
            assert_eq!(
                classify_endpoint_build_error("QUIC TLS config error: no cipher"),
                ErrorCategory::Tls
            );
            assert_eq!(
                classify_endpoint_build_error("No host in URL"),
                ErrorCategory::Config
            );
            assert_eq!(
                classify_endpoint_build_error("QUIC endpoint creation failed: eperm"),
                ErrorCategory::Other
            );
        }

        #[tokio::test]
        async fn h3_probe_bad_ca_bundle_is_tls_error() {
            init_crypto();
            let target: url::Url = "https://127.0.0.1:8443/health".parse().unwrap();
            let a = run_http3_probe(
                Uuid::new_v4(),
                4,
                &target,
                3_000,
                false,
                Some("/nonexistent/ca.pem"),
            )
            .await;
            assert!(!a.success);
            assert_eq!(a.error.unwrap().category, ErrorCategory::Tls);
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
            let a = h3_failed(
                run_id,
                attempt_id,
                7,
                Protocol::Http3,
                Utc::now(),
                ErrorCategory::Tls,
                "test error",
            );
            assert!(!a.success);
            assert_eq!(a.protocol, Protocol::Http3);
            assert_eq!(a.run_id, run_id);
            assert_eq!(a.attempt_id, attempt_id);
            assert_eq!(a.sequence_num, 7);
            let err = a.error.unwrap();
            assert_eq!(err.category, ErrorCategory::Tls);
            assert_eq!(err.message, "test error");
            assert!(a.dns.is_none());
            assert!(a.tcp.is_none());
            assert!(a.tls.is_none());
            assert!(a.http.is_none());
        }

        #[cfg(unix)]
        #[test]
        fn get_rusage_csw_returns_non_negative() {
            let (v, i) = get_rusage_csw();
            assert!(v >= 0);
            assert!(i >= 0);
        }
    }
}
