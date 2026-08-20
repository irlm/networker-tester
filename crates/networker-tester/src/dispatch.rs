use chrono::Utc;
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

use crate::cli::ResolvedConfig;
use crate::metrics::{ErrorCategory, Protocol, RequestAttempt};
use crate::runner::{
    browser::run_browser_probe,
    curl::run_curl_probe,
    dns::run_dns_probe,
    dualstack::run_dualstack_probe,
    http::{run_probe, RunConfig},
    http3::run_http3_probe,
    mthroughput::{run_mthroughput_probe, MthroughputConfig},
    native::run_native_probe,
    pageload::{run_pageload2_probe, run_pageload3_probe, run_pageload_probe, PageLoadConfig},
    path::{run_path_probe, PathProbeConfig},
    ping::{run_ping_probe, PingProbeConfig},
    pmtud::{run_pmtud_probe, PmtudProbeConfig},
    responsiveness::{run_responsiveness_probe, ResponsivenessConfig},
    rpm::{run_rpm_probe, RpmProbeConfig},
    stamp::{run_stamp_probe, StampProbeConfig},
    throughput::{
        run_download1_probe, run_download2_probe, run_download3_probe, run_download_probe,
        run_upload1_probe, run_upload2_probe, run_upload3_probe, run_upload_probe,
        run_webdownload_probe, run_webupload_probe, ThroughputConfig,
    },
    tls::{run_tls_probe, run_tls_resumption_probe},
    udp::{run_udp_probe, UdpProbeConfig},
    udp_throughput::{run_udpdownload_probe, run_udpupload_probe, UdpThroughputConfig},
    websocket::{run_websocket_probe, WebSocketProbeConfig},
};

/// Rewrite a target URL to use a different port for an HTTP stack.
/// If `https` is true, keeps the https:// scheme; otherwise uses http://.
pub fn rewrite_url_for_stack(base: &url::Url, port: u16, https: bool) -> url::Url {
    let mut u = base.clone();
    let _ = u.set_scheme(if https { "https" } else { "http" });
    let _ = u.set_port(Some(port));
    u
}

pub fn apply_impairment_target(
    proto: &Protocol,
    target: &url::Url,
    cfg: &ResolvedConfig,
) -> url::Url {
    if cfg.impairment.delay_ms == 0 {
        return target.clone();
    }

    let supported = matches!(
        proto,
        Protocol::Http1
            | Protocol::Http2
            | Protocol::Http3
            | Protocol::Tcp
            | Protocol::Tls
            | Protocol::TlsResume
            | Protocol::Native
            | Protocol::Curl
    );

    if !supported {
        return target.clone();
    }

    let mut delayed = target.clone();
    delayed.set_path("/delay");
    delayed.set_query(Some(&format!("ms={}", cfg.impairment.delay_ms)));
    delayed
}

/// Per-attempt safety cap for the SHORT diagnostic modes — every one of them is
/// bounded by construction (a request timeout, or max_ttl × per-hop timeout), so
/// exceeding this means the probe stopped making progress rather than "the work
/// takes a while". Without it a wedged probe hangs the whole run until the
/// agent's overall budget expires: prod hit exactly that when `path` (runs=2,
/// ICMP fully blocked) returned an error on iteration 1 and then never returned
/// on iteration 2 — the run sat `running` with no attempts for 22 minutes
/// (prod mode sweep, v0.28.213).
///
/// Deliberately NOT applied to the long-by-design families (throughput,
/// page-load, browser, rpm/responsiveness/mthroughput): a 1 GB transfer or a
/// saturation ramp on a slow link legitimately outlives any fixed cap, and they
/// carry their own budgets.
fn attempt_cap(proto: &Protocol, cfg: &RunConfig) -> Option<Duration> {
    let short = matches!(
        proto,
        Protocol::Tcp
            | Protocol::Dns
            | Protocol::Tls
            | Protocol::TlsResume
            | Protocol::Http1
            | Protocol::Http2
            | Protocol::Http3
            | Protocol::Curl
            | Protocol::Native
            | Protocol::Ping
            | Protocol::Path
            | Protocol::Pmtud
            | Protocol::DualStack
            | Protocol::Udp
            | Protocol::Stamp
            | Protocol::WebSocket
            | Protocol::SdkProbe
    );
    if !short {
        return None;
    }
    // 10× the per-request timeout (a single attempt may legitimately do several
    // round-trips: DNS + connect + TLS + request, or 30 TTL probes), floored at
    // 120s so a tiny --timeout can never make the cap trip on a healthy probe.
    let ms = cfg.timeout_ms.saturating_mul(10).max(120_000);
    Some(Duration::from_millis(ms))
}

#[allow(clippy::too_many_arguments)]
pub async fn dispatch_once(
    proto: &Protocol,
    payload_sz: Option<usize>,
    run_id: Uuid,
    seq: u32,
    target: &url::Url,
    resolved_cfg: &ResolvedConfig,
    cfg: &RunConfig,
    udp_cfg: &UdpProbeConfig,
    udp_throughput_cfg: &UdpThroughputConfig,
    throughput_cfg: &ThroughputConfig,
    pageload_cfg: &PageLoadConfig,
) -> RequestAttempt {
    let mut attempt = match attempt_cap(proto, cfg) {
        Some(cap) => {
            let started_at = Utc::now();
            match tokio::time::timeout(
                cap,
                dispatch_once_inner(
                    proto,
                    payload_sz,
                    run_id,
                    seq,
                    target,
                    resolved_cfg,
                    cfg,
                    udp_cfg,
                    udp_throughput_cfg,
                    throughput_cfg,
                    pageload_cfg,
                ),
            )
            .await
            {
                Ok(attempt) => attempt,
                Err(_) => stalled_attempt(proto, run_id, seq, started_at, cap),
            }
        }
        None => {
            dispatch_once_inner(
                proto,
                payload_sz,
                run_id,
                seq,
                target,
                resolved_cfg,
                cfg,
                udp_cfg,
                udp_throughput_cfg,
                throughput_cfg,
                pageload_cfg,
            )
            .await
        }
    };
    // Stamp the probed URL on EVERY attempt (including stalled ones) at the one
    // choke point all probes flow through — multi-target runs (#782) attribute
    // each streamed attempt to its URL by this field.
    attempt.target_url = Some(target.to_string());
    attempt
}

