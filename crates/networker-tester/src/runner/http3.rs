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
            phase: None,
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
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Real HTTP/3 implementation (feature-gated)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "http3")]
pub use real::{run_http3_probe, run_http3_request_probe};

// QUIC stats mapping shared with the pageload3 runner. Feature-gated only —
// it takes a `quinn` type, so it cannot exist (and needs no stub mirror) in
// `--no-default-features` builds, whose h3 paths all return the stub error.
#[cfg(feature = "http3")]
pub(crate) use real::quic_stats_from;

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

    /// Congestion controller of OUR quinn client configuration. The probes
    /// never override `TransportConfig::congestion_controller_factory`, whose
    /// quinn 0.11 default is Cubic. quinn exposes no runtime query for the
    /// active controller, so this label records configuration — hence the
    /// explicit "(client-config)" suffix distinguishing it from the
    /// kernel-queried `TCP_CONGESTION` value on the TCP side.
    pub(crate) const QUIC_CONGESTION_ALGORITHM: &str = "cubic (client-config)";

    /// Map `quinn::Connection::stats()` output into the serializable additive
    /// [`QuicStats`](crate::metrics::QuicStats) record (deep-measurement M1
    /// B.1 / M3 G1).
    ///
    /// Sampled by callers AFTER the response body completes and before the
    /// connection closes — the point where cwnd, loss counts, and the
    /// DPLPMTUD `current_mtu` describe the transfer (same moment as the TCP
    /// dup-fd `SocketStats` snapshot).
    ///
    /// quinn 0.11 does not expose ECN counters or PTO episode counts, so
    /// those are honestly absent from `QuicStats` rather than faked.
    pub(crate) fn quic_stats_from(stats: &quinn::ConnectionStats) -> crate::metrics::QuicStats {
        let p = &stats.path;
        crate::metrics::QuicStats {
            rtt_ms: Some(p.rtt.as_secs_f64() * 1000.0),
            // QUIC cwnd is bytes (RFC 9002) — never converted to segments.
            cwnd_bytes: Some(p.cwnd),
            current_mtu: Some(p.current_mtu),
            lost_packets: Some(p.lost_packets),
            lost_bytes: Some(p.lost_bytes),
            sent_packets: Some(p.sent_packets),
            congestion_events: Some(p.congestion_events),
            sent_plpmtud_probes: Some(p.sent_plpmtud_probes),
            lost_plpmtud_probes: Some(p.lost_plpmtud_probes),
            black_holes_detected: Some(p.black_holes_detected),
            udp_tx_datagrams: Some(stats.udp_tx.datagrams),
            udp_tx_bytes: Some(stats.udp_tx.bytes),
            udp_rx_datagrams: Some(stats.udp_rx.datagrams),
            udp_rx_bytes: Some(stats.udp_rx.bytes),
            congestion_algorithm: Some(QUIC_CONGESTION_ALGORITHM.to_string()),
        }
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
        // Allow the rustls client to store session tickets with an early-data
        // allowance. Without this, `Connecting::into_0rtt()` can never return
        // `Ok` and the QUIC resumption/0-RTT measurement always reports
        // `zero_rtt_attempted = false`. Has no effect on the cold handshake.
        tls_config.enable_early_data = true;

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
    ///
    /// Resolution goes through the SAME hickory instrument as HTTP/1.1/2
    /// (`dns_runner::resolve`), honoring the `--ipv4-only`/`--ipv6-only`
    /// flags. It previously used `tokio::net::lookup_host` (OS getaddrinfo) —
    /// a different resolver path with a different cache and search-list
    /// behavior, so the flagship h1/h2/h3 head-to-head compared `dns_ms`
    /// columns measured by two different instruments and could not honor
    /// family pinning. (Deep-measurement audit M2 finding D2.)
    async fn resolve_addr(
        host: &str,
        port: u16,
        ipv4_only: bool,
        ipv6_only: bool,
    ) -> Result<(std::net::SocketAddr, Option<DnsResult>), String> {
        // Parse the bare host as an IP literal first — no DNS phase to
        // measure, matching the pre-existing h3 contract (`dns: None`).
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            return Ok((std::net::SocketAddr::new(ip, port), None));
        }
        let (ips, dns) = crate::runner::dns::resolve(host, ipv4_only, ipv6_only)
            .await
            .map_err(|e| format!("DNS error: {}", e.message))?;
        // Same connect-address selection as the h1/h2 path (`pick_ip`):
        // hickory's pinned Ipv4thenIpv6 strategy already orders A first.
        let ip = if ipv4_only {
            ips.iter()
                .find(|ip| ip.is_ipv4())
                .copied()
                .unwrap_or(ips[0])
        } else {
            ips[0]
        };
        Ok((std::net::SocketAddr::new(ip, port), Some(dns)))
    }

    /// Facts from the follow-up (second) QUIC connection that measures TLS 1.3
    /// session resumption and 0-RTT early data. All fields default to `None`
    /// (nothing measured) and the measurement never fails the parent attempt.
    #[derive(Default)]
    struct QuicResumptionFacts {
        quic_resumed: Option<bool>,
        zero_rtt_attempted: Option<bool>,
        zero_rtt_accepted: Option<bool>,
        resumed_handshake_ms: Option<f64>,
        /// Transport stats of the follow-up connection itself, sampled after
        /// its early-data exchange completed (`HttpResult::quic_resumption_stats`).
        /// None when the 0-RTT attempt never got a live connection.
        quic_stats: Option<crate::metrics::QuicStats>,
    }

    /// Open a follow-up QUIC connection through the same endpoint (and
    /// therefore the same rustls client config + session-ticket store) and
    /// measure whether the TLS 1.3 session resumes and whether the server
    /// accepts 0-RTT early data.
    ///
    /// Design note — why a second connection *inside* the attempt (the
    /// tlsresume idiom) rather than sharing ticket state across sequential
    /// attempts: probe attempts are architecturally stateless (`dispatch_once`
    /// builds a fresh runner per attempt), and a shared cross-attempt cache
    /// would silently turn attempts 2..n of every http3 run into warm
    /// handshakes — skewing the h1/h2/h3 head-to-head comparison, whose
    /// per-attempt connections are cold across all protocols. The follow-up
    /// connection leaves the primary (cold) connection's numbers untouched and
    /// is reported only through the additive `quic_*`/`zero_rtt_*` fields.
    ///
    /// What is actually measured: quinn's `Connecting::into_0rtt()` succeeds
    /// only when 0-RTT keys derived from a stored session ticket are
    /// available; the request is then sent in 0-RTT early data on the wire,
    /// and the `ZeroRttAccepted` future (resolving at handshake completion)
    /// reports whether the server *accepted* that early data. Acceptance
    /// requires PSK resumption, so it doubles as proof of session resumption
    /// (quinn does not expose rustls' handshake kind for QUIC).
    async fn measure_quic_resumption(
        endpoint: &Endpoint,
        server_addr: std::net::SocketAddr,
        host: &str,
        port: u16,
        deadline: tokio::time::Instant,
    ) -> QuicResumptionFacts {
        let mut facts = QuicResumptionFacts::default();
        let Ok(connecting) = endpoint.connect(server_addr, host) else {
            return facts;
        };
        let t0 = Instant::now();
        let (conn, accepted) = match connecting.into_0rtt() {
            Ok(v) => v,
            Err(_connecting) => {
                // No 0-RTT keys — the client has no usable session ticket
                // with an early-data allowance, so resumption cannot be
                // attempted (dropping `_connecting` cancels the handshake).
                facts.zero_rtt_attempted = Some(false);
                facts.quic_resumed = Some(false);
                return facts;
            }
        };
        facts.zero_rtt_attempted = Some(true);
        // Cheap handle clone: `conn` itself is consumed by the h3 client
        // below; the clone lets us sample `Connection::stats()` after the
        // early-data exchange completes.
        let conn_handle = conn.clone();

        // Best-effort: put a real request on the wire in 0-RTT early data
        // before the handshake completes. Bounded by the probe's remaining
        // deadline so a stalled peer cannot extend the attempt.
        let early_request = async {
            let (mut driver, mut send_req) =
                h3::client::new(QuinnH3Connection::new(conn)).await.ok()?;
            tokio::spawn(async move {
                let _ = futures::future::poll_fn(|cx| driver.poll_close(cx)).await;
            });
            let req = http::Request::builder()
                .method("GET")
                .uri(format!("https://{host}:{port}/health"))
                .header("user-agent", "networker-tester/0.1 (h3 0-rtt)")
                .body(())
                .ok()?;
            let mut stream = send_req.send_request(req).await.ok()?;
            stream.finish().await.ok()?;
            // Keep `send_req` alive alongside the stream: dropping the last
            // SendRequest makes the h3 client shut the connection down, which
            // would resolve ZeroRttAccepted to a spurious `false` before the
            // handshake completes.
            Some((send_req, stream))
        };
        let stream = tokio::time::timeout_at(deadline, early_request)
            .await
            .ok()
            .flatten();

        // Handshake completion doubles as the acceptance verdict: the future
        // resolves true iff the server accepted the early data.
        match tokio::time::timeout_at(deadline, accepted).await {
            Ok(acc) => {
                facts.resumed_handshake_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
                facts.zero_rtt_accepted = Some(acc);
                // Acceptance proves PSK resumption; rejection leaves
                // resumption unproven → reported false.
                facts.quic_resumed = Some(acc);
            }
            Err(_timeout) => return facts,
        }

        // Drain the early request's response so the exchange completes
        // cleanly (best-effort; ignored on 0-RTT rejection, where the
        // stream errors with ZeroRttRejected).
        if let Some((_send_req, mut stream)) = stream {
            if let Ok(Ok(_resp)) = tokio::time::timeout_at(deadline, stream.recv_response()).await {
                while let Ok(Ok(Some(_chunk))) =
                    tokio::time::timeout_at(deadline, stream.recv_data()).await
                {}
            }
        }
        // Post-exchange transport snapshot of the follow-up connection —
        // same capture moment as the primary connection's `quic_stats`.
        facts.quic_stats = Some(quic_stats_from(&conn_handle.stats()));
        facts
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

        let (server_addr, dns_result) =
            match resolve_addr(&host, port, cfg.ipv4_only, cfg.ipv6_only).await {
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

        // Cheap handle clone (quinn::Connection is an Arc-backed handle):
        // `conn` is consumed by the h3 client below; the clone samples
        // `Connection::stats()` after the body drain, before close — the QUIC
        // analogue of the TCP dup-fd post-transfer snapshot.
        let conn_handle = conn.clone();

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

        // Post-transfer QUIC transport snapshot (deep-measurement M1 B.1 /
        // M3 G1): body fully drained, connection still open — cwnd, loss
        // counts, and the DPLPMTUD MTU verdict describe THIS transfer.
        let quic_stats = Some(quic_stats_from(&conn_handle.stats()));

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

        // Follow-up connection: QUIC session-resumption + 0-RTT measurement.
        // Runs after total_ms/cpu accounting so it cannot contaminate the
        // primary (cold) connection's numbers. Only for plain http3 latency
        // probes — the throughput/pageload H3 variants reuse this runner but
        // don't need the extra connection per attempt.
        let resumption = if matches!(protocol, Protocol::Http3) {
            Some(measure_quic_resumption(&endpoint, server_addr, &host, port, deadline).await)
        } else {
            None
        };

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
            ocsp_stapled: None,
            ocsp_response_bytes: None,
            quic_resumed: resumption.as_ref().and_then(|r| r.quic_resumed),
            zero_rtt_attempted: resumption.as_ref().and_then(|r| r.zero_rtt_attempted),
            zero_rtt_accepted: resumption.as_ref().and_then(|r| r.zero_rtt_accepted),
            quic_resumed_handshake_ms: resumption.as_ref().and_then(|r| r.resumed_handshake_ms),
        };

        // Content negotiation metadata (gap #9), from the captured h3 headers.
        let content_encoding = response_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
            .map(|(_, v)| v.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty());
        let content_length_header = response_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<u64>().ok());

        RequestAttempt {
            phase: None,
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
                // HTTP/3 runs over QUIC/UDP — TCP kernel stats do not apply.
                socket_stats: None,
                content_encoding,
                content_length_header,
                security_headers: None,
                quic_stats,
                quic_resumption_stats: resumption.as_ref().and_then(|r| r.quic_stats.clone()),
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
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
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
            phase: None,
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
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
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
                    stamp_port: free_udp_port(),
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
            let (addr, dns) = resolve_addr("127.0.0.1", 8443, false, false).await.unwrap();
            assert_eq!(addr, "127.0.0.1:8443".parse().unwrap());
            assert!(dns.is_none(), "IP literal must not record a DNS phase");
        }

        #[tokio::test]
        async fn resolve_addr_ipv6_literal() {
            let (addr, dns) = resolve_addr("::1", 443, false, false).await.unwrap();
            assert_eq!(addr, "[::1]:443".parse().unwrap());
            assert!(dns.is_none());
        }

        /// Deep-measurement M2 D2: h3 must resolve through the SAME hickory
        /// instrument as h1/h2, so the recorded resolver label is the hickory
        /// one ("system (…:53)" / "google-fallback …") — never the old
        /// "system (OS getaddrinfo)" second instrument.
        #[tokio::test]
        async fn resolve_addr_hostname_localhost() {
            let (addr, dns) = resolve_addr("localhost", 9999, false, false).await.unwrap();
            assert_eq!(addr.port(), 9999);
            assert!(addr.ip().is_loopback());
            // Trust audit V10: an actual DNS lookup must emit phase timing.
            let dns = dns.expect("hostname resolution must record a DnsResult");
            assert_eq!(dns.query_name, "localhost");
            assert!(dns.success);
            assert!(dns.duration_ms >= 0.0);
            assert!(!dns.resolved_ips.is_empty());
            let resolver = dns.resolver.expect("resolver identity must be recorded");
            assert_ne!(
                resolver, "system (OS getaddrinfo)",
                "h3 DNS must use the hickory instrument, not getaddrinfo"
            );
            assert!(
                resolver.starts_with("system") || resolver.contains("fallback"),
                "unexpected resolver label: {resolver}"
            );
        }

        /// `--ipv4-only` must be honored by the h3 resolution path (the old
        /// getaddrinfo path could not express family pinning).
        #[tokio::test]
        async fn resolve_addr_hostname_ipv4_only_pins_family() {
            let (addr, dns) = resolve_addr("localhost", 9999, true, false).await.unwrap();
            assert!(addr.ip().is_ipv4(), "ipv4_only must pin the family");
            let dns = dns.expect("hostname resolution must record a DnsResult");
            assert!(dns
                .resolved_ips
                .iter()
                .all(|ip| ip.parse::<std::net::IpAddr>().unwrap().is_ipv4()));
        }

        #[tokio::test]
        async fn resolve_addr_unresolvable() {
            let err = resolve_addr("this-does-not-exist-xyz.invalid", 443, false, false)
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

        /// The follow-up connection must measure QUIC session resumption /
        /// 0-RTT and populate the additive tls fields. Acceptance itself is
        /// not asserted — a server may legitimately reject early data — only
        /// the attempted/populated semantics.
        #[cfg(not(target_os = "windows"))]
        #[tokio::test]
        async fn h3_probe_measures_quic_resumption() {
            let ep = TestEndpoint::start().await;
            ep.wait_for_quic().await;
            let target = ep.https_url("/health");
            let a = run_http3_probe(Uuid::new_v4(), 0, &target, 10_000, true, None).await;
            assert!(a.success, "H3 probe failed: {:?}", a.error);
            let tls = a.tls.expect("tls facts must be present");

            assert!(
                tls.zero_rtt_attempted.is_some(),
                "http3 attempts must always report whether 0-RTT was attempted"
            );
            assert!(
                tls.quic_resumed.is_some(),
                "http3 attempts must always report the resumption verdict"
            );
            if tls.zero_rtt_attempted == Some(true) {
                // With a 10s budget on loopback the handshake always
                // completes, so the verdict and timing must be recorded.
                assert!(
                    tls.zero_rtt_accepted.is_some(),
                    "attempted 0-RTT must record the server's accept/reject verdict"
                );
                let warm = tls
                    .quic_resumed_handshake_ms
                    .expect("attempted 0-RTT must time the resumed handshake");
                assert!(warm > 0.0, "resumed handshake time must be positive");
                assert_eq!(
                    tls.quic_resumed, tls.zero_rtt_accepted,
                    "resumption is verified via early-data acceptance"
                );
            } else {
                assert_eq!(
                    tls.quic_resumed,
                    Some(false),
                    "no ticket → resumption not attempted"
                );
            }
        }

        /// Throughput/pageload H3 variants reuse the request runner but must
        /// NOT pay for (or report) the extra resumption connection.
        #[cfg(not(target_os = "windows"))]
        #[tokio::test]
        async fn h3_request_probe_skips_resumption_for_non_http3_protocols() {
            let ep = TestEndpoint::start().await;
            ep.wait_for_quic().await;
            let target = ep.https_url("/download?bytes=1024");
            let a = run_http3_request_probe(
                Uuid::new_v4(),
                0,
                Protocol::Download3,
                &target,
                0,
                &crate::runner::http::RunConfig {
                    timeout_ms: 10_000,
                    insecure: true,
                    ..Default::default()
                },
            )
            .await;
            assert!(a.success, "H3 download failed: {:?}", a.error);
            let tls = a.tls.expect("tls facts must be present");
            assert!(tls.zero_rtt_attempted.is_none());
            assert!(tls.quic_resumed.is_none());
            assert!(tls.zero_rtt_accepted.is_none());
            assert!(tls.quic_resumed_handshake_ms.is_none());
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

        // ── QuicStats mapping (deep-measurement M1 B.1 / M3 G1) ──────────────

        /// The mapping must carry every PathStats/UdpStats field through
        /// verbatim — no unit conversion beyond rtt→ms, and cwnd stays in
        /// BYTES (RFC 9002), never faked into segments.
        #[test]
        fn quic_stats_from_maps_all_fields_verbatim() {
            let mut stats = quinn::ConnectionStats::default();
            stats.path.rtt = std::time::Duration::from_micros(12_340);
            stats.path.cwnd = 98_765;
            stats.path.current_mtu = 1_452;
            stats.path.lost_packets = 3;
            stats.path.lost_bytes = 3_600;
            stats.path.sent_packets = 210;
            stats.path.congestion_events = 2;
            stats.path.sent_plpmtud_probes = 5;
            stats.path.lost_plpmtud_probes = 1;
            stats.path.black_holes_detected = 1;
            stats.udp_tx.datagrams = 180;
            stats.udp_tx.bytes = 42_000;
            stats.udp_rx.datagrams = 150;
            stats.udp_rx.bytes = 1_048_576;

            let q = quic_stats_from(&stats);
            assert_eq!(q.rtt_ms, Some(12.34));
            assert_eq!(q.cwnd_bytes, Some(98_765));
            assert_eq!(q.current_mtu, Some(1_452));
            assert_eq!(q.lost_packets, Some(3));
            assert_eq!(q.lost_bytes, Some(3_600));
            assert_eq!(q.sent_packets, Some(210));
            assert_eq!(q.congestion_events, Some(2));
            assert_eq!(q.sent_plpmtud_probes, Some(5));
            assert_eq!(q.lost_plpmtud_probes, Some(1));
            assert_eq!(q.black_holes_detected, Some(1));
            assert_eq!(q.udp_tx_datagrams, Some(180));
            assert_eq!(q.udp_tx_bytes, Some(42_000));
            assert_eq!(q.udp_rx_datagrams, Some(150));
            assert_eq!(q.udp_rx_bytes, Some(1_048_576));
        }

        /// The congestion-algorithm label must be honestly marked as client
        /// configuration (quinn exposes no runtime controller query), so it
        /// can never be mistaken for a kernel-queried `TCP_CONGESTION` fact.
        #[test]
        fn quic_stats_congestion_algorithm_is_config_labeled() {
            let q = quic_stats_from(&quinn::ConnectionStats::default());
            let algo = q.congestion_algorithm.expect("always recorded");
            assert_eq!(algo, QUIC_CONGESTION_ALGORITHM);
            assert!(
                algo.contains("(client-config)"),
                "label must mark the value as config-not-kernel: {algo}"
            );
        }

        /// A live h3 probe must attach the post-transfer QUIC snapshot to the
        /// PRIMARY connection: packets were actually exchanged, so sent
        /// packets / UDP datagram counts must be non-zero, and the DPLPMTUD
        /// MTU must be at least QUIC's 1200-byte minimum (RFC 9000 §8.1).
        #[cfg(not(target_os = "windows"))]
        #[tokio::test]
        async fn h3_probe_carries_primary_quic_stats() {
            let ep = TestEndpoint::start().await;
            ep.wait_for_quic().await;
            let target = ep.https_url("/health");
            let a = run_http3_probe(Uuid::new_v4(), 0, &target, 10_000, true, None).await;
            assert!(a.success, "H3 probe failed: {:?}", a.error);
            let http = a.http.expect("http result");
            let q = http
                .quic_stats
                .expect("h3 attempts must carry the primary connection's quic_stats");
            assert!(q.sent_packets.unwrap() > 0, "packets were exchanged");
            assert!(q.udp_tx_datagrams.unwrap() > 0);
            assert!(q.udp_rx_datagrams.unwrap() > 0);
            assert!(q.udp_rx_bytes.unwrap() > 0, "response body was received");
            assert!(q.cwnd_bytes.unwrap() > 0);
            assert!(
                q.current_mtu.unwrap() >= 1200,
                "QUIC guarantees a 1200-byte minimum UDP payload"
            );
            assert!(q.rtt_ms.unwrap() > 0.0);
            // Loopback: nothing should be declared lost.
            assert_eq!(q.lost_packets, Some(0));

            // The plain http3 probe also runs the resumption follow-up
            // connection; when 0-RTT was attempted a live connection existed
            // and its own snapshot must be recorded separately.
            let tls = a.tls.expect("tls facts");
            if tls.zero_rtt_attempted == Some(true) {
                let rq = http
                    .quic_resumption_stats
                    .expect("attempted 0-RTT must record the follow-up connection's stats");
                assert!(rq.sent_packets.unwrap() > 0);
            } else {
                assert!(http.quic_resumption_stats.is_none());
            }
        }

        /// Throughput variants (download3/upload3) reuse the request runner:
        /// they must carry primary quic_stats but never the resumption
        /// follow-up's (they skip that extra connection).
        #[cfg(not(target_os = "windows"))]
        #[tokio::test]
        async fn h3_download_carries_quic_stats_without_resumption_stats() {
            let ep = TestEndpoint::start().await;
            ep.wait_for_quic().await;
            let target = ep.https_url("/download?bytes=65536");
            let a = run_http3_request_probe(
                Uuid::new_v4(),
                0,
                Protocol::Download3,
                &target,
                0,
                &crate::runner::http::RunConfig {
                    timeout_ms: 10_000,
                    insecure: true,
                    ..Default::default()
                },
            )
            .await;
            assert!(a.success, "H3 download failed: {:?}", a.error);
            let http = a.http.expect("http result");
            let q = http.quic_stats.expect("download3 must carry quic_stats");
            assert!(q.sent_packets.unwrap() > 0);
            assert!(
                q.udp_rx_bytes.unwrap() >= 65_536,
                "at least the payload must have arrived over UDP"
            );
            assert!(http.quic_resumption_stats.is_none());
        }
    }
}
