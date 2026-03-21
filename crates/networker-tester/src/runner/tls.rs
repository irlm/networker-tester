/// Standalone TLS probe: DNS + TCP connect + TLS handshake, no HTTP request.
///
/// Collects the full certificate chain (all certs with SANs), negotiated
/// cipher suite, TLS version, and ALPN protocol.  Advertises both `h2` and
/// `http/1.1` in ALPN so the server picks its preferred protocol.
use crate::metrics::{CertEntry, ErrorCategory, ErrorRecord, Protocol, RequestAttempt, TlsResult};
use crate::runner::http::RunConfig;
use crate::runner::{dns as dns_runner, socket_info::SocketInfo};
use chrono::Utc;
use rustls::pki_types::ServerName;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::debug;
use uuid::Uuid;

/// Run one standalone TLS probe and return a fully populated `RequestAttempt`.
pub async fn run_tls_probe(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Config,
                "Target URL has no host".into(),
                None,
                None,
                None,
            );
        }
    };

    let default_port = if target.scheme() == "https" {
        443u16
    } else {
        80
    };
    let port = target.port().unwrap_or(default_port);

    // ── 1. DNS ────────────────────────────────────────────────────────────────
    let (addr, dns_result) = if cfg.dns_enabled {
        match dns_runner::resolve(&host, cfg.ipv4_only, cfg.ipv6_only).await {
            Ok((ips, r)) => {
                let ip = ips
                    .iter()
                    .find(|ip| {
                        if cfg.ipv4_only {
                            ip.is_ipv4()
                        } else {
                            ip.is_ipv6() || ip.is_ipv4()
                        }
                    })
                    .copied()
                    .unwrap_or(ips[0]);
                (SocketAddr::new(ip, port), Some(r))
            }
            Err(e) => {
                return make_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    e.category,
                    e.message,
                    e.detail,
                    None,
                    None,
                );
            }
        }
    } else {
        match host.parse::<std::net::IpAddr>() {
            Ok(ip) => (SocketAddr::new(ip, port), None),
            Err(_) => {
                return make_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Config,
                    format!("dns_enabled=false but '{host}' is not a valid IP"),
                    None,
                    None,
                    None,
                );
            }
        }
    };

    // ── 2. TCP connect ────────────────────────────────────────────────────────
    let tcp_started_at = Utc::now();
    let t_tcp = Instant::now();
    let tcp_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tcp,
                e.to_string(),
                Some(format!("connect to {addr}")),
                dns_result,
                None,
            );
        }
        Err(_) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Timeout,
                format!("TCP connect to {addr} timed out after {}ms", cfg.timeout_ms),
                None,
                dns_result,
                None,
            );
        }
    };
    let tcp_duration_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let sock_info = SocketInfo::from_stream(&tcp_stream);
    let tcp_result = crate::metrics::TcpResult {
        local_addr,
        remote_addr: addr.to_string(),
        connect_duration_ms: tcp_duration_ms,
        attempt_count: 1,
        started_at: tcp_started_at,
        success: true,
        mss_bytes: sock_info.mss_bytes,
        rtt_estimate_ms: sock_info.rtt_estimate_ms,
        retransmits: sock_info.retransmits,
        total_retrans: sock_info.total_retrans,
        snd_cwnd: sock_info.snd_cwnd,
        snd_ssthresh: sock_info.snd_ssthresh,
        rtt_variance_ms: sock_info.rtt_variance_ms,
        rcv_space: sock_info.rcv_space,
        segs_out: sock_info.segs_out,
        segs_in: sock_info.segs_in,
        congestion_algorithm: sock_info.congestion_algorithm,
        delivery_rate_bps: sock_info.delivery_rate_bps,
        min_rtt_ms: sock_info.min_rtt_ms,
    };
    debug!("TLS probe: TCP connected to {addr} in {tcp_duration_ms:.1}ms");

    // ── 3. TLS handshake ──────────────────────────────────────────────────────
    let tls_started_at = Utc::now();
    let t_tls = Instant::now();

    let tls_config = match build_tls_config_for_probe(cfg.insecure, cfg.ca_bundle.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tls,
                e.to_string(),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };
    let connector = TlsConnector::from(Arc::new(tls_config));

    let server_name = match ServerName::try_from(host.clone()) {
        Ok(n) => n,
        Err(e) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tls,
                format!("Invalid SNI: {e}"),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };

    let tls_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        connector.connect(server_name, tcp_stream),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tls,
                e.to_string(),
                Some("TLS handshake".into()),
                dns_result,
                Some(tcp_result),
            );
        }
        Err(_) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Timeout,
                format!("TLS handshake timed out after {}ms", cfg.timeout_ms),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };
    let tls_duration_ms = t_tls.elapsed().as_secs_f64() * 1000.0;
    debug!("TLS probe: handshake done in {tls_duration_ms:.1}ms");

    let tls_result = extract_tls_probe_info(&tls_stream, tls_started_at, tls_duration_ms);

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Tls,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: true,
        dns: dns_result,
        tcp: Some(tcp_result),
        tls: Some(tls_result),
        http: None,
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
    }
}

