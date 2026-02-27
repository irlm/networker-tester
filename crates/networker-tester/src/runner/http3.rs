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
                message: "HTTP/3 support not compiled in. Rebuild with --features http3".into(),
                detail: Some("cargo build --features http3".into()),
                occurred_at: Utc::now(),
            }),
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

    pub async fn run_http3_probe(
        run_id: Uuid,
        sequence_num: u32,
        target: &url::Url,
        timeout_ms: u64,
    ) -> RequestAttempt {
        let attempt_id = Uuid::new_v4();
        let started_at = Utc::now();
        let t0 = Instant::now();

        let host = match target.host_str() {
            Some(h) => h.to_string(),
            None => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    "No host in URL",
                );
            }
        };
        let port = target.port().unwrap_or(443);

        // Build QUIC/TLS config
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let quinn_tls = QuinnClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
                .expect("valid QUIC TLS config"),
        ));

        let mut endpoint = match Endpoint::client("0.0.0.0:0".parse().unwrap()) {
            Ok(e) => e,
            Err(e) => {
                return h3_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    &format!("QUIC endpoint creation failed: {e}"),
                );
            }
        };
        endpoint.set_default_client_config(quinn_tls);

        let addr = format!("{host}:{port}");
        let server_addr: std::net::SocketAddr = match addr.parse() {
            Ok(a) => a,
            Err(_) => {
                // DNS lookup
                match tokio::net::lookup_host(&addr).await {
                    Ok(mut a) => match a.next() {
                        Some(sa) => sa,
                        None => {
                            return h3_failed(
                                run_id,
                                attempt_id,
                                sequence_num,
                                started_at,
                                "No address resolved",
                            )
                        }
                    },
                    Err(e) => {
                        return h3_failed(
                            run_id,
                            attempt_id,
                            sequence_num,
                            started_at,
                            &format!("DNS error: {e}"),
                        )
                    }
                }
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
            }),
            udp: None,
            error: None,
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
        }
    }
}
