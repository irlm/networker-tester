use tracing::{info, warn};
use uuid::Uuid;

use crate::cli::ResolvedConfig;
use crate::metrics::{Protocol, RequestAttempt};
use crate::runner::{
    browser::run_browser_probe,
    curl::run_curl_probe,
    dns::run_dns_probe,
    http::{run_probe, RunConfig},
    http3::run_http3_probe,
    native::run_native_probe,
    pageload::{run_pageload2_probe, run_pageload3_probe, run_pageload_probe, PageLoadConfig},
    throughput::{
        run_download1_probe, run_download2_probe, run_download3_probe, run_download_probe,
        run_upload1_probe, run_upload2_probe, run_upload3_probe, run_upload_probe,
        run_webdownload_probe, run_webupload_probe, ThroughputConfig,
    },
    tls::{run_tls_probe, run_tls_resumption_probe},
    udp::{run_udp_probe, UdpProbeConfig},
    udp_throughput::{run_udpdownload_probe, run_udpupload_probe, UdpThroughputConfig},
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
        (Protocol::Http1, _) | (Protocol::Http2, _) | (Protocol::Tcp, _) => {
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

pub fn log_attempt(a: &RequestAttempt) {
    use crate::metrics::Protocol::*;
    let status = if a.success { "✓" } else { "✗" };
    let retry_suffix = if a.retry_count > 0 {
        format!(" (retry #{})", a.retry_count)
    } else {
        String::new()
    };

    match &a.protocol {
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
                    recv = ut.datagrams_received,
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
                info!(
                    "{status} #{seq} [{mode}] proto={proto} TTFB:{ttfb:.1}ms \
                     DCL:{dcl:.1}ms Load:{load:.1}ms res={res} bytes={bytes} [{protos}]{retry}",
                    mode = a.protocol,
                    seq = a.sequence_num,
                    proto = b.protocol,
                    ttfb = b.ttfb_ms,
                    dcl = b.dom_content_loaded_ms,
                    load = b.load_ms,
                    res = b.resource_count,
                    bytes = b.transferred_bytes,
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

pub fn published_logical_attempts(attempts: Vec<RequestAttempt>) -> Vec<RequestAttempt> {
    attempts.into_iter().last().into_iter().collect()
}