/// Run a two-connection TLS resumption probe.
///
/// The first connection performs a real HTTP/1.1 request so TLS 1.3 session
/// tickets have a chance to arrive post-handshake. The second connection uses
/// the same rustls ClientConfig/resumption store and succeeds only if the
/// handshake is classified as resumed.
pub async fn run_tls_resumption_probe(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return make_failed_resume(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Config,
                "Target URL has no host".into(),
                None,
                None,
                None,
            );
        }
    };

    if target.scheme() != "https" {
        return make_failed_resume(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            ErrorCategory::Config,
            "tlsresume requires an https:// target".into(),
            None,
            None,
            None,
        );
    }
    let port = target.port().unwrap_or(443);
    let (addr, dns_result) = if cfg.dns_enabled {
        match dns_runner::resolve(&host, cfg.ipv4_only, cfg.ipv6_only).await {
            Ok((ips, r)) => {
                let ip = ips
                    .iter()
                    .find(|ip| {
                        if cfg.ipv4_only {
                            ip.is_ipv4()
                        } else {
                            ip.is_ipv6() || ip.is_ipv4()
                        }
                    })
                    .copied()
                    .unwrap_or(ips[0]);
                (SocketAddr::new(ip, port), Some(r))
            }
            Err(e) => {
                return make_failed_resume(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    e.category,
                    e.message,
                    e.detail,
                    None,
                    None,
                );
            }
        }
    } else {
        match host.parse::<std::net::IpAddr>() {
            Ok(ip) => (SocketAddr::new(ip, port), None),
            Err(_) => {
                return make_failed_resume(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Config,
                    format!("dns_enabled=false but '{host}' is not a valid IP"),
                    None,
                    None,
                    None,
                );
            }
        }
    };

    let tls_config = match build_tls_config_for_http1_probe(cfg.insecure, cfg.ca_bundle.as_deref())
    {
        Ok(c) => Arc::new(c),
        Err(e) => {
            return make_failed_resume(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tls,
                e.to_string(),
                None,
                dns_result,
                None,
            );
        }
    };

    let first = match run_one_tls_http_request(addr, &host, target, cfg, tls_config.clone()).await {
        Ok(v) => v,
        Err((category, message, detail, tcp_result)) => {
            return make_failed_resume(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                category,
                message,
                detail,
                dns_result,
                tcp_result,
            );
        }
    };

    let second = match run_one_tls_http_request(addr, &host, target, cfg, tls_config.clone()).await
    {
        Ok(v) => v,
        Err((category, message, detail, tcp_result)) => {
            return make_failed_resume(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                category,
                format!("warm/resumption attempt failed: {message}"),
                detail,
                dns_result,
                tcp_result,
            );
        }
    };

    let mut tls_result = second.tls;
    tls_result.previous_handshake_duration_ms = Some(first.tls.handshake_duration_ms);
    tls_result.previous_handshake_kind = first.tls.handshake_kind.clone();
    tls_result.previous_http_status_code = first.http_status_code;
    tls_result.http_status_code = second.http_status_code;

    let resumed = tls_result.resumed.unwrap_or(false);
    let success = resumed;
    let error = if success {
        None
    } else {
        Some(ErrorRecord {
            category: ErrorCategory::Tls,
            message: format!(
                "TLS session was not resumed on the second connection (cold={}, warm={})",
                first.tls.handshake_kind.as_deref().unwrap_or("unknown"),
                tls_result.handshake_kind.as_deref().unwrap_or("unknown")
            ),
            detail: Some(format!(
                "cold_status={:?} warm_status={:?} cold_tickets={:?} warm_tickets={:?}",
                first.http_status_code,
                second.http_status_code,
                first.tls.tls13_tickets_received,
                tls_result.tls13_tickets_received
            )),
            occurred_at: Utc::now(),
        })
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::TlsResume,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success,
        dns: dns_result,
        tcp: Some(second.tcp),
        tls: Some(tls_result),
        http: None,
        udp: None,
        error,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
    }
}

