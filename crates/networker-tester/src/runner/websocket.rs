//! WebSocket probe (`websocket` mode) — upgrade time + message round-trips.
//!
//! Full timing ladder, then the two things only this probe can see:
//!
//! 1. **DNS / TCP / TLS** — the standard phases, reusing the same machinery
//!    (and the same trust-audit rules: TLS config built before the timer) as
//!    the standalone `tls` probe. TLS runs only for `wss://` (https targets)
//!    and advertises `http/1.1` in ALPN — the WS upgrade is an HTTP/1.1
//!    mechanism.
//! 2. **`upgrade_ms`** — the HTTP GET + `101 Switching Protocols` round-trip.
//! 3. **Echo messages** — N binary messages (count/size reuse `--udp-probes` /
//!    `--udp-payload` semantics) against the networker-endpoint's `/ws` echo
//!    route. Each message embeds `[seq u32 BE][timestamp_us i64 BE]`; echoes
//!    are credited to the message that sent them by the embedded sequence id
//!    (trust audit V12), so a late/reordered echo cannot desync the matcher.
//!    Aggregation (min/avg/p95, arrival-order jitter, loss) reuses
//!    [`aggregate_udp_rtts`].
//!
//! The target path is rewritten to `/ws` (like webdownload rewrites to
//! `/download`): the probe requires a networker-endpoint target.

use crate::metrics::{
    aggregate_udp_rtts, ErrorCategory, ErrorRecord, Protocol, RequestAttempt, WebSocketResult,
};
use crate::runner::dns as dns_runner;
use crate::runner::http::RunConfig;
use crate::runner::socket_info::SocketInfo;
use crate::runner::tls::{build_tls_config_for_http1_probe, extract_tls_probe_info};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tracing::debug;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WebSocketProbeConfig {
    /// Number of echo messages after the upgrade (`--udp-probes` semantics).
    pub message_count: u32,
    /// Bytes per echo message (min 12: seq + timestamp; `--udp-payload`).
    pub payload_size: usize,
    /// Per-message echo timeout in ms (`--udp-timeout` semantics).
    pub msg_timeout_ms: u64,
}

