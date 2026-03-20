use anyhow::Context;
use chrono::Utc;
use clap::Parser;
use networker_tester::capture;
use networker_tester::cli;
use networker_tester::cli::ResolvedConfig;
use networker_tester::metrics::{
    attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value, HostInfo,
    NetworkBaseline, NetworkType, PageLoadResult, Protocol, RequestAttempt, TestRun,
};
use networker_tester::output;
use networker_tester::output::{excel, html, json};
use networker_tester::runner::{
    browser::run_browser_probe,
    curl::run_curl_probe,
    dns::run_dns_probe,
    http::{run_probe, RunConfig},
    http3::run_http3_probe,
    native::run_native_probe,
    pageload::{
        run_pageload2_probe, run_pageload2_warm, run_pageload3_probe, run_pageload3_warm,
        run_pageload_probe, warmup_pageload2, warmup_pageload3, PageLoadConfig, SharedH2Conn,
    },
    throughput::{
        run_download1_probe, run_download2_probe, run_download3_probe, run_download_probe,
        run_upload1_probe, run_upload2_probe, run_upload3_probe, run_upload_probe,
        run_webdownload_probe, run_webupload_probe, ThroughputConfig,
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
    if cfg.json_stdout {
        // When outputting JSON to stdout, send logs to stderr so stdout is clean
        tracing_subscriber::fmt()
            .with_env_filter(log_filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(log_filter).init();
    }

    // ── Privilege notice (Linux only) ─────────────────────────────────────────
    #[cfg(target_os = "linux")]
    if !cli::running_as_root() {
        eprintln!(
            "[info] Not running as root. TCP kernel stats (retransmits, cwnd) are \
             captured via TCP_INFO without root. Run with sudo to enable raw-socket \
             and BPF metrics."
        );
    }

    let modes = cfg.parsed_modes();
    if modes.is_empty() {
        anyhow::bail!(
            "No valid modes specified. Use: tcp,http1,http2,http3,udp,dns,tls,native,curl,\
             download,download1,download2,download3,upload,upload1,upload2,upload3,webdownload,webupload,udpdownload,udpupload,\
             pageload(H1+H2+H3),pageload1,pageload2,pageload3,\
             browser(H1+H2+H3),browser1,browser2,browser3"
        );
    }

    let payload_sizes = cfg.parsed_payload_sizes().context("--payload-sizes")?;
    let has_throughput = modes.iter().any(|m| {
        matches!(
            m,
            Protocol::Download
                | Protocol::Download1
                | Protocol::Download2
                | Protocol::Download3
                | Protocol::Upload
                | Protocol::Upload1
                | Protocol::Upload2
                | Protocol::Upload3
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

    info!(
        targets = ?cfg.targets,
        modes   = ?modes.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        runs    = cfg.runs,
        retries = cfg.retries,
        version = env!("CARGO_PKG_VERSION"),
        "Starting networker-tester"
    );

    if cfg.impairment.delay_ms > 0 {
        warn!(
            delay_ms = cfg.impairment.delay_ms,
            "impairment delay uses the endpoint /delay route and is intended for controlled benchmark environments, not public exposure"
        );
        if cfg.impairment.delay_ms as u128 > cfg.timeout as u128 {
            warn!(
                delay_ms = cfg.impairment.delay_ms,
                timeout_ms = cfg.timeout,
                "configured impairment delay exceeds request timeout; this scenario may fail by construction"
            );
        }
    }

    // ── Ensure output dir exists ──────────────────────────────────────────────
    let out_dir = PathBuf::from(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir).context("Cannot create output directory")?;

    let capture_plan = capture::build_plan(&cfg, &out_dir);
    if cfg.packet_capture.mode.captures_endpoint() {
        warn!(
            "endpoint-side packet capture requested but not implemented yet; continuing with tester-side only"
        );
    }
    let capture_session = match capture_plan {
        Some(plan) => match capture::start(plan).await {
            Ok(session) => {
                info!("Tester-side packet capture started");
                Some(session)
            }
            Err(e) => {
                if cfg.packet_capture.install_requirements {
                    warn!("packet capture requested but requirements/setup are incomplete: {e:#}");
                } else {
                    warn!("packet capture requested but unavailable: {e:#}");
                }
                None
            }
        },
        None => None,
    };

    // ── Run probes for every target ───────────────────────────────────────────
    let mut all_runs: Vec<TestRun> = Vec::new();
    for target_url_str in &cfg.targets {
        info!(target = %target_url_str, "Running probes for target");
        let run = run_for_target(target_url_str, &cfg, &modes, &payload_sizes).await?;
        all_runs.push(run);
    }

    // ── Finalize packet capture ────────────────────────────────────────────
    let packet_capture_summary = if let Some(session) = capture_session {
        match session.finalize().await {
            Ok(Some(summary)) => {
                info!(
                    tcp_packets = summary.tcp_packets,
                    udp_packets = summary.udp_packets,
                    retransmissions = summary.retransmissions,
                    duplicate_acks = summary.duplicate_acks,
                    resets = summary.resets,
                    "Packet capture summary saved"
                );
                Some(summary)
            }
            Ok(None) => {
                info!("Packet capture finalized without summary output");
                None
            }
            Err(e) => {
                warn!("packet capture finalize failed: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Attach capture summary to runs for JSON/DB output
    if let Some(ref summary) = packet_capture_summary {
        for run in &mut all_runs {
            run.packet_capture_summary = Some(summary.clone());
        }
    }

    // ── JSON stdout mode (for agent integration) ─────────────────────────────
    if cfg.json_stdout {
        // Output all runs as JSON array to stdout, skip file outputs
        if all_runs.len() == 1 {
            let first = all_runs
                .first()
                .context("no targets produced any test runs")?;
            match serde_json::to_string(first) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    error!(error = %e, "failed to serialize test run");
                    println!("{{\"error\":\"serialization failed\"}}");
                }
            }
        } else {
            match serde_json::to_string(&all_runs) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    error!(error = %e, "failed to serialize test runs");
                    println!("{{\"error\":\"serialization failed\"}}");
                }
            }
        }
        return Ok(());
    }

    let first_run = all_runs
        .first()
        .context("no targets produced any test runs")?;
    let ts = first_run.started_at.format("%Y%m%d-%H%M%S");
    let multi = all_runs.len() > 1;

    // ── JSON artifact (one per target) ────────────────────────────────────────
    for (i, run) in all_runs.iter().enumerate() {
        let name = if multi {
            format!("run-{ts}-{}.json", i + 1)
        } else {
            format!("run-{ts}.json")
        };
        let json_path = out_dir.join(&name);
        json::save(run, &json_path).context("Failed to write JSON artifact")?;
        info!(path = %json_path.display(), "JSON artifact saved");
    }

    // ── HTML report (single combined report) ──────────────────────────────────
    let html_path = out_dir.join(&cfg.html_report);
    let css_href = cfg.css.as_deref().or(Some("report.css"));
    html::save_multi(
        &all_runs,
        &html_path,
        css_href,
        packet_capture_summary.as_ref(),
    )
    .context("Failed to write HTML report")?;
    info!(path = %html_path.display(), "HTML report saved");

    // Copy default CSS to output dir if it doesn't exist yet
    copy_default_css(&out_dir);

    // ── Excel report (one per target) ─────────────────────────────────────────
    if cfg.excel {
        for (i, run) in all_runs.iter().enumerate() {
            let name = if multi {
                format!("run-{ts}-{}.xlsx", i + 1)
            } else {
                format!("run-{ts}-{}.xlsx", run.run_id)
            };
            let xlsx_path = out_dir.join(&name);
            match excel::save(run, &xlsx_path, packet_capture_summary.as_ref()) {
                Ok(()) => info!(path = %xlsx_path.display(), "Excel report saved"),
                Err(e) => warn!("Excel report failed: {e:#}"),
            }
        }
    }

    // ── Database insert (one per target) ─────────────────────────────────────
    let do_db = cfg.save_to_db || cfg.save_to_sql;
    if do_db {
        if let Some(db_url) = cfg.db_url.as_deref().or(cfg.connection_string.as_deref()) {
            match output::db::connect(db_url).await {
                Ok(backend) => {
                    if cfg.db_migrate {
                        if let Err(e) = backend.migrate().await {
                            error!("Database migration failed: {e:#}");
                        }
                    }
                    for run in &all_runs {
                        info!(target = %run.target_url, "Inserting into database…");
                        match backend.save(run).await {
                            Ok(()) => info!("Database insert complete"),
                            Err(e) => error!("Database insert failed: {e:#}"),
                        }
                    }
                }
                Err(e) => error!("Database connection failed: {e:#}"),
            }
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    for run in &all_runs {
        print_summary(run);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-target probe runner
// ─────────────────────────────────────────────────────────────────────────────

async fn run_for_target(
    target_url_str: &str,
    cfg: &cli::ResolvedConfig,
    modes: &[Protocol],
    payload_sizes: &[usize],
) -> anyhow::Result<TestRun> {
    let target = url::Url::parse(target_url_str)
        .with_context(|| format!("Invalid --target URL: {target_url_str}"))?;
    let target_host = target.host_str().unwrap_or("unknown").to_string();

    // ── ALPN warning for H2/H3/pageload2/browser over plain HTTP ─────────────
    for mode in modes {
        if matches!(
            mode,
            Protocol::Http2
                | Protocol::Http3
                | Protocol::PageLoad2
                | Protocol::PageLoad3
                | Protocol::Browser
                | Protocol::Browser2
                | Protocol::Browser3
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

    // ── Fetch server info (connectivity check + metadata) ─────────────────────
    let server_info = match fetch_server_info(&target, cfg.insecure).await {
        Some(info) => {
            info!(
                "Server: {} {} | {} cores | {} MB RAM | {} | v{} | region: {}",
                info.os,
                info.arch,
                info.cpu_cores,
                info.total_memory_mb.unwrap_or(0),
                info.os_version.as_deref().unwrap_or("?"),
                info.server_version.as_deref().unwrap_or("?"),
                info.region.as_deref().unwrap_or("unknown"),
            );
            Some(info)
        }
        None => {
            warn!(
                "Could not fetch server info from /info — server may not be a networker-endpoint"
            );
            None
        }
    };
    let client_info = Some(HostInfo::collect_local());

    // ── Measure network baseline RTT ────────────────────────────────────────
    let baseline = match measure_baseline(&target).await {
        Some(bl) if bl.samples > 0 => {
            info!(
                "Network baseline: {} | RTT avg={:.2}ms min={:.2}ms max={:.2}ms p50={:.2}ms p95={:.2}ms ({} samples)",
                bl.network_type, bl.rtt_avg_ms, bl.rtt_min_ms, bl.rtt_max_ms,
                bl.rtt_p50_ms, bl.rtt_p95_ms, bl.samples,
            );
            Some(bl)
        }
        Some(bl) => {
            warn!(
                "Network baseline: {} | RTT probes failed (target may be unreachable) — network type still detected",
                bl.network_type,
            );
            Some(bl)
        }
        None => {
            warn!("Could not measure network baseline");
            None
        }
    };

    let run_id = Uuid::new_v4();
    let started_at = Utc::now();

    info!(
        run_id  = %run_id,
        target  = %target_url_str,
        modes   = ?modes.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        runs    = cfg.runs,
        "Starting run"
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
            | Protocol::Download1
            | Protocol::Download2
            | Protocol::Download3
            | Protocol::Upload
            | Protocol::Upload1
            | Protocol::Upload2
            | Protocol::Upload3
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

    // ── Connection reuse: warmup ──────────────────────────────────────────────
    let has_pageload2 = modes.iter().any(|m| matches!(m, Protocol::PageLoad2));
    let has_pageload3 = modes.iter().any(|m| matches!(m, Protocol::PageLoad3));

    // Initialized below after warmup when connection_reuse && has_pageload2
    let shared_h2: Option<std::sync::Arc<SharedH2Conn>> = None;
    #[cfg(feature = "http3")]
    let shared_h3: Option<
        std::sync::Arc<tokio::sync::Mutex<networker_tester::runner::pageload::SharedH3Conn>>,
    > = None;

    let retries = cfg.retries;
    let mut all_attempts = Vec::new();
    let mut seq = 0u32;

    // Warmup probes for connection-reuse modes
    let shared_h2 = if cfg.connection_reuse && has_pageload2 {
        info!("Connection reuse: warming up HTTP/2 connection…");
        let (warmup, conn) = warmup_pageload2(run_id, seq, &pageload_cfg).await;
        log_attempt(&warmup);
        all_attempts.push(warmup);
        seq += 1;
        conn.map(std::sync::Arc::new)
    } else {
        shared_h2
    };

    #[cfg(feature = "http3")]
    let shared_h3 = if cfg.connection_reuse && has_pageload3 {
        info!("Connection reuse: warming up HTTP/3 QUIC connection…");
        let (warmup, conn) = warmup_pageload3(run_id, seq, &pageload_cfg).await;
        log_attempt(&warmup);
        all_attempts.push(warmup);
        seq += 1;
        conn.map(|c| std::sync::Arc::new(tokio::sync::Mutex::new(c)))
    } else {
        shared_h3
    };
    #[cfg(not(feature = "http3"))]
    let shared_h3: Option<std::sync::Arc<tokio::sync::Mutex<()>>> = None;

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
                let shared_h2_clone = shared_h2.clone();
                let shared_h3_clone = shared_h3.clone();
                let current_seq = seq;
                seq += 1;

                async move {
                    // Helper macro to dispatch: warm probe if shared conn, else cold
                    macro_rules! do_dispatch {
                        () => {{
                            if matches!(proto, Protocol::PageLoad2) {
                                if let Some(ref h2) = shared_h2_clone {
                                    run_pageload2_warm(run_id, current_seq, &pageload_cfg_clone, h2)
                                        .await
                                } else {
                                    dispatch_once(
                                        &proto,
                                        payload_sz,
                                        run_id,
                                        current_seq,
                                        &target_clone,
                                        &cfg,
                                        &probe_cfg_clone,
                                        &udp_cfg_clone,
                                        &udp_throughput_cfg_clone,
                                        &throughput_cfg_clone,
                                        &pageload_cfg_clone,
                                    )
                                    .await
                                }
                            } else if matches!(proto, Protocol::PageLoad3)
                                && shared_h3_clone.is_some()
                            {
                                #[cfg(feature = "http3")]
                                {
                                    run_pageload3_warm(
                                        run_id,
                                        current_seq,
                                        &pageload_cfg_clone,
                                        shared_h3_clone.as_ref().unwrap(),
                                    )
                                    .await
                                }
                                #[cfg(not(feature = "http3"))]
                                {
                                    dispatch_once(
                                        &proto,
                                        payload_sz,
                                        run_id,
                                        current_seq,
                                        &target_clone,
                                        &cfg,
                                        &probe_cfg_clone,
                                        &udp_cfg_clone,
                                        &udp_throughput_cfg_clone,
                                        &throughput_cfg_clone,
                                        &pageload_cfg_clone,
                                    )
                                    .await
                                }
                            } else {
                                dispatch_once(
                                    &proto,
                                    payload_sz,
                                    run_id,
                                    current_seq,
                                    &target_clone,
                                    &cfg,
                                    &probe_cfg_clone,
                                    &udp_cfg_clone,
                                    &udp_throughput_cfg_clone,
                                    &throughput_cfg_clone,
                                    &pageload_cfg_clone,
                                )
                                .await
                            }
                        }};
                    }

                    // First attempt
                    let mut a = do_dispatch!();

                    // Retry loop
                    for retry_num in 1..=retries {
                        if a.success {
                            break;
                        }
                        let mut retry_a = do_dispatch!();
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

    // ── HTTP stack comparison probes ──────────────────────────────────────────
    // For each configured HTTP stack (nginx, IIS, etc.), run pageload/browser
    // probes against the stack's ports and tag results with the stack name.
    let stack_modes: Vec<Protocol> = modes
        .iter()
        .filter(|m| {
            matches!(
                m,
                Protocol::PageLoad
                    | Protocol::PageLoad2
                    | Protocol::PageLoad3
                    | Protocol::Browser
                    | Protocol::Browser1
                    | Protocol::Browser2
                    | Protocol::Browser3
            )
        })
        .cloned()
        .collect();

    if !cfg.http_stacks.is_empty() && !stack_modes.is_empty() {
        for stack in &cfg.http_stacks {
            // Probe health endpoint to check if stack is running
            let stack_http_url = rewrite_url_for_stack(&target, stack.http_port, false);
            let health_url = {
                let mut u = stack_http_url.clone();
                u.set_path("/health");
                u
            };

            info!(stack = %stack.name, url = %health_url, "Checking HTTP stack availability");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap();
            match client.get(health_url.as_str()).send().await {
                Ok(resp) if resp.status().is_success() => {
                    info!(stack = %stack.name, "HTTP stack is available");
                }
                Ok(resp) => {
                    warn!(stack = %stack.name, status = %resp.status(), "HTTP stack returned non-OK; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(stack = %stack.name, err = %e, "HTTP stack not reachable; skipping");
                    continue;
                }
            }

            // Build stack URLs: HTTP for pageload (H1.1), HTTPS for pageload2/3/browser.
            // Use root path "/" since nginx serves the test page there (not /health).
            let mut stack_https_url = rewrite_url_for_stack(&target, stack.https_port, true);
            stack_https_url.set_path("/");
            let mut stack_http_root = stack_http_url.clone();
            stack_http_root.set_path("/");

            // Build stack-specific configs (use HTTPS base for pageload2/3)
            let stack_pageload_cfg = PageLoadConfig {
                run_cfg: probe_cfg.clone(),
                base_url: stack_https_url.clone(),
                asset_sizes: cfg.page_asset_sizes.clone(),
                preset_name: cfg.page_preset_name.clone(),
            };
            // Separate config for pageload H1.1 (plain HTTP)
            let stack_pageload_h1_cfg = PageLoadConfig {
                run_cfg: probe_cfg.clone(),
                base_url: stack_http_root.clone(),
                asset_sizes: cfg.page_asset_sizes.clone(),
                preset_name: cfg.page_preset_name.clone(),
            };

            let stack_mode_tasks: Vec<(Protocol, Option<usize>)> =
                stack_modes.iter().map(|p| (p.clone(), None)).collect();

            for run_num in 0..cfg.runs {
                info!(stack = %stack.name, run = run_num + 1, "Stack probe run");

                for (proto, _) in &stack_mode_tasks {
                    // PageLoad (H1.1) uses plain HTTP; everything else uses HTTPS
                    let (stack_target, stack_pl_cfg) = if matches!(proto, Protocol::PageLoad) {
                        (&stack_http_root, &stack_pageload_h1_cfg)
                    } else {
                        (&stack_https_url, &stack_pageload_cfg)
                    };
                    let mut attempt = dispatch_once(
                        proto,
                        None,
                        run_id,
                        seq,
                        stack_target,
                        cfg,
                        &probe_cfg,
                        &udp_cfg,
                        &udp_throughput_cfg,
                        &throughput_cfg,
                        stack_pl_cfg,
                    )
                    .await;
                    seq += 1;

                    // Tag with the HTTP stack name
                    attempt.http_stack = Some(stack.name.clone());

                    // Retry loop
                    for retry_num in 1..=retries {
                        if attempt.success {
                            break;
                        }
                        let mut retry_a = dispatch_once(
                            proto,
                            None,
                            run_id,
                            seq,
                            stack_target,
                            cfg,
                            &probe_cfg,
                            &udp_cfg,
                            &udp_throughput_cfg,
                            &throughput_cfg,
                            stack_pl_cfg,
                        )
                        .await;
                        seq += 1;
                        retry_a.retry_count = retry_num;
                        retry_a.http_stack = Some(stack.name.clone());
                        attempt = retry_a;
                    }

                    log_attempt(&attempt);
                    all_attempts.push(attempt);
                }
            }
        }
    }

    let finished_at = Utc::now();

    let run = TestRun {
        run_id,
        started_at,
        finished_at: Some(finished_at),
        target_url: target_url_str.to_string(),
        target_host,
        modes: modes.iter().map(|m| m.to_string()).collect(),
        total_runs: cfg.runs,
        concurrency: cfg.concurrency as u32,
        timeout_ms: cfg.timeout * 1000,
        client_os: std::env::consts::OS.to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        server_info,
        client_info,
        baseline,
        packet_capture_summary: None,
        attempts: all_attempts,
    };

    info!(
        success = run.success_count(),
        failure = run.failure_count(),
        target  = %target_url_str,
        "Run complete"
    );

    Ok(run)
}

// ─────────────────────────────────────────────────────────────────────────────
// Network baseline: RTT measurement + network type classification
// ─────────────────────────────────────────────────────────────────────────────

/// Classify an IP address as Loopback, LAN (private), or Internet (public).
fn classify_ip(ip: &std::net::IpAddr) -> NetworkType {
    match ip {
        std::net::IpAddr::V4(v4) => {
            if v4.is_loopback() {
                NetworkType::Loopback
            } else if v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64
            {
                // 10.x, 172.16-31.x, 192.168.x, 169.254.x, 100.64-127.x (CGNAT)
                NetworkType::LAN
            } else {
                NetworkType::Internet
            }
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() {
                NetworkType::Loopback
            } else {
                let segs = v6.segments();
                if segs[0] == 0xfe80 || segs[0] & 0xfe00 == 0xfc00 {
                    // Link-local (fe80::) or ULA (fc00::/7)
                    NetworkType::LAN
                } else {
                    NetworkType::Internet
                }
            }
        }
    }
}

/// Classify the network type based on the target hostname/IP.
fn classify_target(host: &str) -> NetworkType {
    if host == "localhost" {
        return NetworkType::Loopback;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return classify_ip(&ip);
    }
    // For hostnames, try DNS resolution and classify the first IP
    use std::net::ToSocketAddrs;
    if let Ok(mut addrs) = (host, 0u16).to_socket_addrs() {
        if let Some(addr) = addrs.next() {
            return classify_ip(&addr.ip());
        }
    }
    NetworkType::Internet // default for unresolvable hostnames
}

/// Measure TCP connect RTT to a target N times (returns sorted RTTs in ms).
async fn measure_rtt(host: &str, port: u16, samples: u32) -> Vec<f64> {
    let mut rtts = Vec::with_capacity(samples as usize);
    let addr = format!("{host}:{port}");
    for _ in 0..samples {
        let t0 = std::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_stream)) => {
                rtts.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            _ => {
                // Connection failed or timed out; skip this sample
            }
        }
        // Small delay between samples to avoid flooding
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    rtts
}

/// Compute a percentile from a sorted slice (linear interpolation).
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = p / 100.0 * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo as f64)
    }
}

/// Run a network baseline measurement: TCP RTT probes + network classification.
/// Always returns the network type (LAN/Internet/Loopback) even if RTT probes fail,
/// so that LAN targets are correctly identified as reference-only in the report.
async fn measure_baseline(target: &url::Url) -> Option<NetworkBaseline> {
    let host = target.host_str()?;
    let port = target.port_or_known_default()?;
    let network_type = classify_target(host);

    let rtts = measure_rtt(host, port, 5).await;
    if rtts.is_empty() {
        // RTT probes failed (target unreachable) but we still know the network type
        return Some(NetworkBaseline {
            samples: 0,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_max_ms: 0.0,
            rtt_p50_ms: 0.0,
            rtt_p95_ms: 0.0,
            network_type,
        });
    }

    let sum: f64 = rtts.iter().sum();
    Some(NetworkBaseline {
        samples: rtts.len() as u32,
        rtt_min_ms: rtts[0],
        rtt_avg_ms: sum / rtts.len() as f64,
        rtt_max_ms: rtts[rtts.len() - 1],
        rtt_p50_ms: percentile(&rtts, 50.0),
        rtt_p95_ms: percentile(&rtts, 95.0),
        network_type,
    })
}

/// Fetch server metadata from GET /info before probes begin.
async fn fetch_server_info(target: &url::Url, insecure: bool) -> Option<HostInfo> {
    let info_url = {
        let mut u = target.clone();
        u.set_path("/info");
        u.set_query(None);
        u.to_string()
    };

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    let resp = client.get(&info_url).send().await.ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;

    let sys = json.get("system")?;
    Some(HostInfo {
        os: sys.get("os")?.as_str()?.to_string(),
        arch: sys.get("arch")?.as_str()?.to_string(),
        cpu_cores: sys.get("cpu_cores")?.as_u64()? as usize,
        total_memory_mb: sys.get("total_memory_mb").and_then(|v| v.as_u64()),
        os_version: sys
            .get("os_version")
            .and_then(|v| v.as_str())
            .map(String::from),
        hostname: sys
            .get("hostname")
            .and_then(|v| v.as_str())
            .map(String::from),
        server_version: json
            .get("version")
            .and_then(|v| v.as_str())
            .map(String::from),
        uptime_secs: json.get("uptime_secs").and_then(|v| v.as_u64()),
        region: json
            .get("region")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// URL rewriting for HTTP stack comparison
// ─────────────────────────────────────────────────────────────────────────────

/// Rewrite a target URL to use a different port for an HTTP stack.
/// If `https` is true, keeps the https:// scheme; otherwise uses http://.
fn rewrite_url_for_stack(base: &url::Url, port: u16, https: bool) -> url::Url {
    let mut u = base.clone();
    let _ = u.set_scheme(if https { "https" } else { "http" });
    let _ = u.set_port(Some(port));
    u
}

fn apply_impairment_target(proto: &Protocol, target: &url::Url, cfg: &ResolvedConfig) -> url::Url {
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
        Protocol::Download1,
        Protocol::Download2,
        Protocol::Download3,
        Protocol::Upload,
        Protocol::Upload1,
        Protocol::Upload2,
        Protocol::Upload3,
        Protocol::WebDownload,
        Protocol::WebUpload,
        Protocol::UdpDownload,
        Protocol::UdpUpload,
        Protocol::PageLoad,
        Protocol::PageLoad2,
        Protocol::PageLoad3,
        Protocol::Browser,
        Protocol::Browser1,
        Protocol::Browser2,
        Protocol::Browser3,
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

    // Protocol comparison table when any pageload or browser variant is present
    let has_pageload = run.attempts.iter().any(|a| {
        matches!(
            a.protocol,
            Protocol::PageLoad
                | Protocol::PageLoad2
                | Protocol::PageLoad3
                | Protocol::Browser
                | Protocol::Browser1
                | Protocol::Browser2
                | Protocol::Browser3
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

    // Browser row (uses BrowserResult, not PageLoadResult)
    let browser_row = |proto: &Protocol| -> Option<String> {
        let attempts: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| &a.protocol == proto)
            .collect();
        if attempts.is_empty() {
            return None;
        }
        let n = attempts.len();
        let br_results: Vec<&networker_tester::metrics::BrowserResult> =
            attempts.iter().filter_map(|a| a.browser.as_ref()).collect();
        if br_results.is_empty() {
            return None;
        }
        let load_ms_vals: Vec<f64> = br_results.iter().map(|b| b.load_ms).collect();
        let avg_resources: f64 = br_results
            .iter()
            .map(|b| b.resource_count as f64)
            .sum::<f64>()
            / n as f64;
        let stats = networker_tester::metrics::compute_stats(&load_ms_vals)?;
        Some(format!(
            " {proto:<10} │ {n:<3} │ {res:>4.0}/—   │   —   │       —  │      —  │       —  │ {p50:>8.1}ms │ {min:>8.1}ms │ {max:>8.1}ms",
            proto = proto,
            n = n,
            res = avg_resources,
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
    for proto in &[
        Protocol::Browser,
        Protocol::Browser1,
        Protocol::Browser2,
        Protocol::Browser3,
    ] {
        if let Some(r) = browser_row(proto) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use networker_tester::cli::{
        ImpairmentProfile, ResolvedImpairmentConfig, ResolvedPacketCaptureConfig,
    };

    #[test]
    fn rewrite_url_for_stack_http_port() {
        let base = url::Url::parse("https://example.com:8443/path").unwrap();
        let result = rewrite_url_for_stack(&base, 8081, false);
        assert_eq!(result.scheme(), "http");
        assert_eq!(result.port(), Some(8081));
        assert_eq!(result.path(), "/path");
        assert_eq!(result.host_str(), Some("example.com"));
    }

    #[test]
    fn rewrite_url_for_stack_https_port() {
        let base = url::Url::parse("https://10.0.0.5:8443/").unwrap();
        let result = rewrite_url_for_stack(&base, 8444, true);
        assert_eq!(result.scheme(), "https");
        assert_eq!(result.port(), Some(8444));
    }

    #[test]
    fn rewrite_url_for_stack_preserves_host_and_path() {
        let base =
            url::Url::parse("https://my-server.eastus.cloudapp.azure.com:8443/test").unwrap();
        let result = rewrite_url_for_stack(&base, 8082, true);
        assert_eq!(
            result.host_str(),
            Some("my-server.eastus.cloudapp.azure.com")
        );
        assert_eq!(result.path(), "/test");
        assert_eq!(result.port(), Some(8082));
    }

    #[test]
    fn stack_mode_filter_keeps_only_pageload_and_browser() {
        let modes = vec![
            Protocol::Http1,
            Protocol::Http2,
            Protocol::Tcp,
            Protocol::PageLoad,
            Protocol::PageLoad2,
            Protocol::PageLoad3,
            Protocol::Browser,
            Protocol::Browser1,
            Protocol::Browser2,
            Protocol::Browser3,
            Protocol::Download,
            Protocol::Dns,
        ];
        let stack_modes: Vec<Protocol> = modes
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    Protocol::PageLoad
                        | Protocol::PageLoad2
                        | Protocol::PageLoad3
                        | Protocol::Browser
                        | Protocol::Browser1
                        | Protocol::Browser2
                        | Protocol::Browser3
                )
            })
            .cloned()
            .collect();
        assert_eq!(stack_modes.len(), 7);
        assert!(stack_modes.contains(&Protocol::PageLoad));
        assert!(stack_modes.contains(&Protocol::Browser3));
        assert!(!stack_modes.contains(&Protocol::Http1));
        assert!(!stack_modes.contains(&Protocol::Download));
    }

    fn sample_resolved_config(delay_ms: u64) -> ResolvedConfig {
        ResolvedConfig {
            targets: vec!["https://127.0.0.1:8443/health".into()],
            modes: vec![],
            runs: 1,
            concurrency: 1,
            timeout: 1000,
            payload_size: 0,
            payload_sizes: vec![],
            udp_port: 9999,
            udp_throughput_port: 9998,
            udp_probes: 20,
            connection_reuse: false,
            dns_enabled: true,
            ipv4_only: false,
            ipv6_only: false,
            no_proxy: false,
            proxy: None,
            ca_bundle: None,
            insecure: true,
            retries: 0,
            output_dir: ".".into(),
            html_report: "report.html".into(),
            css: None,
            excel: false,
            save_to_db: false,
            db_url: None,
            db_migrate: false,
            save_to_sql: false,
            connection_string: None,
            log_level: None,
            page_asset_sizes: vec![],
            page_preset_name: None,
            http_stacks: vec![],
            packet_capture: ResolvedPacketCaptureConfig {
                mode: networker_tester::cli::PacketCaptureMode::None,
                install_requirements: false,
                interface: "auto".into(),
                write_pcap: false,
                write_summary_json: false,
            },
            impairment: ResolvedImpairmentConfig {
                profile: ImpairmentProfile::None,
                delay_ms,
            },
            json_stdout: false,
        }
    }

    #[test]
    fn impairment_target_rewrites_supported_http_family_probe() {
        let cfg = sample_resolved_config(150);
        let base = url::Url::parse("https://example.com:8443/health").unwrap();
        for proto in [
            Protocol::Http1,
            Protocol::Http2,
            Protocol::Http3,
            Protocol::Tcp,
            Protocol::Tls,
            Protocol::Native,
            Protocol::Curl,
        ] {
            let rewritten = apply_impairment_target(&proto, &base, &cfg);
            assert_eq!(rewritten.path(), "/delay");
            assert_eq!(rewritten.query(), Some("ms=150"));
            assert_eq!(rewritten.host_str(), Some("example.com"));
        }
    }

    #[test]
    fn impairment_target_skips_unsupported_probe_types() {
        let cfg = sample_resolved_config(150);
        let base = url::Url::parse("https://example.com:8443/health").unwrap();
        let rewritten = apply_impairment_target(&Protocol::Udp, &base, &cfg);
        assert_eq!(rewritten.path(), "/health");
        assert_eq!(rewritten.query(), None);
    }

    #[test]
    fn impairment_target_is_noop_when_delay_is_zero() {
        let cfg = sample_resolved_config(0);
        let base = url::Url::parse("https://example.com:8443/health").unwrap();
        let rewritten = apply_impairment_target(&Protocol::Http2, &base, &cfg);
        assert_eq!(rewritten, base);
    }
}