struct TlsHttpRequestResult {
    tcp: crate::metrics::TcpResult,
    tls: TlsResult,
    http_status_code: Option<u16>,
}

async fn run_one_tls_http_request(
    addr: SocketAddr,
    host: &str,
    target: &url::Url,
    cfg: &RunConfig,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<
    TlsHttpRequestResult,
    (
        ErrorCategory,
        String,
        Option<String>,
        Option<crate::metrics::TcpResult>,
    ),
> {
    let tcp_started_at = Utc::now();
    let t_tcp = Instant::now();
    let tcp_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err((
                ErrorCategory::Tcp,
                e.to_string(),
                Some(format!("connect to {addr}")),
                None,
            ))
        }
        Err(_) => {
            return Err((
                ErrorCategory::Timeout,
                format!("TCP connect to {addr} timed out after {}ms", cfg.timeout_ms),
                None,
                None,
            ))
        }
    };
    let tcp_duration_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let local_addr = tcp_stream.local_addr().ok().map(|a| a.to_string());
    let sock_info = SocketInfo::from_stream(&tcp_stream);
    let tcp_result = crate::metrics::TcpResult {
        local_addr,
        remote_addr: addr.to_string(),
        connect_duration_ms: tcp_duration_ms,
        attempt_count: 1,
        started_at: tcp_started_at,
        success: true,
        mss_bytes: sock_info.mss_bytes,
        rtt_estimate_ms: sock_info.rtt_estimate_ms,
        retransmits: sock_info.retransmits,
        total_retrans: sock_info.total_retrans,
        snd_cwnd: sock_info.snd_cwnd,
        snd_ssthresh: sock_info.snd_ssthresh,
        rtt_variance_ms: sock_info.rtt_variance_ms,
        rcv_space: sock_info.rcv_space,
        segs_out: sock_info.segs_out,
        segs_in: sock_info.segs_in,
        congestion_algorithm: sock_info.congestion_algorithm,
        delivery_rate_bps: sock_info.delivery_rate_bps,
        min_rtt_ms: sock_info.min_rtt_ms,
    };

    let server_name = ServerName::try_from(host.to_string()).map_err(|e| {
        (
            ErrorCategory::Tls,
            format!("Invalid SNI: {e}"),
            None,
            Some(tcp_result.clone()),
        )
    })?;
    let connector = TlsConnector::from(tls_config);
    let tls_started_at = Utc::now();
    let t_tls = Instant::now();
    let mut tls_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        connector.connect(server_name, tcp_stream),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err((
                ErrorCategory::Tls,
                e.to_string(),
                Some("TLS handshake".into()),
                Some(tcp_result),
            ))
        }
        Err(_) => {
            return Err((
                ErrorCategory::Timeout,
                format!("TLS handshake timed out after {}ms", cfg.timeout_ms),
                None,
                Some(tcp_result),
            ))
        }
    };
    let tls_duration_ms = t_tls.elapsed().as_secs_f64() * 1000.0;

    let mut request_path = target.path().to_string();
    if request_path.is_empty() {
        request_path = "/".to_string();
    }
    if let Some(q) = target.query() {
        request_path.push('?');
        request_path.push_str(q);
    }
    if request_path.contains(['\r', '\n']) || host.contains(['\r', '\n']) {
        return Err((
            ErrorCategory::Config,
            "Target URL contains invalid characters (CR/LF) in path or host".into(),
            None,
            Some(tcp_result.clone()),
        ));
    }
    let request = format!(
        "GET {request_path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: networker-tester/tlsresume\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        tls_stream.write_all(request.as_bytes()),
    )
    .await
    .map_err(|_| {
        (
            ErrorCategory::Timeout,
            format!("HTTP request write timed out after {}ms", cfg.timeout_ms),
            None,
            Some(tcp_result.clone()),
        )
    })?
    .map_err(|e| {
        (
            ErrorCategory::Http,
            e.to_string(),
            Some("write HTTP request over TLS".into()),
            Some(tcp_result.clone()),
        )
    })?;
    let _ = tls_stream.flush().await;

    // Read the response up to a cap.  We need to consume enough data for
    // TLS 1.3 NewSessionTicket messages to be processed by rustls, but we
    // must not allow an adversarial server to exhaust memory.
    const MAX_RESPONSE_BYTES: usize = 256 * 1024; // 256 KiB
    let mut buf = Vec::with_capacity(8 * 1024);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(cfg.timeout_ms), async {
        loop {
            let mut chunk = [0u8; 8192];
            match tls_stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let remaining = MAX_RESPONSE_BYTES.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..n.min(remaining)]);
                    if buf.len() >= MAX_RESPONSE_BYTES {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
    .await;

    let http_status_code = parse_http_status_code(&buf);
    let mut tls_result = extract_tls_probe_info(&tls_stream, tls_started_at, tls_duration_ms);
    tls_result.tls13_tickets_received = Some(tls_stream.get_ref().1.tls13_tickets_received());
    tls_result.handshake_kind = tls_stream
        .get_ref()
        .1
        .handshake_kind()
        .map(|k| handshake_kind_label(k).to_string());
    tls_result.resumed = Some(matches!(
        tls_stream.get_ref().1.handshake_kind(),
        Some(rustls::HandshakeKind::Resumed)
    ));
    tls_result.http_status_code = http_status_code;

    Ok(TlsHttpRequestResult {
        tcp: tcp_result,
        tls: tls_result,
        http_status_code,
    })
}