/// The failed attempt recorded when a short diagnostic probe blows its cap —
/// an honest, actionable row instead of a run that silently stops progressing.
fn stalled_attempt(
    proto: &Protocol,
    run_id: Uuid,
    seq: u32,
    started_at: chrono::DateTime<Utc>,
    cap: Duration,
) -> RequestAttempt {
    let secs = cap.as_secs();
    crate::runner::pageload::error_attempt_proto(
        Uuid::new_v4(),
        run_id,
        seq,
        started_at,
        proto.clone(),
        ErrorCategory::Timeout,
        format!(
            "{proto} probe made no progress within {secs}s (per-attempt cap) — \
             the probe was abandoned so the rest of the run could continue"
        ),
    )
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_once_inner(
    proto: &Protocol,
    payload_sz: Option<usize>,
    run_id: Uuid,
    seq: u32,
    target: &url::Url,
    resolved_cfg: &ResolvedConfig,
    cfg: &RunConfig,
    udp_cfg: &UdpProbeConfig,
    udp_throughput_cfg: &UdpThroughputConfig,
    throughput_cfg: &ThroughputConfig,
    pageload_cfg: &PageLoadConfig,
) -> RequestAttempt {
    let impaired_target = apply_impairment_target(proto, target, resolved_cfg);
    match (proto, payload_sz) {
        (Protocol::Download, Some(sz)) => run_download_probe(run_id, seq, sz, throughput_cfg).await,
        (Protocol::Download1, Some(sz)) => {
            run_download1_probe(run_id, seq, sz, throughput_cfg).await
        }
        (Protocol::Download2, Some(sz)) => {
            run_download2_probe(run_id, seq, sz, throughput_cfg).await
        }
        (Protocol::Download3, Some(sz)) => {
            run_download3_probe(run_id, seq, sz, throughput_cfg).await
        }
        (Protocol::Upload, Some(sz)) => run_upload_probe(run_id, seq, sz, throughput_cfg).await,
        (Protocol::Upload1, Some(sz)) => run_upload1_probe(run_id, seq, sz, throughput_cfg).await,
        (Protocol::Upload2, Some(sz)) => run_upload2_probe(run_id, seq, sz, throughput_cfg).await,
        (Protocol::Upload3, Some(sz)) => run_upload3_probe(run_id, seq, sz, throughput_cfg).await,
        (Protocol::WebDownload, Some(sz)) => {
            run_webdownload_probe(run_id, seq, sz, throughput_cfg).await
        }
        (Protocol::WebUpload, Some(sz)) => {
            run_webupload_probe(run_id, seq, sz, throughput_cfg).await
        }
        (Protocol::UdpDownload, Some(sz)) => {
            run_udpdownload_probe(run_id, seq, sz, udp_throughput_cfg).await
        }
        (Protocol::UdpUpload, Some(sz)) => {
            run_udpupload_probe(run_id, seq, sz, udp_throughput_cfg).await
        }
        (Protocol::Http1, _)
        | (Protocol::Http2, _)
        | (Protocol::Tcp, _)
        | (Protocol::SdkProbe, _) => {
            run_probe(run_id, seq, proto.clone(), &impaired_target, cfg).await
        }
        (Protocol::Http3, _) => {
            run_http3_probe(
                run_id,
                seq,
                &impaired_target,
                cfg.timeout_ms,
                cfg.insecure,
                cfg.ca_bundle.as_deref(),
            )
            .await
        }
        (Protocol::Udp, _) => run_udp_probe(run_id, seq, udp_cfg).await,
        (Protocol::Rpm, _) => {
            // Latency-under-load: reuses the resolved UDP echo settings for
            // both phases and the throughput config for the load generator.
            let rpm_cfg = RpmProbeConfig::from_parts(udp_cfg.clone(), throughput_cfg.clone());
            run_rpm_probe(run_id, seq, &rpm_cfg).await
        }
        (Protocol::Responsiveness, _) => {
            // Draft-conformant responsiveness: base URL + TLS trust settings
            // come from the throughput config; ramp/stability parameters are
            // the draft-08 defaults.
            let resp_cfg = ResponsivenessConfig::from_parts(throughput_cfg);
            run_responsiveness_probe(run_id, seq, &resp_cfg).await
        }
        (Protocol::Mthroughput, _) => {
            // Multi-connection capacity: base URL + TLS trust settings come
            // from the throughput config; ramp/measure parameters are the
            // mode's time-boxed defaults (no payload size — stages are
            // time-boxed, see runner docs).
            let m_cfg = MthroughputConfig::from_parts(throughput_cfg);
            run_mthroughput_probe(run_id, seq, &m_cfg).await
        }
        (Protocol::Stamp, _) => {
            // STAMP Session-Sender: reflector host follows the UDP echo
            // target; the port is the endpoint's Session-Reflector
            // (`--stamp-port`, default 9997); Tmax follows --udp-timeout.
            let stamp_cfg = StampProbeConfig {
                target_host: udp_cfg.target_host.clone(),
                target_port: resolved_cfg.stamp_port,
                timeout_ms: udp_cfg.timeout_ms,
                ..StampProbeConfig::default()
            };
            run_stamp_probe(run_id, seq, &stamp_cfg).await
        }
        (Protocol::Ping, _) => {
            // ICMP echo to the target host; probe count/timeout reuse the
            // resolved UDP echo settings (--udp-probes / --udp-timeout).
            let ping_cfg = PingProbeConfig {
                target_host: target.host_str().unwrap_or("").to_string(),
                probe_count: udp_cfg.probe_count,
                timeout_ms: udp_cfg.timeout_ms,
                payload_size: udp_cfg.payload_size,
                // The labeled tcp-rtt fallback (all echoes lost → cloud SNAT
                // ate the ICMP) connects to the probe URL's own port.
                fallback_tcp_port: Some(target.port_or_known_default().unwrap_or(443)),
            };
            run_ping_probe(run_id, seq, &ping_cfg).await
        }
        (Protocol::Path, _) => {
            let path_cfg = PathProbeConfig {
                target_host: target.host_str().unwrap_or("").to_string(),
                // Zero-info traces get classified against the probe URL's own
                // port: reachable-over-TCP ⇒ the environment ate the ICMP.
                verify_tcp_port: Some(target.port_or_known_default().unwrap_or(443)),
                // Honour the run's timeout: a filtered path used to cost
                // max_ttl x per-hop (30 x 1 s) no matter what the caller asked
                // for, which read as a hang from the second iteration on.
                total_budget_ms: cfg.timeout_ms,
                ..PathProbeConfig::default()
            };
            run_path_probe(run_id, seq, &path_cfg).await
        }
        (Protocol::DualStack, _) => run_dualstack_probe(run_id, seq, target, cfg).await,
        (Protocol::WebSocket, _) => {
            // Message count/size/timeout reuse the resolved UDP echo settings
            // (--udp-probes / --udp-payload / --udp-timeout); the connection
            // ladder (timeout, TLS trust, family pins) comes from RunConfig.
            let ws_cfg = WebSocketProbeConfig {
                message_count: udp_cfg.probe_count,
                payload_size: udp_cfg.payload_size,
                msg_timeout_ms: udp_cfg.timeout_ms,
            };
            run_websocket_probe(run_id, seq, target, cfg, &ws_cfg).await
        }
        (Protocol::Pmtud, _) => {
            // DF probes aim at the resolved UDP echo host/port so a
            // networker-endpoint target positively confirms delivery; the
            // probe still concludes from ICMP alone against anything else.
            let pmtud_cfg = PmtudProbeConfig {
                target_host: udp_cfg.target_host.clone(),
                target_port: udp_cfg.target_port,
                ..PmtudProbeConfig::default()
            };
            run_pmtud_probe(run_id, seq, &pmtud_cfg).await
        }
        (Protocol::Dns, _) => {
            let host = target.host_str().unwrap_or("");
            run_dns_probe(run_id, seq, host, cfg.ipv4_only, cfg.ipv6_only).await
        }
        (Protocol::Tls, _) => run_tls_probe(run_id, seq, &impaired_target, cfg).await,
        (Protocol::TlsResume, _) => {
            run_tls_resumption_probe(run_id, seq, &impaired_target, cfg).await
        }
        (Protocol::Native, _) => run_native_probe(run_id, seq, &impaired_target, cfg).await,
        (Protocol::Curl, _) => run_curl_probe(run_id, seq, &impaired_target, cfg).await,
        (Protocol::PageLoad, _) => run_pageload_probe(run_id, seq, pageload_cfg).await,
        (Protocol::PageLoad2, _) => run_pageload2_probe(run_id, seq, pageload_cfg).await,
        (Protocol::PageLoad3, _) => run_pageload3_probe(run_id, seq, pageload_cfg).await,
        (Protocol::Browser | Protocol::Browser1 | Protocol::Browser2 | Protocol::Browser3, _) => {
            run_browser_probe(
                run_id,
                seq,
                proto.clone(),
                target,
                &pageload_cfg.asset_sizes,
                cfg.timeout_ms,
                cfg.insecure,
            )
            .await
        }
        _ => unreachable!("Upload/WebUpload/UdpDownload/UdpUpload without payload_size"),
    }
}

/// True when the embedding process (the C# agent) asked for per-attempt NDJSON
/// events via `NETWORKER_ATTEMPT_STREAM=1`. Env-gated so the versioned
/// `--json-stdout` single-artifact contract is untouched by default and OLD
/// tester binaries degrade gracefully (they simply ignore the env var).
fn attempt_stream_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NETWORKER_ATTEMPT_STREAM").is_ok_and(|v| v == "1"))
}

