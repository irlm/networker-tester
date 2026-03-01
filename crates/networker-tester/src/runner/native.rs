#[cfg(feature = "native")]
use crate::metrics::{CertEntry, DnsResult, HttpResult, TcpResult, TlsResult};
/// Native-TLS probe: DNS + TCP + platform TLS + HTTP/1.1.
///
/// Uses the OS TLS stack via the `native-tls` crate:
///   macOS   → SecureTransport
///   Windows → SChannel
///   Linux   → OpenSSL
///
/// Enable with `--features native`.  Without the feature, probes return a
/// graceful error rather than failing to compile.
use crate::metrics::{ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
use crate::runner::http::RunConfig;
#[cfg(feature = "native")]
use crate::runner::{dns as dns_runner, socket_info::SocketInfo};
use chrono::Utc;
#[cfg(feature = "native")]
use std::net::SocketAddr;
#[cfg(feature = "native")]
use std::time::Instant;
#[cfg(feature = "native")]
use tokio::net::TcpStream;
#[cfg(feature = "native")]
use tracing::debug;
use uuid::Uuid;

/// Compile-time backend label embedded in `TlsResult.tls_backend`.
#[cfg(feature = "native")]
fn native_backend_name() -> &'static str {
    #[cfg(target_os = "windows")]
    return "native/schannel";
    #[cfg(target_os = "macos")]
    return "native/secure-transport";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    return "native/openssl";
}

/// Run one native-TLS probe and return a fully populated `RequestAttempt`.
pub async fn run_native_probe(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    #[cfg(not(feature = "native"))]
    {
        let _ = (target, cfg);
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Native,
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
                message: "native-tls probe requires '--features native' (recompile to enable)"
                    .into(),
                detail: None,
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
        }
    }

    #[cfg(feature = "native")]
    run_native_probe_impl(run_id, sequence_num, target, cfg).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Full implementation (feature = "native" only)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "native")]
async fn run_native_probe_impl(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::Request;
    use hyper_util::rt::TokioIo;

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

    let scheme = target.scheme();
    let default_port = if scheme == "https" { 443 } else { 80 };
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
    let tcp_result = TcpResult {
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
    debug!("native: TCP connected to {addr} in {tcp_duration_ms:.1}ms");

    // ── 3. TLS handshake (HTTPS only) or plain IO ─────────────────────────────
    // We split here: HTTPS uses native-tls; HTTP goes straight to hyper.
    if scheme == "https" {
        run_native_https(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            target,
            cfg,
            host,
            tcp_stream,
            tcp_result,
            dns_result,
        )
        .await
    } else {
        // HTTP: no TLS, plain TCP stream to hyper HTTP/1.1
        let tls_result = None;
        let io = TokioIo::new(tcp_stream);

        let path = {
            let p = target.path();
            let q = target.query();
            if let Some(q) = q {
                format!("{p}?{q}")
            } else {
                p.to_owned()
            }
        };

        let http_started_at = Utc::now();
        let t_http = Instant::now();

        let (mut sender, conn) = match tokio::time::timeout(
            std::time::Duration::from_millis(cfg.timeout_ms),
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io),
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                return make_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Http,
                    format!("HTTP/1.1 handshake failed: {e}"),
                    None,
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
                    "HTTP/1.1 handshake timed out".into(),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        };
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let req = match Request::builder()
            .method("GET")
            .uri(&path)
            .header("Host", &host)
            .header("User-Agent", "networker-tester/native")
            .body(Full::new(Bytes::new()))
        {
            Ok(r) => r,
            Err(e) => {
                return make_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Http,
                    format!("request build failed: {e}"),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        };

        let resp = match tokio::time::timeout(
            std::time::Duration::from_millis(cfg.timeout_ms),
            sender.send_request(req),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                return make_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Http,
                    e.to_string(),
                    None,
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
                    "HTTP request timed out".into(),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        };
        let ttfb_ms = t_http.elapsed().as_secs_f64() * 1000.0;
        let status_code = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect::<Vec<_>>();
        let headers_size = headers.iter().map(|(k, v)| k.len() + v.len() + 4).sum();
        let body = match resp.into_body().collect().await {
            Ok(b) => b.to_bytes(),
            Err(_) => Bytes::new(),
        };
        let total_ms = t_http.elapsed().as_secs_f64() * 1000.0;

        RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::Native,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: status_code < 400,
            dns: dns_result,
            tcp: Some(tcp_result),
            tls: tls_result,
            http: Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code,
                headers_size_bytes: headers_size,
                body_size_bytes: body.len(),
                ttfb_ms,
                total_duration_ms: total_ms,
                redirect_count: 0,
                started_at: http_started_at,
                response_headers: headers,
                payload_bytes: 0,
                throughput_mbps: None,
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
        }
    }
}

