use anyhow::Context;
use chrono::Utc;
use clap::Parser;
use networker_tester::cli;
use networker_tester::metrics::PageLoadResult;
use networker_tester::metrics::{
    attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value, Protocol,
    RequestAttempt, TestRun,
};
use networker_tester::output::{excel, html, json, sql};
use networker_tester::runner::{
    curl::run_curl_probe,
    dns::run_dns_probe,
    http::{run_probe, RunConfig},
    http3::run_http3_probe,
    native::run_native_probe,
    pageload::{run_pageload2_probe, run_pageload3_probe, run_pageload_probe, PageLoadConfig},
    throughput::{
        run_download_probe, run_upload_probe, run_webdownload_probe, run_webupload_probe,
        ThroughputConfig,
    },
    tls::run_tls_probe,
    udp::{run_udp_probe, UdpProbeConfig},
    udp_throughput::{run_udpdownload_probe, run_udpupload_probe, UdpThroughputConfig},
};
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // rustls 0.23 requires an explicit CryptoProvider.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring CryptoProvider");

    let cli = cli::Cli::parse();
    let config_file = if let Some(ref path) = cli.config {
        Some(cli::load_config(path)?)
    } else {
        None
    };
    let cfg = cli.resolve(config_file);
    cfg.validate()?;

    let log_filter = if let Some(ref level) = cfg.log_level {
        tracing_subscriber::EnvFilter::new(level)
    } else {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    };
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    // ── Privilege notice (Linux only) ─────────────────────────────────────────
    #[cfg(target_os = "linux")]
    if !cli::running_as_root() {
        eprintln!(
            "[info] Not running as root. TCP kernel stats (retransmits, cwnd) are \
             captured via TCP_INFO without root. Run with sudo to enable raw-socket \
             and BPF metrics."
        );
    }

    let target = url::Url::parse(&cfg.target).context("Invalid --target URL")?;
    let target_host = target.host_str().unwrap_or("unknown").to_string();

    let modes = cfg.parsed_modes();
    if modes.is_empty() {
        anyhow::bail!(
            "No valid modes specified. Use: tcp,http1,http2,http3,udp,dns,tls,native,curl,\
             download,upload,webdownload,webupload,udpdownload,udpupload,pageload,pageload2,pageload3"
        );
    }

    // ── ALPN warning for H2/H3/pageload2 over plain HTTP ─────────────────────
    for mode in &modes {
        if matches!(
            mode,
            Protocol::Http2 | Protocol::Http3 | Protocol::PageLoad2 | Protocol::PageLoad3
        ) && target.scheme() == "http"
        {
            warn!(
                "{mode} requires HTTPS for ALPN negotiation; \
                 over plain http:// the connection falls back to HTTP/1.1. \
                 Use https:// (e.g. https://host:8443) with --insecure for the \
                 self-signed endpoint cert."
            );
        }
    }

    let payload_sizes = cfg.parsed_payload_sizes().context("--payload-sizes")?;
    let has_throughput = modes.iter().any(|m| {
        matches!(
            m,
            Protocol::Download
                | Protocol::Upload
                | Protocol::WebDownload
                | Protocol::WebUpload
                | Protocol::UdpDownload
                | Protocol::UdpUpload
        )
    });
    if has_throughput && payload_sizes.is_empty() {
        anyhow::bail!(
            "--payload-sizes required for download/upload/webdownload/webupload/\
             udpdownload/udpupload modes (e.g. --payload-sizes 4k,64k,1m)"
        );
    }

    let run_id = Uuid::new_v4();
    let started_at = Utc::now();

    info!(
        run_id  = %run_id,
        target  = %cfg.target,
        modes   = ?modes.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        runs    = cfg.runs,
        retries = cfg.retries,
        version = env!("CARGO_PKG_VERSION"),
        "Starting networker-tester"
    );

    let probe_cfg = RunConfig {
        timeout_ms: cfg.timeout * 1000,
        dns_enabled: cfg.dns_enabled,
        ipv4_only: cfg.ipv4_only,
        ipv6_only: cfg.ipv6_only,
        insecure: cfg.insecure,
        payload_size: cfg.payload_size,
        path: target.path().to_string(),
        ca_bundle: cfg.ca_bundle.clone(),
        proxy: cfg.proxy.clone(),
        no_proxy: cfg.no_proxy,
    };

    let throughput_cfg = ThroughputConfig {
        run_cfg: probe_cfg.clone(),
        base_url: target.clone(),
    };

    let pageload_cfg = PageLoadConfig {
        run_cfg: probe_cfg.clone(),
        base_url: target.clone(),
        asset_sizes: cfg.page_asset_sizes.clone(),
        preset_name: cfg.page_preset_name.clone(),
    };

    // Expand modes × payload sizes into a flat task list.
    // All throughput modes generate one task per payload size.
    let mode_tasks: Vec<(Protocol, Option<usize>)> = modes
        .iter()
        .flat_map(|p| match p {
            Protocol::Download
            | Protocol::Upload
            | Protocol::WebDownload
            | Protocol::WebUpload
            | Protocol::UdpDownload
            | Protocol::UdpUpload => payload_sizes
                .iter()
                .map(|&sz| (p.clone(), Some(sz)))
                .collect::<Vec<_>>(),
            other => vec![(other.clone(), None)],
        })
        .collect();

    let udp_cfg = UdpProbeConfig {
        target_host: target_host.clone(),
        target_port: cfg.udp_port,
        probe_count: cfg.udp_probes,
        timeout_ms: cfg.timeout * 1000,
        payload_size: 64,
    };

    let udp_throughput_cfg = UdpThroughputConfig {
        target_host: target_host.clone(),
        target_port: cfg.udp_throughput_port,
        timeout_ms: cfg.timeout * 1000,
    };

    // ── Collect all attempts ──────────────────────────────────────────────────
    let retries = cfg.retries;
    let mut all_attempts = Vec::new();
    let mut seq = 0u32;

    for run_num in 0..cfg.runs {
        info!("Run {}/{}", run_num + 1, cfg.runs);

        let futures: Vec<_> = mode_tasks
            .iter()
            .map(|(proto, payload_sz)| {
                let proto = proto.clone();
                let payload_sz = *payload_sz;
                let target_clone = target.clone();
                let probe_cfg_clone = probe_cfg.clone();
                let udp_cfg_clone = udp_cfg.clone();
                let udp_throughput_cfg_clone = udp_throughput_cfg.clone();
                let throughput_cfg_clone = throughput_cfg.clone();
                let pageload_cfg_clone = pageload_cfg.clone();
                let current_seq = seq;
                seq += 1;

                async move {
                    // First attempt
                    let mut a = dispatch_once(
                        &proto,
                        payload_sz,
                        run_id,
                        current_seq,
                        &target_clone,
                        &probe_cfg_clone,
                        &udp_cfg_clone,
                        &udp_throughput_cfg_clone,
                        &throughput_cfg_clone,
                        &pageload_cfg_clone,
                    )
                    .await;

                    // Retry loop
                    for retry_num in 1..=retries {
                        if a.success {
                            break;
                        }
                        let mut retry_a = dispatch_once(
                            &proto,
                            payload_sz,
                            run_id,
                            current_seq,
                            &target_clone,
                            &probe_cfg_clone,
                            &udp_cfg_clone,
                            &udp_throughput_cfg_clone,
                            &throughput_cfg_clone,
                            &pageload_cfg_clone,
                        )
                        .await;
                        retry_a.retry_count = retry_num;
                        a = retry_a;
                    }

                    // Progress logging
                    log_attempt(&a);
                    a
                }
            })
            .collect();

        // Run futures with bounded concurrency
        use futures::stream::{self, StreamExt};
        let results: Vec<_> = stream::iter(futures)
            .buffer_unordered(cfg.concurrency)
            .collect()
            .await;
        all_attempts.extend(results);
    }

    let finished_at = Utc::now();

    let run = TestRun {
        run_id,
        started_at,
        finished_at: Some(finished_at),
        target_url: cfg.target.clone(),
        target_host: target_host.clone(),
        modes: modes.iter().map(|m| m.to_string()).collect(),
        total_runs: cfg.runs,
        concurrency: cfg.concurrency as u32,
        timeout_ms: cfg.timeout * 1000,
        client_os: std::env::consts::OS.to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        attempts: all_attempts,
    };

    // Summary
    info!(
        success = run.success_count(),
        failure = run.failure_count(),
        "Run complete"
    );

    // ── Ensure output dir exists ──────────────────────────────────────────────
    let out_dir = PathBuf::from(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir).context("Cannot create output directory")?;

    let ts = started_at.format("%Y%m%d-%H%M%S");

    // ── JSON artifact ─────────────────────────────────────────────────────────
    let json_path = out_dir.join(format!("run-{ts}.json"));
    json::save(&run, &json_path).context("Failed to write JSON artifact")?;
    info!(path = %json_path.display(), "JSON artifact saved");

    // ── HTML report ───────────────────────────────────────────────────────────
    let html_path = out_dir.join(&cfg.html_report);
    let css_href = cfg.css.as_deref().or(Some("report.css"));
    html::save(&run, &html_path, css_href).context("Failed to write HTML report")?;
    info!(path = %html_path.display(), "HTML report saved");

    // Copy default CSS to output dir if it doesn't exist yet
    copy_default_css(&out_dir);

    // ── Excel report ──────────────────────────────────────────────────────────
    if cfg.excel {
        let name = format!("run-{ts}-{}.xlsx", run.run_id);
        let xlsx_path = out_dir.join(&name);
        match excel::save(&run, &xlsx_path) {
            Ok(()) => info!(path = %xlsx_path.display(), "Excel report saved"),
            Err(e) => warn!("Excel report failed: {e:#}"),
        }
    }

    // ── SQL insert ────────────────────────────────────────────────────────────
    if cfg.save_to_sql {
        if let Some(conn_str) = &cfg.connection_string {
            info!("Inserting into SQL Server…");
            match sql::save(&run, conn_str).await {
                Ok(()) => info!("SQL insert complete"),
                Err(e) => error!("SQL insert failed: {e:#}"),
            }
        }
    }

    print_summary(&run);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Single probe dispatch (used for both the initial attempt and retries)
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn dispatch_once(
    proto: &Protocol,
    payload_sz: Option<usize>,
    run_id: Uuid,
    seq: u32,
    target: &url::Url,
    cfg: &RunConfig,
    udp_cfg: &UdpProbeConfig,
    udp_throughput_cfg: &UdpThroughputConfig,
    throughput_cfg: &ThroughputConfig,
    pageload_cfg: &PageLoadConfig,
) -> RequestAttempt {
    match (proto, payload_sz) {
        (Protocol::Download, Some(sz)) => run_download_probe(run_id, seq, sz, throughput_cfg).await,
        (Protocol::Upload, Some(sz)) => run_upload_probe(run_id, seq, sz, throughput_cfg).await,
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
            run_probe(run_id, seq, proto.clone(), target, cfg).await
        }
        (Protocol::Http3, _) => run_http3_probe(run_id, seq, target, cfg.timeout_ms).await,
        (Protocol::Udp, _) => run_udp_probe(run_id, seq, udp_cfg).await,
        (Protocol::Dns, _) => {
            let host = target.host_str().unwrap_or("");
            run_dns_probe(run_id, seq, host, cfg.ipv4_only, cfg.ipv6_only).await
        }
        (Protocol::Tls, _) => run_tls_probe(run_id, seq, target, cfg).await,
        (Protocol::Native, _) => run_native_probe(run_id, seq, target, cfg).await,
        (Protocol::Curl, _) => run_curl_probe(run_id, seq, target, cfg).await,
        (Protocol::PageLoad, _) => run_pageload_probe(run_id, seq, pageload_cfg).await,
        (Protocol::PageLoad2, _) => run_pageload2_probe(run_id, seq, pageload_cfg).await,
        (Protocol::PageLoad3, _) => run_pageload3_probe(run_id, seq, pageload_cfg).await,
        _ => unreachable!("Upload/WebUpload/UdpDownload/UdpUpload without payload_size"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn log_attempt(a: &networker_tester::metrics::RequestAttempt) {
    use networker_tester::metrics::Protocol::*;
    let status = if a.success { "✓" } else { "✗" };
    let retry_suffix = if a.retry_count > 0 {
        format!(" (retry #{})", a.retry_count)
    } else {
        String::new()
    };

    match &a.protocol {
        Http1 | Http2 | Http3 | Tcp | Native | Curl => {
            let dns_ms = a.dns.as_ref().map(|d| d.duration_ms).unwrap_or(0.0);
            let tcp_ms = a.tcp.as_ref().map(|t| t.connect_duration_ms).unwrap_or(0.0);
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

            info!(
                "{status} #{seq} [{proto}] {status_code} {ver} \
                 DNS:{dns:.1}ms TCP:{tcp:.1}ms TLS:{tls:.1}ms TTFB:{ttfb:.1}ms Total:{total:.1}ms{cpu}{csw}{retry}",
                seq = a.sequence_num,
                proto = a.protocol,
                dns = dns_ms,
                tcp = tcp_ms,
                tls = tls_ms,
                ttfb = ttfb_ms,
                total = total_ms,
                retry = retry_suffix,
            );
        }
        Download | Upload | WebDownload | WebUpload => {
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

fn fmt_bytes(n: usize) -> String {
    if n >= 1 << 30 {
        format!("{:.1}GiB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.0}MiB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0}KiB", n as f64 / (1u64 << 10) as f64)
    } else {
        format!("{n}B")
    }
}

fn print_summary(run: &TestRun) {
    let ok = run.success_count();
    let fail = run.failure_count();
    let total = run.attempts.len();

    // Extract server version from the first attempt that reported it.
    let server_version: String = run
        .attempts
        .iter()
        .find_map(|a| {
            a.server_timing
                .as_ref()
                .and_then(|st| st.server_version.as_deref())
        })
        .unwrap_or("—")
        .to_string();

    println!("\n══════════════════════════════════════════════");
    println!(" Networker Tester – Run {}", run.run_id);
    println!("══════════════════════════════════════════════");
    println!(" Target         : {}", run.target_url);
    println!(" Modes          : {}", run.modes.join(", "));
    println!(" Results        : {ok}/{total} succeeded  ({fail} failed)");
    println!(" Client version : {}", run.client_version);
    println!(" Server version : {server_version}");

    if let Some(fin) = run.finished_at {
        let dur = (fin - run.started_at).num_milliseconds();
        println!(" Duration       : {dur}ms total");
    }

    // Build (proto, Option<payload_bytes>) groups in canonical protocol order.
    use std::collections::BTreeSet;
    let ordered_protos = [
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Http3,
        Protocol::Native,
        Protocol::Curl,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Dns,
        Protocol::Tls,
        Protocol::Download,
        Protocol::Upload,
        Protocol::WebDownload,
        Protocol::WebUpload,
        Protocol::UdpDownload,
        Protocol::UdpUpload,
        Protocol::PageLoad,
        Protocol::PageLoad2,
        Protocol::PageLoad3,
    ];
    let stat_groups: Vec<(Protocol, Option<usize>)> = ordered_protos
        .iter()
        .flat_map(|proto| {
            let payloads: BTreeSet<Option<usize>> = run
                .attempts
                .iter()
                .filter(|a| &a.protocol == proto)
                .map(attempt_payload_bytes)
                .collect();
            payloads.into_iter().map(move |p| (proto.clone(), p))
        })
        .collect();

    let group_label = |proto: &Protocol, payload: Option<usize>| match payload {
        None => proto.to_string(),
        Some(b) => format!("{proto} {}", fmt_bytes(b)),
    };

    // Per-protocol/payload averages table
    println!(
        "\n {:<16} │ #   │ Avg DNS │ Avg TCP │ Avg TLS │ Avg TTFB │ Avg Total",
        "Protocol"
    );
    println!("──────────────────┼─────┼─────────┼─────────┼─────────┼──────────┼───────────");

    for (proto, payload) in &stat_groups {
        let rows: Vec<_> = run
            .attempts
            .iter()
            .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
            .collect();
        if rows.is_empty() {
            continue;
        }

        let avg_f = |f: fn(&RequestAttempt) -> Option<f64>| -> String {
            let vals: Vec<f64> = rows.iter().filter_map(|a| f(a)).collect();
            if vals.is_empty() {
                "—".into()
            } else {
                format!("{:.1}ms", vals.iter().sum::<f64>() / vals.len() as f64)
            }
        };

        println!(
            " {label:<16} │ {n:<3} │ {dns:<7} │ {tcp:<7} │ {tls:<7} │ {ttfb:<8} │ {total}",
            label = group_label(proto, *payload),
            n = rows.len(),
            dns = avg_f(|a| a.dns.as_ref().map(|d| d.duration_ms)),
            tcp = avg_f(|a| a.tcp.as_ref().map(|t| t.connect_duration_ms)),
            tls = avg_f(|a| a.tls.as_ref().map(|t| t.handshake_duration_ms)),
            ttfb = avg_f(|a| a.http.as_ref().map(|h| h.ttfb_ms)),
            total = avg_f(|a| {
                a.http
                    .as_ref()
                    .map(|h| h.total_duration_ms)
                    .or_else(|| a.udp.as_ref().map(|u| u.rtt_avg_ms))
                    .or_else(|| a.udp_throughput.as_ref().map(|ut| ut.transfer_ms))
            }),
        );
    }

    // Per-group statistics (primary metric: ms for latency, MB/s for throughput)
    let has_stats = stat_groups.iter().any(|(proto, payload)| {
        run.attempts
            .iter()
            .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
            .any(|a| primary_metric_value(a).is_some())
    });
    if has_stats {
        println!();
        println!(
            " {:<16} │ Metric           │  N  │    Min   │   Mean   │   p50    │   p95    │   p99    │    Max   │  StdDev",
            "Protocol"
        );
        println!(
            "──────────────────┼──────────────────┼─────┼──────────┼──────────┼──────────┼──────────┼──────────┼──────────┼─────────"
        );
        for (proto, payload) in &stat_groups {
            let vals: Vec<f64> = run
                .attempts
                .iter()
                .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
                .filter_map(primary_metric_value)
                .collect();
            if let Some(s) = compute_stats(&vals) {
                let label = primary_metric_label(proto);
                println!(
                    " {grp:<16} │ {label:<16} │ {n:<3} │ {min:>8.2} │ {mean:>8.2} │ {p50:>8.2} │ {p95:>8.2} │ {p99:>8.2} │ {max:>8.2} │ {stddev:>7.2}",
                    grp = group_label(proto, *payload),
                    n = s.count,
                    min = s.min,
                    mean = s.mean,
                    p50 = s.p50,
                    p95 = s.p95,
                    p99 = s.p99,
                    max = s.max,
                    stddev = s.stddev,
                );
            }
        }
    }

    // Protocol comparison table when any pageload variant is present
    let has_pageload = run.attempts.iter().any(|a| {
        matches!(
            a.protocol,
            Protocol::PageLoad | Protocol::PageLoad2 | Protocol::PageLoad3
        )
    });
    if has_pageload {
        print_comparison(run);
    }

    println!("══════════════════════════════════════════════\n");
}

fn print_comparison(run: &TestRun) {
    let row = |proto: &Protocol| -> Option<String> {
        let attempts: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| &a.protocol == proto)
            .collect();
        if attempts.is_empty() {
            return None;
        }
        let n = attempts.len();
        let pl_results: Vec<&PageLoadResult> = attempts
            .iter()
            .filter_map(|a| a.page_load.as_ref())
            .collect();
        if pl_results.is_empty() {
            return None;
        }
        let total_ms_vals: Vec<f64> = pl_results.iter().map(|p| p.total_ms).collect();
        let avg_conns: f64 = pl_results
            .iter()
            .map(|p| p.connections_opened as f64)
            .sum::<f64>()
            / n as f64;
        let avg_assets: f64 = pl_results
            .iter()
            .map(|p| p.assets_fetched as f64)
            .sum::<f64>()
            / n as f64;
        let total_assets = pl_results.first().map(|p| p.asset_count).unwrap_or(0);
        let avg_tls_ms: f64 = pl_results.iter().map(|p| p.tls_setup_ms).sum::<f64>() / n as f64;
        let avg_tls_pct: f64 = pl_results
            .iter()
            .map(|p| p.tls_overhead_ratio * 100.0)
            .sum::<f64>()
            / n as f64;
        let cpu_vals: Vec<f64> = pl_results.iter().filter_map(|p| p.cpu_time_ms).collect();
        let avg_cpu_str = if cpu_vals.is_empty() {
            "  —".into()
        } else {
            format!(
                "{:>5.1}",
                cpu_vals.iter().sum::<f64>() / cpu_vals.len() as f64
            )
        };
        let stats = networker_tester::metrics::compute_stats(&total_ms_vals)?;
        Some(format!(
            " {proto:<10} │ {n:<3} │ {assets:>3.0}/{total:<3} │ {conns:>5.1} │ {tls_ms:>8.1} │ {tls_pct:>6.1}% │ {cpu:>8} │ {p50:>8.1}ms │ {min:>8.1}ms │ {max:>8.1}ms",
            proto = proto,
            n = n,
            assets = avg_assets,
            total = total_assets,
            conns = avg_conns,
            tls_ms = avg_tls_ms,
            tls_pct = avg_tls_pct,
            cpu = avg_cpu_str,
            p50 = stats.p50,
            min = stats.min,
            max = stats.max,
        ))
    };

    println!();
    println!(" ── Protocol Comparison (Page Load) ─────────────────────────────────────────────────────────────────────────");
    println!(" Protocol  │ N   │ Assets  │ Conns │  TLS ms  │  TLS %  │  CPU ms  │   p50    │   Min    │   Max");
    println!("───────────┼─────┼─────────┼───────┼──────────┼─────────┼──────────┼──────────┼──────────┼──────────");
    for proto in &[Protocol::PageLoad, Protocol::PageLoad2, Protocol::PageLoad3] {
        if let Some(r) = row(proto) {
            println!("{r}");
        }
    }
}

/// Copy the bundled `report.css` from the binary's embedded bytes to the
/// output directory so the HTML report can link to it.
fn copy_default_css(out_dir: &Path) {
    let dest = out_dir.join("report.css");
    if dest.exists() {
        return;
    }
    if let Ok(src) = std::fs::read("assets/report.css") {
        let _ = std::fs::write(&dest, src);
    } else {
        let _ = std::fs::write(&dest, networker_tester::output::html::FALLBACK_CSS);
    }
}