/// One NDJSON attempt-event line: `{"event":"attempt","attempt":{…}}`.
/// Emitted on stdout AHEAD of the final `--json-stdout` artifact so the agent
/// can stream attempts live instead of waiting for process exit. The final
/// artifact line has no `"event"` key, so readers separate them by shape.
pub fn format_attempt_event(a: &RequestAttempt) -> Option<String> {
    serde_json::to_string(a)
        .ok()
        .map(|json| format!("{{\"event\":\"attempt\",\"attempt\":{json}}}"))
}

fn emit_attempt_event(a: &RequestAttempt) {
    if !attempt_stream_enabled() {
        return;
    }
    if let Some(line) = format_attempt_event(a) {
        // Rust's stdout is a LineWriter — the newline flushes the event
        // immediately, which is the whole point of streaming.
        println!("{line}");
    }
}

pub fn log_attempt(a: &RequestAttempt) {
    use crate::metrics::Protocol::*;
    emit_attempt_event(a);
    let status = if a.success { "✓" } else { "✗" };
    // One suffix, two orthogonal facts (#782 P2): which burst sample this is
    // (0-based `sample_index`, shown 1-based) and how many retries it took.
    // A non-burst attempt keeps the historical "" / " (retry #N)" wording.
    let retry_suffix = match (a.sample_index, a.retry_count) {
        (0, 0) => String::new(),
        (0, r) => format!(" (retry #{r})"),
        (s, 0) => format!(" (sample #{})", s + 1),
        (s, r) => format!(" (sample #{}, retry #{r})", s + 1),
    };

    match &a.protocol {
        SdkProbe => {
            let dns = a
                .dns
                .as_ref()
                .map(|d| format!(" DNS:{:.1}ms", d.duration_ms))
                .unwrap_or_default();
            let tcp = a
                .tcp
                .as_ref()
                .map(|t| format!(" TCP:{:.1}ms", t.connect_duration_ms))
                .unwrap_or_default();
            let tls_ms = a
                .tls
                .as_ref()
                .map(|t| t.handshake_duration_ms)
                .unwrap_or(0.0);
            let tls_part = if tls_ms > 0.0 {
                format!(" TLS:{tls_ms:.1}ms")
            } else {
                String::new()
            };
            let ttfb_ms = a.http.as_ref().map(|h| h.ttfb_ms).unwrap_or(0.0);
            let total_ms = a.http.as_ref().map(|h| h.total_duration_ms).unwrap_or(0.0);
            let status_code = a
                .http
                .as_ref()
                .map(|h| h.status_code.to_string())
                .unwrap_or_default();
            // The distinguishing output: NETWORK vs SERVER split. "—" when the
            // response carried no app/total server-timing (split unavailable).
            let split = match a.server_timing.as_ref() {
                Some(st) => match (st.server_ms, st.network_ms) {
                    (Some(srv), Some(net)) => {
                        let flag = if st.split_anomaly { " (clamped)" } else { "" };
                        format!(" Server:{srv:.1}ms Network:{net:.1}ms{flag}")
                    }
                    _ => " Server:— Network:—".into(),
                },
                None => " Server:— Network:—".into(),
            };
            info!(
                "{status} #{seq} [sdkprobe] {status_code}{dns}{tcp}{tls_part} \
                 TTFB:{ttfb:.1}ms Total:{total:.1}ms{split}{retry}",
                seq = a.sequence_num,
                ttfb = ttfb_ms,
                total = total_ms,
                retry = retry_suffix,
            );
        }
        Http1 | Http2 | Http3 | Tcp | Native | Curl => {
            // Only show DNS/TCP segments when the probe actually measured them.
            // HTTP/3 sets both to None (QUIC has no separate DNS/TCP phase —
            // the QUIC handshake is captured in TLS:Xms instead).
            let dns = a
                .dns
                .as_ref()
                .map(|d| format!(" DNS:{:.1}ms", d.duration_ms))
                .unwrap_or_default();
            let tcp = a
                .tcp
                .as_ref()
                .map(|t| format!(" TCP:{:.1}ms", t.connect_duration_ms))
                .unwrap_or_default();
            let tls_ms = a
                .tls
                .as_ref()
                .map(|t| t.handshake_duration_ms)
                .unwrap_or(0.0);
            let ttfb_ms = a.http.as_ref().map(|h| h.ttfb_ms).unwrap_or(0.0);
            let total_ms = a.http.as_ref().map(|h| h.total_duration_ms).unwrap_or(0.0);
            let ver = a
                .http
                .as_ref()
                .map(|h| h.negotiated_version.clone())
                .unwrap_or_default();
            let status_code = a
                .http
                .as_ref()
                .map(|h| h.status_code.to_string())
                .unwrap_or_default();
            let cpu = a
                .http
                .as_ref()
                .and_then(|h| h.cpu_time_ms)
                .map(|c| format!(" CPU:{c:.1}ms"))
                .unwrap_or_default();
            let csw = match (
                a.http.as_ref().and_then(|h| h.csw_voluntary),
                a.http.as_ref().and_then(|h| h.csw_involuntary),
            ) {
                (Some(v), Some(i)) => format!(" CSW:{v}v/{i}i"),
                _ => String::new(),
            };
            // For HTTP/3, TLS: is the QUIC handshake; label it accordingly.
            let tls_label = if matches!(a.protocol, Http3) {
                "QUIC"
            } else {
                "TLS"
            };

            info!(
                "{status} #{seq} [{proto}] {status_code} {ver}{dns}{tcp} \
                 {tls_label}:{tls:.1}ms TTFB:{ttfb:.1}ms Total:{total:.1}ms{cpu}{csw}{retry}",
                seq = a.sequence_num,
                proto = a.protocol,
                tls = tls_ms,
                ttfb = ttfb_ms,
                total = total_ms,
                retry = retry_suffix,
            );
        }
        Download | Download1 | Download2 | Download3 | Upload | Upload1 | Upload2 | Upload3
        | WebDownload | WebUpload => {
            if let Some(h) = &a.http {
                let n = h.payload_bytes;
                let payload_str = if n >= 1 << 20 {
                    format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64)
                } else if n >= 1 << 10 {
                    format!("{:.1} KiB", n as f64 / (1u64 << 10) as f64)
                } else {
                    format!("{n} B")
                };
                let tls_ms = a
                    .tls
                    .as_ref()
                    .map(|t| t.handshake_duration_ms)
                    .unwrap_or(0.0);
                let ttfb_ms = h.ttfb_ms;
                let tls_part = if tls_ms > 0.0 {
                    format!(" TLS:{tls_ms:.1}ms")
                } else {
                    String::new()
                };
                let throughput = h
                    .throughput_mbps
                    .map(|m| format!("{m:.2} MB/s"))
                    .unwrap_or_else(|| "—".into());
                let goodput = h
                    .goodput_mbps
                    .map(|g| format!(" Goodput:{g:.2} MB/s"))
                    .unwrap_or_default();
                let cpu = h
                    .cpu_time_ms
                    .map(|c| format!(" CPU:{c:.1}ms"))
                    .unwrap_or_default();
                let csw = match (h.csw_voluntary, h.csw_involuntary) {
                    (Some(v), Some(i)) => format!(" CSW:{v}v/{i}i"),
                    _ => String::new(),
                };
                let srv_csw = match a.server_timing.as_ref() {
                    Some(st) => match (st.srv_csw_voluntary, st.srv_csw_involuntary) {
                        (Some(v), Some(i)) => format!(" sCSW:{v}v/{i}i"),
                        _ => String::new(),
                    },
                    None => String::new(),
                };
                info!(
                    "{status} #{seq} [{proto}] {payload}{tls} TTFB:{ttfb:.1}ms Total:{total:.1}ms Throughput:{throughput}{goodput}{cpu}{csw}{srv_csw}{retry}",
                    seq = a.sequence_num,
                    proto = a.protocol,
                    payload = payload_str,
                    tls = tls_part,
                    ttfb = ttfb_ms,
                    total = h.total_duration_ms,
                    retry = retry_suffix,
                );
            }
        }
        Udp => {
            if let Some(u) = &a.udp {
                info!(
                    "{status} #{seq} [udp] RTT avg={avg:.1}ms p95={p95:.1}ms loss={loss:.1}%{retry}",
                    seq = a.sequence_num,
                    avg = u.rtt_avg_ms,
                    p95 = u.rtt_p95_ms,
                    loss = u.loss_percent,
                    retry = retry_suffix,
                );
            }
        }
        Rpm => {
            if let Some(r) = &a.rpm {
                let rpm_str = r
                    .rpm
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "—".into());
                let factor_str = r
                    .bufferbloat_factor
                    .map(|f| format!("{f:.2}x"))
                    .unwrap_or_else(|| "—".into());
                info!(
                    "{status} #{seq} [rpm] RPM={rpm} factor={factor} \
                     idle avg={idle_avg:.1}ms p95={idle_p95:.1}ms | \
                     loaded avg={load_avg:.1}ms p95={load_p95:.1}ms \
                     jitter={jitter:.1}ms loss={loss:.1}%{retry}",
                    seq = a.sequence_num,
                    rpm = rpm_str,
                    factor = factor_str,
                    idle_avg = r.unloaded_rtt_avg_ms,
                    idle_p95 = r.unloaded_rtt_p95_ms,
                    load_avg = r.loaded_rtt_avg_ms,
                    load_p95 = r.loaded_rtt_p95_ms,
                    jitter = r.loaded_jitter_ms,
                    loss = r.loaded_loss_percent,
                    retry = retry_suffix,
                );
            }
        }
        Responsiveness => {
            if let Some(r) = &a.responsiveness {
                let fmt_rpm =
                    |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into());
                let fmt_cap = |v: Option<f64>| {
                    v.map(|x| format!("{x:.1} MB/s"))
                        .unwrap_or_else(|| "—".into())
                };
                let d = &r.download;
                let up = r
                    .upload
                    .as_ref()
                    .map(|u| {
                        format!(
                            "up RPM={rpm} cap={cap} conns={c} sat={s}",
                            rpm = fmt_rpm(u.rpm),
                            cap = fmt_cap(u.capacity_mbps),
                            c = u.saturated_connections,
                            s = u.saturation_reached,
                        )
                    })
                    .unwrap_or_else(|| "up —".into());
                info!(
                    "{status} #{seq} [responsiveness] down RPM={rpm} cap={cap} \
                     conns={conns} sat={sat} | {up}{retry}",
                    seq = a.sequence_num,
                    rpm = fmt_rpm(d.rpm),
                    cap = fmt_cap(d.capacity_mbps),
                    conns = d.saturated_connections,
                    sat = d.saturation_reached,
                    retry = retry_suffix,
                );
            }
        }
        Mthroughput => {
            if let Some(m) = &a.mthroughput {
                let fmt_cap = |v: Option<f64>| {
                    v.map(|x| format!("{x:.1} MB/s"))
                        .unwrap_or_else(|| "\u{2014}".into())
                };
                let fmt_spread = |v: Option<f64>| {
                    v.map(|x| format!("{x:.0}%"))
                        .unwrap_or_else(|| "\u{2014}".into())
                };
                let d = &m.download;
                let up = m
                    .upload
                    .as_ref()
                    .map(|u| {
                        format!(
                            "up cap={cap} conns={c} spread={sp} sat={s}",
                            cap = fmt_cap(u.capacity_mbps),
                            c = u.connections,
                            sp = fmt_spread(u.fair_share_spread_pct),
                            s = u.saturation_reached,
                        )
                    })
                    .unwrap_or_else(|| "up \u{2014}".into());
                info!(
                    "{status} #{seq} [mthroughput] down cap={cap} conns={conns} \
                     spread={spread} sat={sat} | {up}{retry}",
                    seq = a.sequence_num,
                    cap = fmt_cap(d.capacity_mbps),
                    conns = d.connections,
                    spread = fmt_spread(d.fair_share_spread_pct),
                    sat = d.saturation_reached,
                    retry = retry_suffix,
                );
            }
        }
        Stamp => {
            if let Some(s) = &a.stamp {
                let fmt_pct =
                    |v: Option<f64>| v.map(|x| format!("{x:.1}%")).unwrap_or_else(|| "—".into());
                info!(
                    "{status} #{seq} [stamp] {addr} RTT avg={avg:.2}ms p95={p95:.2}ms \
                     (processing-corrected) loss fwd={fwd}/rev={rev} \
                     replies={replies}/{sent}{retry}",
                    seq = a.sequence_num,
                    addr = s.remote_addr,
                    avg = s.rtt_avg_ms,
                    p95 = s.rtt_p95_ms,
                    fwd = fmt_pct(s.loss_sent_percent),
                    rev = fmt_pct(s.loss_return_percent),
                    replies = s.replies_received,
                    sent = s.probes_sent,
                    retry = retry_suffix,
                );
            }
        }
        Ping => {
            if let Some(p) = &a.ping {
                let ttl = p.reply_ttl.map(|t| format!(" ttl={t}")).unwrap_or_default();
                // A tcp-rtt fallback RTT must never read as an ICMP RTT.
                let via = p
                    .fallback_method
                    .as_deref()
                    .map(|m| {
                        let port = p.fallback_port.map(|p| format!(":{p}")).unwrap_or_default();
                        format!(" via={m}{port} (ICMP blocked)")
                    })
                    .unwrap_or_default();
                info!(
                    "{status} #{seq} [ping] {addr} RTT avg={avg:.1}ms p95={p95:.1}ms \
                     jitter={jitter:.1}ms loss={loss:.1}%{ttl}{via}{retry}",
                    seq = a.sequence_num,
                    addr = p.remote_addr,
                    avg = p.rtt_avg_ms,
                    p95 = p.rtt_p95_ms,
                    jitter = p.jitter_ms,
                    loss = p.loss_percent,
                    retry = retry_suffix,
                );
            }
        }
        Path => {
            if let Some(p) = &a.path {
                let hops_str = p
                    .hop_count
                    .map(|h| h.to_string())
                    .unwrap_or_else(|| "?".into());
                let dest_rtt = p
                    .destination_rtt_ms
                    .map(|r| format!(" dest_rtt={r:.1}ms"))
                    .unwrap_or_default();
                info!(
                    "{status} #{seq} [path] {addr} hops={hops} reached={reached} \
                     responding={responding}/{probed} method={method}{dest_rtt}{retry}",
                    seq = a.sequence_num,
                    addr = p.remote_addr,
                    hops = hops_str,
                    reached = p.destination_reached,
                    responding = p.hops.iter().filter(|h| h.addr.is_some()).count(),
                    probed = p.hops.len(),
                    method = p.method,
                    retry = retry_suffix,
                );
            }
        }
        DualStack => {
            if let Some(d) = &a.dualstack {
                let leg = |l: &crate::metrics::DualStackLeg| -> String {
                    if !l.attempted {
                        "absent".to_string()
                    } else if let Some(total) = l.total_ms {
                        format!("{total:.1}ms")
                    } else {
                        "failed".to_string()
                    }
                };
                let delta = d
                    .delta_ms
                    .map(|v| format!(" Δ={v:.1}ms"))
                    .unwrap_or_default();
                info!(
                    "{status} #{seq} [dualstack] v4={v4} v6={v6}{delta} HE→{verdict}{retry}",
                    seq = a.sequence_num,
                    v4 = leg(&d.ipv4),
                    v6 = leg(&d.ipv6),
                    verdict = d.happy_eyeballs_verdict,
                    retry = retry_suffix,
                );
            }
        }
        WebSocket => {
            if let Some(w) = &a.websocket {
                let tls_part = a
                    .tls
                    .as_ref()
                    .map(|t| format!(" TLS:{:.1}ms", t.handshake_duration_ms))
                    .unwrap_or_default();
                info!(
                    "{status} #{seq} [websocket] {url}{tls} upgrade={upgrade:.1}ms \
                     msg RTT avg={avg:.1}ms p95={p95:.1}ms jitter={jitter:.1}ms \
                     echoes={echoes}/{sent} loss={loss:.1}%{retry}",
                    seq = a.sequence_num,
                    url = w.url,
                    tls = tls_part,
                    upgrade = w.upgrade_ms,
                    avg = w.msg_rtt_avg_ms,
                    p95 = w.msg_rtt_p95_ms,
                    jitter = w.jitter_ms,
                    echoes = w.echo_count,
                    sent = w.message_count,
                    loss = w.loss_percent,
                    retry = retry_suffix,
                );
            }
        }
        Pmtud => {
            if let Some(p) = &a.pmtud {
                let mtu_str = match (p.path_mtu, p.lower_bound_only) {
                    (Some(m), true) => format!("≥{m}"),
                    (Some(m), false) => m.to_string(),
                    (None, _) => "unknown".into(),
                };
                let icmp = p
                    .icmp_mtu
                    .map(|m| format!(" icmp_mtu={m}"))
                    .unwrap_or_default();
                let local = p
                    .local_mtu
                    .map(|m| format!(" local_mtu={m}"))
                    .unwrap_or_default();
                info!(
                    "{status} #{seq} [pmtud] {addr} path_mtu={mtu}{icmp}{local} \
                     probes={probes} method={method}{retry}",
                    seq = a.sequence_num,
                    addr = p.remote_addr,
                    mtu = mtu_str,
                    probes = p.probes_sent,
                    method = p.method,
                    retry = retry_suffix,
                );
            }
        }
        UdpDownload | UdpUpload => {
            if let Some(ut) = &a.udp_throughput {
                let n = ut.payload_bytes;
                let payload_str = if n >= 1 << 20 {
                    format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64)
                } else if n >= 1 << 10 {
                    format!("{:.1} KiB", n as f64 / (1u64 << 10) as f64)
                } else {
                    format!("{n} B")
                };
                let throughput = ut
                    .throughput_mbps
                    .map(|m| format!("{m:.2} MB/s"))
                    .unwrap_or_else(|| "—".into());
                info!(
                    "{status} #{seq} [{proto}] {payload} \
                     sent={sent} recv={recv} loss={loss:.1}% \
                     xfer={xfer:.1}ms Throughput:{throughput}{retry}",
                    seq = a.sequence_num,
                    proto = a.protocol,
                    payload = payload_str,
                    sent = ut.datagrams_sent,
                    // Upload probes cannot know the received datagram count
                    // (server reports bytes) — shown as "n/a", never faked.
                    recv = ut
                        .datagrams_received
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "n/a".into()),
                    loss = ut.loss_percent,
                    xfer = ut.transfer_ms,
                    retry = retry_suffix,
                );
            }
        }
        Dns => {
            if let Some(d) = &a.dns {
                info!(
                    "{status} #{seq} [dns] {name} → {ips} in {dur:.1}ms{retry}",
                    seq = a.sequence_num,
                    name = d.query_name,
                    ips = d.resolved_ips.join(", "),
                    dur = d.duration_ms,
                    retry = retry_suffix,
                );
            }
        }
        Tls => {
            if let Some(t) = &a.tls {
                let ver = &t.protocol_version;
                let alpn = t.alpn_negotiated.as_deref().unwrap_or("—");
                info!(
                    "{status} #{seq} [tls] {ver} ALPN={alpn} \
                     TCP:{tcp:.1}ms Handshake:{hs:.1}ms{retry}",
                    seq = a.sequence_num,
                    tcp = a.tcp.as_ref().map(|t| t.connect_duration_ms).unwrap_or(0.0),
                    hs = t.handshake_duration_ms,
                    retry = retry_suffix,
                );
            }
        }
        TlsResume => {
            if let Some(t) = &a.tls {
                info!(
                    "{status} #{seq} [tlsresume] cold={cold_kind}:{cold_hs:.1}ms warm={warm_kind}:{warm_hs:.1}ms resumed={resumed} cold_http={cold_http:?} warm_http={warm_http:?}{retry}",
                    seq = a.sequence_num,
                    cold_kind = t.previous_handshake_kind.as_deref().unwrap_or("unknown"),
                    cold_hs = t.previous_handshake_duration_ms.unwrap_or(0.0),
                    warm_kind = t.handshake_kind.as_deref().unwrap_or("unknown"),
                    warm_hs = t.handshake_duration_ms,
                    resumed = t.resumed.unwrap_or(false),
                    cold_http = t.previous_http_status_code,
                    warm_http = t.http_status_code,
                    retry = retry_suffix,
                );
            }
        }
        Browser | Browser1 | Browser2 | Browser3 => {
            if let Some(b) = &a.browser {
                let protos = b
                    .resource_protocols
                    .iter()
                    .map(|(p, n)| format!("{p}×{n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                // TTFB here is the *browser* definition (navigationStart →
                // responseStart, includes DNS/connect/TLS). Bytes: prefer
                // wire_bytes (Σ CDP encodedDataLength — real wire bytes incl.
                // headers); fall back to cl_bytes (Σ declared Content-Length
                // headers, NOT wire bytes) on pre-Wave-W data.
                let bytes_str = match b.wire_bytes_total {
                    Some(w) => format!("wire_bytes={w}"),
                    None => format!("cl_bytes={}", b.transferred_bytes),
                };
                // Core Web Vitals — only what was actually observed.
                let mut cwv = String::new();
                if let Some(v) = b.fcp_ms {
                    cwv.push_str(&format!(" FCP:{v:.1}ms"));
                }
                if let Some(v) = b.lcp_ms {
                    cwv.push_str(&format!(" LCP:{v:.1}ms"));
                }
                if let Some(v) = b.cls {
                    cwv.push_str(&format!(" CLS:{v:.3}"));
                }
                if let Some(v) = b.tbt_ms {
                    cwv.push_str(&format!(" TBT:{v:.1}ms"));
                }
                info!(
                    "{status} #{seq} [{mode}] proto={proto} TTFB(nav):{ttfb:.1}ms \
                     DCL:{dcl:.1}ms Load:{load:.1}ms{cwv} res={res} {bytes_str} [{protos}]{retry}",
                    mode = a.protocol,
                    seq = a.sequence_num,
                    proto = b.protocol,
                    ttfb = b.ttfb_ms,
                    dcl = b.dom_content_loaded_ms,
                    load = b.load_ms,
                    res = b.resource_count,
                    retry = retry_suffix,
                );
            }
        }
        PageLoad | PageLoad2 | PageLoad3 => {
            if let Some(p) = &a.page_load {
                let tls_info = if p.tls_setup_ms > 0.0 {
                    format!(
                        " tls={:.1}ms({:.1}%)",
                        p.tls_setup_ms,
                        p.tls_overhead_ratio * 100.0
                    )
                } else {
                    String::new()
                };
                let cpu_info = p
                    .cpu_time_ms
                    .map(|ms| format!(" cpu={ms:.1}ms"))
                    .unwrap_or_default();
                info!(
                    "{status} #{seq} [{proto}] {fetched}/{total} assets \
                     conns={conns}{tls}{cpu} {ms:.1}ms{retry}",
                    seq = a.sequence_num,
                    proto = a.protocol,
                    fetched = p.assets_fetched,
                    total = p.asset_count,
                    conns = p.connections_opened,
                    tls = tls_info,
                    cpu = cpu_info,
                    ms = p.total_ms,
                    retry = retry_suffix,
                );
            }
        }
    }

    if let Some(e) = &a.error {
        warn!("  Error [{cat}] {msg}", cat = e.category, msg = e.message);
    }
}