#[cfg(feature = "native")]
#[allow(clippy::too_many_arguments)]
async fn run_native_https(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    target: &url::Url,
    cfg: &RunConfig,
    host: String,
    tcp_stream: TcpStream,
    tcp_result: TcpResult,
    dns_result: Option<crate::metrics::DnsResult>,
) -> RequestAttempt {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::Request;
    use hyper_util::rt::TokioIo;
    use tokio_native_tls::TlsConnector;

    let tls_started_at = Utc::now();
    let t_tls = Instant::now();

    // Build native-tls connector
    let mut builder = native_tls::TlsConnector::builder();
    if cfg.insecure {
        builder.danger_accept_invalid_certs(true);
        builder.danger_accept_invalid_hostnames(true);
    }
    if let Some(ref bundle_path) = cfg.ca_bundle {
        match load_native_ca_bundle(bundle_path) {
            Ok(certs) => {
                for cert in certs {
                    let _ = builder.add_root_certificate(cert);
                }
            }
            Err(e) => {
                return make_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Tls,
                    format!("CA bundle error: {e}"),
                    None,
                    dns_result,
                    Some(tcp_result),
                );
            }
        }
    }

    let native_connector = match builder.build() {
        Ok(c) => TlsConnector::from(c),
        Err(e) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tls,
                format!("TLS connector build failed: {e}"),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };

    let tls_stream = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        native_connector.connect(&host, tcp_stream),
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
                Some("native-TLS handshake".into()),
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
                format!("native-TLS handshake timed out after {}ms", cfg.timeout_ms),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };
    let tls_duration_ms = t_tls.elapsed().as_secs_f64() * 1000.0;
    debug!("native: TLS handshake done in {tls_duration_ms:.1}ms");

    // Extract cert info from native TLS stream
    let tls_result = extract_native_tls_result(&tls_stream, tls_started_at, tls_duration_ms);

    // ── 4. HTTP/1.1 request ───────────────────────────────────────────────────
    let io = TokioIo::new(tls_stream);
    let path = {
        let p = target.path();
        let q = target.query();
        if let Some(q) = q {
            format!("{p}?{q}")
        } else {
            p.to_owned()
        }
    };

    let http_started_at = Utc::now();
    let t_http = Instant::now();

    let (mut sender, conn) = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Http,
                format!("HTTP/1.1 handshake failed: {e}"),
                None,
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
                "HTTP/1.1 handshake timed out".into(),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = match Request::builder()
        .method("GET")
        .uri(&path)
        .header("Host", &host)
        .header("User-Agent", "networker-tester/native")
        .body(Full::new(Bytes::new()))
    {
        Ok(r) => r,
        Err(e) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Http,
                format!("request build failed: {e}"),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };

    let resp = match tokio::time::timeout(
        std::time::Duration::from_millis(cfg.timeout_ms),
        sender.send_request(req),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return make_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Http,
                e.to_string(),
                None,
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
                "HTTP request timed out".into(),
                None,
                dns_result,
                Some(tcp_result),
            );
        }
    };
    let ttfb_ms = t_http.elapsed().as_secs_f64() * 1000.0;
    let status_code = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect::<Vec<_>>();
    let headers_size = headers.iter().map(|(k, v)| k.len() + v.len() + 4).sum();
    let body = match resp.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => bytes::Bytes::new(),
    };
    let total_ms = t_http.elapsed().as_secs_f64() * 1000.0;

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Native,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: status_code < 400,
        dns: dns_result,
        tcp: Some(tcp_result),
        tls: Some(tls_result),
        http: Some(HttpResult {
            negotiated_version: "HTTP/1.1".into(),
            status_code,
            headers_size_bytes: headers_size,
            body_size_bytes: body.len(),
            ttfb_ms,
            total_duration_ms: total_ms,
            redirect_count: 0,
            started_at: http_started_at,
            response_headers: headers,
            payload_bytes: 0,
            throughput_mbps: None,
        }),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TLS result extraction (native-tls)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "native")]
fn extract_native_tls_result(
    stream: &tokio_native_tls::TlsStream<TcpStream>,
    started_at: chrono::DateTime<Utc>,
    duration_ms: f64,
) -> TlsResult {
    let inner = stream.get_ref();

    // peer_certificate() gives us the leaf cert as DER.
    let cert_chain = inner
        .peer_certificate()
        .ok()
        .flatten()
        .and_then(|c| c.to_der().ok())
        .and_then(|der| parse_cert_entry(&der))
        .map(|entry| vec![entry])
        .unwrap_or_default();

    let (cert_subject, cert_issuer, cert_expiry) = cert_chain
        .first()
        .map(|c| (Some(c.subject.clone()), Some(c.issuer.clone()), c.expiry))
        .unwrap_or((None, None, None));

    TlsResult {
        // native-tls does not expose protocol version or cipher suite portably.
        protocol_version: "unknown".into(),
        cipher_suite: "unknown".into(),
        alpn_negotiated: None,
        cert_subject,
        cert_issuer,
        cert_expiry,
        handshake_duration_ms: duration_ms,
        started_at,
        success: true,
        cert_chain,
        tls_backend: Some(native_backend_name().into()),
    }
}

#[cfg(feature = "native")]
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

#[cfg(feature = "native")]
fn load_native_ca_bundle(path: &str) -> anyhow::Result<Vec<native_tls::Certificate>> {
    use anyhow::Context;
    let pem_data = std::fs::read(path).with_context(|| format!("Cannot read CA bundle: {path}"))?;
    // Split on -----BEGIN CERTIFICATE----- boundaries.
    let pem_str = std::str::from_utf8(&pem_data).with_context(|| "CA bundle is not valid UTF-8")?;
    let mut certs = Vec::new();
    let mut current = String::new();
    for line in pem_str.lines() {
        current.push_str(line);
        current.push('\n');
        if line.contains("-----END CERTIFICATE-----") {
            match native_tls::Certificate::from_pem(current.as_bytes()) {
                Ok(cert) => certs.push(cert),
                Err(e) => debug!("native: skipping CA cert parse error: {e}"),
            }
            current.clear();
        }
    }
    if certs.is_empty() {
        anyhow::bail!("No PEM certificates found in {path}");
    }
    Ok(certs)
}

// ─────────────────────────────────────────────────────────────────────────────
// Error helper (only needed when feature = "native" is active)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "native")]
#[allow(clippy::too_many_arguments)]
fn make_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
    dns: Option<DnsResult>,
    tcp: Option<TcpResult>,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::Native,
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
    }
}