fn parse_http_status_code(buf: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(buf).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let _http = parts.next()?;
    parts.next()?.parse().ok()
}

fn handshake_kind_label(kind: rustls::HandshakeKind) -> &'static str {
    match kind {
        rustls::HandshakeKind::Full => "full",
        rustls::HandshakeKind::FullWithHelloRetryRequest => "full-hrr",
        rustls::HandshakeKind::Resumed => "resumed",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TLS config for probe (advertises both h2 + http/1.1)
// ─────────────────────────────────────────────────────────────────────────────

fn build_tls_config_for_http1_probe(
    insecure: bool,
    ca_bundle: Option<&str>,
) -> anyhow::Result<rustls::ClientConfig> {
    build_tls_config_for_probe_with_alpn(insecure, ca_bundle, vec![b"http/1.1".to_vec()])
}

fn build_tls_config_for_probe(
    insecure: bool,
    ca_bundle: Option<&str>,
) -> anyhow::Result<rustls::ClientConfig> {
    build_tls_config_for_probe_with_alpn(
        insecure,
        ca_bundle,
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
    )
}

fn build_tls_config_for_probe_with_alpn(
    insecure: bool,
    ca_bundle: Option<&str>,
    alpn_protocols: Vec<Vec<u8>>,
) -> anyhow::Result<rustls::ClientConfig> {
    let mut config = if insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            let _ = root_store.add(cert);
        }
        if let Some(bundle_path) = ca_bundle {
            load_ca_bundle(&mut root_store, bundle_path)?;
        }
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };
    config.alpn_protocols = alpn_protocols;
    Ok(config)
}