/// Reduce one logical attempt's RAW attempt vector to the attempts that get
/// published (persisted / streamed / summarised).
///
/// Two kinds of repeat live in that vector and they are NOT the same thing
/// (issue #782 P2):
///
/// * a **retry** (`--retries`, `retry_count`) REPLACES a failed try of the
///   same sample — only its final outcome is published, so a mode that
///   succeeded on the second try counts once, as a success. That is what this
///   function has always done and what keeps "success" meaning the same thing
///   for a logical attempt.
/// * a **sample** (`--samples`, `sample_index`) is an intentional repeat of
///   the measurement — every sample is published, so a point has a real
///   median and spread instead of one noisy value.
///
/// So: collapse within a `sample_index`, keep across `sample_index`. Input is
/// the burst in execution order (sample 0's tries, then sample 1's, …); the
/// output preserves that order, one attempt per sample that ran. A failed
/// sample stays in the output as a failed sample — it is never dropped, and
/// nothing is ever synthesised for a sample that did not run.
pub fn published_logical_attempts(attempts: Vec<RequestAttempt>) -> Vec<RequestAttempt> {
    let mut published: Vec<RequestAttempt> = Vec::new();
    for attempt in attempts {
        match published.last_mut() {
            // Same sample as the previous attempt → this is its retry, and the
            // retry replaces what it retried.
            Some(prev) if prev.sample_index == attempt.sample_index => *prev = attempt,
            _ => published.push(attempt),
        }
    }
    published
}