impl Default for WebSocketProbeConfig {
    fn default() -> Self {
        Self {
            message_count: 10,
            payload_size: 64,
            msg_timeout_ms: 5000,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_websocket_probe(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
    ws_cfg: &WebSocketProbeConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let host = match target.host_str() {
        Some(h) => h.to_string(),
        None => {
            return ws_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Config,
                "Target URL has no host".into(),
                None,
                None,
                None,
                None,
            );
        }
    };

    // Derive the WS scheme from the target: https/wss → wss, else ws. The
    // path is always the endpoint's /ws echo route.
    let secure = matches!(target.scheme(), "https" | "wss");
    let default_port = if secure { 443u16 } else { 80 };
    let port = target.port().unwrap_or(default_port);
    let ws_scheme = if secure { "wss" } else { "ws" };
    // `host` may be an IPv6 literal — url::Url::host_str keeps the brackets,
    // which is exactly what the request URL needs.
    let ws_url = format!("{ws_scheme}://{host}:{port}/ws");

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
                return ws_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    e.category,
                    e.message,
                    e.detail,
                    None,
                    None,
                    None,
                );
            }
        }
    } else {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        match bare.parse::<std::net::IpAddr>() {
            Ok(ip) => (SocketAddr::new(ip, port), None),
            Err(_) => {
                return ws_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Config,
                    format!("dns_enabled=false but '{host}' is not a valid IP"),
                    None,
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
        Duration::from_millis(cfg.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return ws_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Tcp,
                e.to_string(),
                Some(format!("connect to {addr}")),
                dns_result,
                None,
                None,
            );
        }
        Err(_) => {
            return ws_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                ErrorCategory::Timeout,
                format!("TCP connect to {addr} timed out after {}ms", cfg.timeout_ms),
                None,
                dns_result,
                None,
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
    debug!("websocket probe: TCP connected to {addr} in {tcp_duration_ms:.1}ms");

    // ── 3. Optional TLS handshake (wss) ──────────────────────────────────────
    // Config is built BEFORE the handshake timer starts (trust audit V5).
    let (tls_result, session) = if secure {
        let (tls_config, _ocsp) =
            match build_tls_config_for_http1_probe(cfg.insecure, cfg.ca_bundle.as_deref()) {
                Ok(c) => c,
                Err(e) => {
                    return ws_failed(
                        run_id,
                        attempt_id,
                        sequence_num,
                        started_at,
                        ErrorCategory::Tls,
                        e.to_string(),
                        None,
                        dns_result,
                        Some(tcp_result),
                        None,
                    );
                }
            };
        let connector = TlsConnector::from(Arc::new(tls_config));
        let bare_host = host.trim_start_matches('[').trim_end_matches(']');
        let server_name = match ServerName::try_from(bare_host.to_string()) {
            Ok(n) => n,
            Err(e) => {
                return ws_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Tls,
                    format!("Invalid SNI: {e}"),
                    None,
                    dns_result,
                    Some(tcp_result),
                    None,
                );
            }
        };
        let tls_started_at = Utc::now();
        let t_tls = Instant::now();
        let tls_stream = match tokio::time::timeout(
            Duration::from_millis(cfg.timeout_ms),
            connector.connect(server_name, tcp_stream),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return ws_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Tls,
                    e.to_string(),
                    Some("TLS handshake".into()),
                    dns_result,
                    Some(tcp_result),
                    None,
                );
            }
            Err(_) => {
                return ws_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    ErrorCategory::Timeout,
                    format!("TLS handshake timed out after {}ms", cfg.timeout_ms),
                    None,
                    dns_result,
                    Some(tcp_result),
                    None,
                );
            }
        };
        let tls_duration_ms = t_tls.elapsed().as_secs_f64() * 1000.0;
        let tls_result = extract_tls_probe_info(&tls_stream, tls_started_at, tls_duration_ms);
        debug!("websocket probe: TLS handshake done in {tls_duration_ms:.1}ms");
        (
            Some(tls_result),
            ws_session(tls_stream, &ws_url, cfg.timeout_ms, ws_cfg).await,
        )
    } else {
        (
            None,
            ws_session(tcp_stream, &ws_url, cfg.timeout_ms, ws_cfg).await,
        )
    };

    let session = match session {
        Ok(s) => s,
        Err((category, message)) => {
            return ws_failed(
                run_id,
                attempt_id,
                sequence_num,
                started_at,
                category,
                message,
                None,
                dns_result,
                Some(tcp_result),
                tls_result,
            );
        }
    };

    let stats = aggregate_udp_rtts(&session.msg_rtts);
    let echo_count = session.msg_rtts.iter().filter(|r| r.is_some()).count() as u32;
    let message_count = session.msg_rtts.len() as u32;

    let result = WebSocketResult {
        url: ws_url,
        upgrade_ms: session.upgrade_ms,
        upgrade_status: session.upgrade_status,
        message_count,
        echo_count,
        loss_percent: stats.loss_percent,
        msg_rtt_min_ms: stats.min,
        msg_rtt_avg_ms: stats.avg,
        msg_rtt_p95_ms: stats.p95,
        jitter_ms: stats.jitter,
        msg_rtts_ms: session.msg_rtts,
        payload_size: ws_cfg.payload_size.max(12),
        started_at,
    };

    // Same rule as the udp/ping probes: all echoes lost = failure (the
    // upgrade succeeded, but the steady-state measurement has no samples).
    let success = echo_count > 0;
    let error = if success {
        None
    } else {
        Some(ErrorRecord {
            category: ErrorCategory::Http,
            message: format!(
                "WebSocket upgrade succeeded but all {message_count} echo messages timed out \
                 after {}ms each — is the target's /ws route an echo server?",
                ws_cfg.msg_timeout_ms
            ),
            detail: None,
            occurred_at: Utc::now(),
        })
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::WebSocket,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success,
        dns: dns_result,
        tcp: Some(tcp_result),
        tls: tls_result,
        http: None,
        udp: None,
        error,
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
        websocket: Some(result),
        pmtud: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WS session: upgrade + echo loop (generic over the underlying stream)
// ─────────────────────────────────────────────────────────────────────────────

struct WsSessionOutcome {
    upgrade_ms: f64,
    upgrade_status: Option<u16>,
    /// Per-message RTTs in send order; `None` = echo never arrived.
    msg_rtts: Vec<Option<f64>>,
}

async fn ws_session<S>(
    stream: S,
    ws_url: &str,
    upgrade_timeout_ms: u64,
    cfg: &WebSocketProbeConfig,
) -> Result<WsSessionOutcome, (ErrorCategory, String)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // ── HTTP 101 upgrade ─────────────────────────────────────────────────────
    let t_upgrade = Instant::now();
    let (mut ws, response) = match tokio::time::timeout(
        Duration::from_millis(upgrade_timeout_ms),
        tokio_tungstenite::client_async(ws_url, stream),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            return Err((
                ErrorCategory::Http,
                format!("WebSocket upgrade failed: {e}"),
            ))
        }
        Err(_) => {
            return Err((
                ErrorCategory::Timeout,
                format!("WebSocket upgrade timed out after {upgrade_timeout_ms}ms"),
            ))
        }
    };
    let upgrade_ms = t_upgrade.elapsed().as_secs_f64() * 1000.0;
    let upgrade_status = Some(response.status().as_u16());
    debug!(
        "websocket probe: upgrade {} in {upgrade_ms:.1}ms",
        response.status()
    );

    // ── Echo loop ────────────────────────────────────────────────────────────
    // Back-to-back like the udp probe: the next message fires when the
    // previous echo arrives or times out. Echoes are matched by the embedded
    // sequence id so late/reordered echoes are credited correctly (V12).
    let count = cfg.message_count.max(1) as usize;
    let payload_size = cfg.payload_size.max(12);
    let mut send_times: Vec<Option<Instant>> = vec![None; count];
    let mut msg_rtts: Vec<Option<f64>> = vec![None; count];

    for seq in 0..count {
        let mut payload = vec![0u8; payload_size];
        payload[0..4].copy_from_slice(&(seq as u32).to_be_bytes());
        let now_us = Utc::now().timestamp_micros();
        payload[4..12].copy_from_slice(&now_us.to_be_bytes());

        let sent_at = Instant::now();
        send_times[seq] = Some(sent_at);
        if let Err(e) = ws.send(Message::binary(payload)).await {
            debug!("websocket probe: send #{seq} failed: {e}");
            // The socket is gone; everything not yet echoed stays lost.
            break;
        }

        // Wait for THIS message's echo (crediting any other echo that shows
        // up meanwhile) until the per-message timeout.
        let deadline = sent_at + Duration::from_millis(cfg.msg_timeout_ms.max(1));
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match tokio::time::timeout(deadline - now, ws.next()).await {
                Ok(Some(Ok(msg))) => {
                    let data = match msg {
                        Message::Binary(b) => b,
                        Message::Text(t) => bytes::Bytes::from(t.as_str().as_bytes().to_vec()),
                        Message::Close(_) => break,
                        // Ping/Pong/Frame are transport chatter, not echoes.
                        _ => continue,
                    };
                    if data.len() < 4 {
                        continue;
                    }
                    let echo_seq =
                        u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                    if let Some(Some(t0)) = send_times.get(echo_seq) {
                        if msg_rtts[echo_seq].is_none() {
                            msg_rtts[echo_seq] = Some(t0.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    if msg_rtts[seq].is_some() {
                        break;
                    }
                }
                Ok(Some(Err(e))) => {
                    debug!("websocket probe: recv error: {e}");
                    break;
                }
                Ok(None) => break, // stream closed
                Err(_) => break,   // timeout
            }
        }
    }

    // Polite close — best-effort, never part of the measurement.
    let _ = tokio::time::timeout(Duration::from_millis(250), ws.close(None)).await;

    Ok(WsSessionOutcome {
        upgrade_ms,
        upgrade_status,
        msg_rtts,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Error helper
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn ws_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
    detail: Option<String>,
    dns: Option<crate::metrics::DnsResult>,
    tcp: Option<crate::metrics::TcpResult>,
    tls: Option<crate::metrics::TlsResult>,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::WebSocket,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: false,
        dns,
        tcp,
        tls,
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
        rpm: None,
        ping: None,
        path: None,
        dualstack: None,
        websocket: None,
        pmtud: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn websocket_unresolvable_host_is_dns_error() {
        let target = url::Url::parse("http://this-hostname-does-not-exist.invalid:8080/").unwrap();
        let cfg = RunConfig {
            dns_enabled: true,
            timeout_ms: 3000,
            ..Default::default()
        };
        let attempt = run_websocket_probe(
            Uuid::new_v4(),
            0,
            &target,
            &cfg,
            &WebSocketProbeConfig::default(),
        )
        .await;
        assert!(!attempt.success);
        assert_eq!(attempt.protocol, Protocol::WebSocket);
        assert!(attempt.websocket.is_none());
        assert_eq!(
            attempt.error.expect("error must be set").category,
            ErrorCategory::Dns
        );
    }

    #[tokio::test]
    async fn websocket_connection_refused_is_tcp_error() {
        // Bind-then-drop so the port is free (nothing listens on it).
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let target = url::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let cfg = RunConfig {
            dns_enabled: false,
            timeout_ms: 3000,
            ..Default::default()
        };
        let attempt = run_websocket_probe(
            Uuid::new_v4(),
            0,
            &target,
            &cfg,
            &WebSocketProbeConfig::default(),
        )
        .await;
        assert!(!attempt.success);
        let err = attempt.error.expect("error must be set");
        assert!(
            matches!(err.category, ErrorCategory::Tcp | ErrorCategory::Timeout),
            "expected tcp/timeout error, got {:?}: {}",
            err.category,
            err.message
        );
    }
}