pub(crate) fn load_ca_bundle(
    root_store: &mut rustls::RootCertStore,
    path: &str,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let pem_data = std::fs::read(path).with_context(|| format!("Cannot read CA bundle: {path}"))?;
    let mut cursor = std::io::BufReader::new(pem_data.as_slice());
    let certs: Vec<_> = rustls_pemfile::certs(&mut cursor)
        .collect::<Result<_, _>>()
        .with_context(|| format!("Failed to parse PEM certs in {path}"))?;
    if certs.is_empty() {
        anyhow::bail!("No PEM certificates found in {path}");
    }
    for cert in certs {
        root_store
            .add(cert)
            .with_context(|| "Failed to add cert to root store")?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// TLS info extraction with full cert chain
// ─────────────────────────────────────────────────────────────────────────────

fn extract_tls_probe_info(
    stream: &tokio_rustls::client::TlsStream<TcpStream>,
    started_at: chrono::DateTime<Utc>,
    duration_ms: f64,
) -> TlsResult {
    let (_, conn) = stream.get_ref();

    let protocol_version = conn
        .protocol_version()
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "unknown".into());

    let cipher_suite = conn
        .negotiated_cipher_suite()
        .map(|c| format!("{:?}", c.suite()))
        .unwrap_or_else(|| "unknown".into());

    let alpn_negotiated = conn
        .alpn_protocol()
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(String::from);

    let cert_chain = extract_full_cert_chain(conn);

    let (cert_subject, cert_issuer, cert_expiry) = cert_chain
        .first()
        .map(|c| (Some(c.subject.clone()), Some(c.issuer.clone()), c.expiry))
        .unwrap_or((None, None, None));

    let handshake_kind = conn
        .handshake_kind()
        .map(|k| handshake_kind_label(k).to_string());
    let resumed = Some(matches!(
        conn.handshake_kind(),
        Some(rustls::HandshakeKind::Resumed)
    ));
    TlsResult {
        protocol_version,
        cipher_suite,
        alpn_negotiated,
        cert_subject,
        cert_issuer,
        cert_expiry,
        handshake_duration_ms: duration_ms,
        started_at,
        success: true,
        cert_chain,
        tls_backend: Some("rustls".into()),
        resumed,
        handshake_kind,
        tls13_tickets_received: Some(conn.tls13_tickets_received()),
        previous_handshake_duration_ms: None,
        previous_handshake_kind: None,
        previous_http_status_code: None,
        http_status_code: None,
    }
}

fn extract_full_cert_chain(conn: &rustls::ClientConnection) -> Vec<CertEntry> {
    conn.peer_certificates()
        .map(|certs| {
            certs
                .iter()
                .filter_map(|c| parse_cert_entry(c.as_ref()))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_cert_entry(der: &[u8]) -> Option<CertEntry> {
    use x509_parser::prelude::*;
    let (_, cert) = X509Certificate::from_der(der).ok()?;

    let subject = cert.subject().to_string();
    let issuer = cert.issuer().to_string();
    let expiry = chrono::DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0);

    let sans = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|gn| match gn {
                    GeneralName::DNSName(name) => Some(name.to_string()),
                    GeneralName::IPAddress(ip) => match ip.len() {
                        4 => {
                            let octets: [u8; 4] = (*ip).try_into().ok()?;
                            Some(std::net::Ipv4Addr::from(octets).to_string())
                        }
                        16 => {
                            let octets: [u8; 16] = (*ip).try_into().ok()?;
                            Some(std::net::Ipv6Addr::from(octets).to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(CertEntry {
        subject,
        issuer,
        expiry,
        sans,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// NoVerifier (for --insecure)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer,
        _intermediates: &[rustls::pki_types::CertificateDer],
        _server_name: &rustls::pki_types::ServerName,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error helper
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn make_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
    dns: Option<crate::metrics::DnsResult>,
    tcp: Option<crate::metrics::TcpResult>,
) -> RequestAttempt {
    make_failed_with_protocol(
        run_id,
        attempt_id,
        sequence_num,
        started_at,
        category,
        message,
        detail,
        dns,
        tcp,
        Protocol::Tls,
    )
}

#[allow(clippy::too_many_arguments)]
fn make_failed_resume(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
    dns: Option<crate::metrics::DnsResult>,
    tcp: Option<crate::metrics::TcpResult>,
) -> RequestAttempt {
    make_failed_with_protocol(
        run_id,
        attempt_id,
        sequence_num,
        started_at,
        category,
        message,
        detail,
        dns,
        tcp,
        Protocol::TlsResume,
    )
}

#[allow(clippy::too_many_arguments)]
fn make_failed_with_protocol(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
    dns: Option<crate::metrics::DnsResult>,
    tcp: Option<crate::metrics::TcpResult>,
    protocol: Protocol,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: false,
        dns,
        tcp,
        tls: None,
        http: None,
        udp: None,
        error: Some(ErrorRecord {
            category,
            message,
            detail,
            occurred_at: Utc::now(),
        }),
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn self_signed_der() -> Vec<u8> {
        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .unwrap();
        cert.cert.der().to_vec()
    }

    // ── build_tls_config_for_probe ────────────────────────────────────────────

    #[test]
    fn tls_config_insecure_builds_without_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = build_tls_config_for_probe(true, None).unwrap();
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn tls_config_secure_builds_without_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = build_tls_config_for_probe(false, None).unwrap();
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn tls_config_ca_bundle_not_found_returns_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let err = build_tls_config_for_probe(false, Some("/nonexistent/ca.pem")).unwrap_err();
        assert!(
            err.to_string().contains("Cannot read CA bundle")
                || err.to_string().contains("nonexistent")
        );
    }

    // ── load_ca_bundle ────────────────────────────────────────────────────────

    #[test]
    fn load_ca_bundle_file_not_found_returns_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut store = rustls::RootCertStore::empty();
        let err = load_ca_bundle(&mut store, "/nonexistent/ca.pem").unwrap_err();
        assert!(err.to_string().contains("Cannot read CA bundle"));
    }

    #[test]
    fn load_ca_bundle_empty_file_returns_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut store = rustls::RootCertStore::empty();
        let tmp = NamedTempFile::new().unwrap();
        let err = load_ca_bundle(&mut store, tmp.path().to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("No PEM certificates found"));
    }

    #[test]
    fn load_ca_bundle_invalid_pem_returns_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut store = rustls::RootCertStore::empty();
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"this is not valid PEM").unwrap();
        // Either "No PEM certificates found" or a parse error — both are errors
        assert!(load_ca_bundle(&mut store, tmp.path().to_str().unwrap()).is_err());
    }

    // ── parse_cert_entry ─────────────────────────────────────────────────────

    #[test]
    fn parse_cert_entry_invalid_der_returns_none() {
        assert!(parse_cert_entry(&[0u8; 32]).is_none());
    }

    #[test]
    fn parse_cert_entry_empty_returns_none() {
        assert!(parse_cert_entry(&[]).is_none());
    }

    #[test]
    fn parse_cert_entry_valid_cert_has_sans() {
        let der = self_signed_der();
        let entry = parse_cert_entry(&der).expect("should parse valid DER");
        assert!(
            entry.sans.iter().any(|s| s == "localhost"),
            "SANs should include 'localhost', got: {:?}",
            entry.sans
        );
        assert!(
            entry.sans.iter().any(|s| s == "127.0.0.1"),
            "SANs should include '127.0.0.1', got: {:?}",
            entry.sans
        );
    }

    #[test]
    fn parse_cert_entry_has_subject_and_issuer() {
        let der = self_signed_der();
        let entry = parse_cert_entry(&der).unwrap();
        assert!(!entry.subject.is_empty(), "subject should be non-empty");
        assert!(!entry.issuer.is_empty(), "issuer should be non-empty");
    }

    // ── make_failed ───────────────────────────────────────────────────────────

    #[test]
    fn make_failed_sets_tls_protocol_and_error() {
        let run_id = uuid::Uuid::new_v4();
        let attempt_id = uuid::Uuid::new_v4();
        let started_at = Utc::now();
        let a = make_failed(
            run_id,
            attempt_id,
            5,
            started_at,
            ErrorCategory::Tls,
            "handshake failed".to_string(),
            Some("detail text".to_string()),
            None,
            None,
        );
        assert!(!a.success);
        assert_eq!(a.run_id, run_id);
        assert_eq!(a.attempt_id, attempt_id);
        assert_eq!(a.sequence_num, 5);
        assert_eq!(a.protocol, Protocol::Tls);
        assert!(a.tls.is_none());
        assert!(a.dns.is_none());
        assert!(a.tcp.is_none());
        assert!(a.finished_at.is_some());
        assert_eq!(a.retry_count, 0);
        let err = a.error.expect("error must be set");
        assert_eq!(err.message, "handshake failed");
        assert_eq!(err.detail.as_deref(), Some("detail text"));
        assert_eq!(err.category, ErrorCategory::Tls);
    }

    #[test]
    fn make_failed_resume_sets_tls_resume_protocol() {
        let a = make_failed_resume(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            1,
            Utc::now(),
            ErrorCategory::Config,
            "tlsresume requires an https:// target".to_string(),
            None,
            None,
            None,
        );
        assert_eq!(a.protocol, Protocol::TlsResume);
        assert!(!a.success);
    }

    #[test]
    fn make_failed_no_detail() {
        let a = make_failed(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            0,
            Utc::now(),
            ErrorCategory::Tcp,
            "connect refused".to_string(),
            None,
            None,
            None,
        );
        assert!(a.error.as_ref().unwrap().detail.is_none());
    }

    #[test]
    fn parse_cert_entry_self_signed_expiry_is_some() {
        let der = self_signed_der();
        let entry = parse_cert_entry(&der).unwrap();
        assert!(entry.expiry.is_some(), "expiry should be present");
    }

    #[test]
    fn parse_http_status_code_extracts_status() {
        let resp = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(parse_http_status_code(resp), Some(403));
    }

    #[test]
    fn parse_http_status_code_returns_none_for_non_http() {
        assert_eq!(parse_http_status_code(b"not http"), None);
    }

    #[test]
    fn handshake_kind_label_maps_variants() {
        assert_eq!(handshake_kind_label(rustls::HandshakeKind::Full), "full");
        assert_eq!(
            handshake_kind_label(rustls::HandshakeKind::FullWithHelloRetryRequest),
            "full-hrr"
        );
        assert_eq!(
            handshake_kind_label(rustls::HandshakeKind::Resumed),
            "resumed"
        );
    }

    #[test]
    fn build_tls_config_for_http1_probe_only_advertises_http11() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = build_tls_config_for_http1_probe(true, None).expect("http1 tls config");
        assert_eq!(cfg.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }
}