#[cfg(test)]
mod attempt_stream_tests {
    use super::format_attempt_event;
    use crate::output::db::test_fixtures::bare_attempt;
    use uuid::Uuid;

    /// The NDJSON event line must be (a) one line, (b) shaped
    /// {"event":"attempt","attempt":{…}} so the agent can split events from
    /// the final --json-stdout artifact (which has no "event" key), and (c)
    /// round-trippable into the same attempt JSON the artifact carries.
    #[test]
    fn event_line_shape_round_trips() {
        let a = bare_attempt(Uuid::nil());
        let line = format_attempt_event(&a).expect("serializes");
        assert!(!line.contains('\n'), "must be a single NDJSON line");

        let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(v["event"], "attempt");
        assert_eq!(v["attempt"]["attempt_id"], a.attempt_id.to_string());
        assert_eq!(v["attempt"]["success"], a.success);
    }

    /// Multi-target ("URL set", #782) attribution: the streamed event carries
    /// target_url when stamped, and OMITS the key entirely when None — so
    /// pre-#782 artifact/stream consumers see byte-identical attempt JSON.
    #[test]
    fn event_line_carries_target_url_and_omits_none() {
        let mut a = bare_attempt(Uuid::nil());
        assert!(
            !format_attempt_event(&a)
                .expect("serializes")
                .contains("target_url"),
            "None must serialize to an ABSENT key, not null"
        );

        a.target_url = Some("https://compare.example/health".into());
        let v: serde_json::Value =
            serde_json::from_str(&format_attempt_event(&a).expect("serializes")).expect("valid");
        assert_eq!(v["attempt"]["target_url"], "https://compare.example/health");
    }

    /// Burst sampling (#782 P2): `sample_index` rides the SAME stream as
    /// `retry_count` — the control plane needs both to tell "the 3rd of 5
    /// samples" from "the 3rd try of one sample". Serialized always (a plain
    /// u32 like retry_count, `#[serde(default)]` for pre-#782 artifacts).
    #[test]
    fn event_line_carries_sample_index() {
        let mut a = bare_attempt(Uuid::nil());
        let v: serde_json::Value =
            serde_json::from_str(&format_attempt_event(&a).expect("serializes")).expect("valid");
        assert_eq!(v["attempt"]["sample_index"], 0, "default sample is 0");

        a.sample_index = 4;
        a.retry_count = 1;
        let v: serde_json::Value =
            serde_json::from_str(&format_attempt_event(&a).expect("serializes")).expect("valid");
        assert_eq!(v["attempt"]["sample_index"], 4);
        assert_eq!(v["attempt"]["retry_count"], 1);
    }

    /// Older artifacts have no `sample_index` key at all; deserializing must
    /// default it to 0 rather than failing (every pre-burst attempt IS the
    /// only sample of its logical attempt).
    #[test]
    fn attempt_json_without_sample_index_defaults_to_zero() {
        let a = bare_attempt(Uuid::nil());
        let mut v = serde_json::to_value(&a).expect("serializes");
        v.as_object_mut()
            .expect("attempt is an object")
            .remove("sample_index");
        let back: crate::metrics::RequestAttempt =
            serde_json::from_value(v).expect("pre-#782 attempt JSON still parses");
        assert_eq!(back.sample_index, 0);
    }
}

#[cfg(test)]
mod attempt_cap_tests {
    use super::{attempt_cap, stalled_attempt};
    use crate::metrics::{ErrorCategory, Protocol};
    use crate::runner::http::RunConfig;
    use chrono::Utc;
    use std::time::Duration;
    use uuid::Uuid;

    // ── per-attempt cap (prod sweep v0.28.213) ────────────────────────────────

    #[test]
    fn attempt_cap_applies_to_short_diagnostics_only() {
        let cfg = RunConfig {
            timeout_ms: 20_000,
            ..RunConfig::default()
        };
        for p in [
            Protocol::Tcp,
            Protocol::Dns,
            Protocol::Path,
            Protocol::Pmtud,
            Protocol::Ping,
            Protocol::Http3,
            Protocol::Stamp,
            Protocol::WebSocket,
        ] {
            assert_eq!(
                attempt_cap(&p, &cfg),
                Some(Duration::from_millis(200_000)),
                "{p:?} must be capped at 10x the request timeout"
            );
        }
        // Long-by-design families keep running: a big transfer or a saturation
        // ramp must never be cut short by a fixed cap.
        for p in [
            Protocol::Download,
            Protocol::Download3,
            Protocol::Upload2,
            Protocol::PageLoad,
            Protocol::PageLoad3,
            Protocol::Browser1,
            Protocol::Rpm,
            Protocol::Responsiveness,
            Protocol::Mthroughput,
            Protocol::UdpDownload,
        ] {
            assert_eq!(attempt_cap(&p, &cfg), None, "{p:?} must stay uncapped");
        }
    }

    #[test]
    fn attempt_cap_has_a_two_minute_floor() {
        let cfg = RunConfig {
            timeout_ms: 1_000,
            ..RunConfig::default()
        };
        assert_eq!(
            attempt_cap(&Protocol::Path, &cfg),
            Some(Duration::from_secs(120))
        );
    }

    #[test]
    fn stalled_attempt_is_an_explained_failure() {
        // The prod symptom: `path` never returned and the run stopped making
        // progress. With the cap the run records a failed, explained attempt.
        let a = stalled_attempt(
            &Protocol::Path,
            Uuid::nil(),
            7,
            Utc::now(),
            Duration::from_secs(120),
        );
        assert!(!a.success);
        assert_eq!(a.protocol, Protocol::Path);
        assert_eq!(a.sequence_num, 7);
        let err = a.error.expect("stalled attempt carries an error");
        assert_eq!(err.category, ErrorCategory::Timeout);
        assert!(
            err.message.contains("no progress within 120s"),
            "{}",
            err.message
        );
    }
}
