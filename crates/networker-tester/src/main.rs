use anyhow::Context;
use chrono::Utc;
use clap::Parser;
use networker_tester::baseline::{
    baseline_from_environment_check, baseline_from_stability_check, fetch_server_info,
    measure_baseline, measure_environment_check, measure_stability_check,
    DEFAULT_ENVIRONMENT_CHECK_INTERVAL_MS, DEFAULT_ENVIRONMENT_CHECK_SAMPLES,
    DEFAULT_STABILITY_CHECK_INTERVAL_MS, DEFAULT_STABILITY_CHECK_SAMPLES,
};
use networker_tester::benchmark::{
    benchmark_adaptive_criteria, benchmark_adaptive_status, benchmark_pilot_criteria,
    derive_measured_plan_from_pilot, BenchmarkAdaptiveCriteria, BenchmarkAdaptiveStopReason,
    DEFAULT_COOLDOWN_SAMPLES, DEFAULT_OVERHEAD_SAMPLES,
};
use networker_tester::capture;
use networker_tester::cli;
use networker_tester::cli::ResolvedConfig;
use networker_tester::dispatch::{
    dispatch_once, log_attempt, published_logical_attempts, rewrite_url_for_stack,
};
use networker_tester::metrics::{
    primary_metric_value, BenchmarkExecutionPlan, BenchmarkNoiseThresholds, HostInfo, Protocol,
    RequestAttempt, TestRun,
};
use networker_tester::output;
use networker_tester::output::db;
use networker_tester::output::{excel, html, json};
use networker_tester::progress::ProgressReporter;
use networker_tester::runner::{
    http::RunConfig,
    pageload::{run_pageload2_warm, warmup_pageload2, PageLoadConfig, SharedH2Conn},
    throughput::ThroughputConfig,
    udp::UdpProbeConfig,
    udp_throughput::UdpThroughputConfig,
};
use networker_tester::summary::{copy_default_css, print_summary, print_url_test_summary};
use networker_tester::tls_profile::{
    run_tls_endpoint_profile, TlsProfileRequest, TlsProfileTargetKind,
};
use networker_tester::url_diagnostic::{
    UrlDiagnosticCapabilities, UrlDiagnosticOrchestrator, UrlDiagnosticRequest,
};
use std::path::PathBuf;
use tracing::{error, info, warn};
use uuid::Uuid;

#[cfg(feature = "http3")]
use networker_tester::runner::pageload::{run_pageload3_warm, warmup_pageload3};

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

    if cfg.url_test_url.is_some() {
        return run_url_test_cli(&cfg).await;
    }
    if cfg.tls_profile_url.is_some() {
        return run_tls_profile_cli(&cfg).await;
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
            "No valid modes specified. Use: tcp,http1,http2,http3,udp,dns,tls,tlsresume,native,curl,\
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

    // ── Progress reporter (benchmark orchestrator integration) ─────────────
    let progress_reporter: Option<std::sync::Arc<ProgressReporter>> =
        cfg.progress_url.as_ref().map(|url| {
            std::sync::Arc::new(ProgressReporter::new(
                format!(
                    "{}/api/benchmarks/callback/request-progress",
                    url.trim_end_matches('/')
                ),
                cfg.progress_token.clone().unwrap_or_default(),
                cfg.progress_config_id.clone().unwrap_or_default(),
                cfg.progress_testbed_id.clone(),
                cfg.benchmark_language
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                cfg.progress_interval,
            ))
        });

    // ── Run probes for every target ───────────────────────────────────────────
    let mut all_runs: Vec<TestRun> = Vec::new();
    for target_url_str in &cfg.targets {
        info!(target = %target_url_str, "Running probes for target");
        let run = run_for_target(
            target_url_str,
            &cfg,
            &modes,
            &payload_sizes,
            progress_reporter.clone(),
        )
        .await?;
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
        let result = if cfg.benchmark_mode {
            json::to_benchmark_string_many(&all_runs)
        } else if all_runs.len() == 1 {
            let first = all_runs
                .first()
                .context("no targets produced any test runs")?;
            serde_json::to_string(first).map_err(anyhow::Error::from)
        } else {
            serde_json::to_string(&all_runs).map_err(anyhow::Error::from)
        };

        match result {
            Ok(json) => println!("{json}"),
            Err(e) => {
                error!(error = %e, "failed to serialize JSON stdout payload");
                println!("{{\"error\":\"serialization failed\"}}");
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

fn make_url_test_capture_config(cfg: &ResolvedConfig) -> ResolvedConfig {
    let mut resolved = cfg.clone();
    resolved.targets = vec![cfg
        .url_test_url
        .clone()
        .unwrap_or_else(|| "https://example.com".into())];
    resolved
}

async fn run_tls_profile_cli(cfg: &ResolvedConfig) -> anyhow::Result<()> {
    let url = url::Url::parse(
        cfg.tls_profile_url
            .as_deref()
            .context("--tls-profile-url is required")?,
    )
    .context("parsing --tls-profile-url")?;
    let host = url
        .host_str()
        .context("--tls-profile-url must include a host")?;
    let port = url.port_or_known_default().unwrap_or(443);
    let target_kind = match cfg
        .tls_profile_target_kind
        .as_deref()
        .unwrap_or("external-url")
        .replace('_', "-")
        .as_str()
    {
        "managed-endpoint" => TlsProfileTargetKind::ManagedEndpoint,
        "external-host" => TlsProfileTargetKind::ExternalHost,
        _ => TlsProfileTargetKind::ExternalUrl,
    };
    let req = TlsProfileRequest {
        target_kind,
        source_url: Some(url.to_string()),
        host: host.to_string(),
        port,
        ip_override: cfg
            .tls_profile_ip
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("invalid --tls-profile-ip")?,
        sni_override: cfg.tls_profile_sni.clone(),
        dns_enabled: cfg.dns_enabled,
        ipv4_only: cfg.ipv4_only,
        ipv6_only: cfg.ipv6_only,
        insecure: cfg.insecure,
        ca_bundle: cfg.ca_bundle.clone(),
        timeout_ms: cfg.timeout.saturating_mul(1000).max(1000),
    };

    let profile = run_tls_endpoint_profile(req).await?;
    let tls_profile_project_id = cfg
        .tls_profile_project_id
        .as_deref()
        .map(str::parse::<uuid::Uuid>)
        .transpose()
        .context("invalid --tls-profile-project-id")?;
    let out_dir = PathBuf::from(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir).context("Cannot create output directory")?;
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let json_path = out_dir.join(format!("tls-profile-{ts}.json"));
    json::save_tls_profile(&profile, &json_path)?;

    if cfg.save_to_db || cfg.save_to_sql {
        let db_url = cfg
            .db_url
            .as_deref()
            .or(cfg.connection_string.as_deref())
            .context(
            "--save-to-db requires --db-url (or legacy --connection-string) for TLS profile runs",
        )?;
        let backend = db::connect(db_url).await?;
        if cfg.db_migrate {
            backend.migrate().await?;
        }
        backend
            .save_tls_profile(&profile, tls_profile_project_id.as_ref())
            .await?;
    }

    if cfg.tls_profile_json {
        println!("{}", json::to_string_tls_profile(&profile)?);
    } else {
        println!("TLS Endpoint Profile");
        println!("--------------------");
        println!("Host: {}:{}", profile.target.host, profile.target.port);
        println!("Status: {}", profile.summary.status);
        println!("JSON: {}", json_path.display());
    }
    Ok(())
}

async fn run_url_test_cli(cfg: &ResolvedConfig) -> anyhow::Result<()> {
    let url = cfg
        .url_test_url
        .clone()
        .context("--url-test-url is required for URL diagnostic mode")?;

    let headers: Vec<(String, String)> = cfg
        .url_test_headers
        .iter()
        .map(|h| {
            let (name, value) = h.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("invalid --url-test-header (expected 'Name: value')")
            })?;
            let name = name.trim();
            let value = value.trim();
            if name.is_empty()
                || name.contains(['\r', '\n'])
                || value.contains(['\r', '\n'])
            {
                anyhow::bail!("invalid --url-test-header (header names/values must not contain CR/LF and name must be non-empty)");
            }
            Ok((name.to_string(), value.to_string()))
        })
        .collect::<anyhow::Result<_>>()?;

    let request = UrlDiagnosticRequest {
        url,
        auth_token: cfg.url_test_auth_token.clone(),
        cookie: cfg.url_test_cookie.clone(),
        headers,
        timeout_ms: Some(cfg.timeout.saturating_mul(1000)),
        follow_redirects: true,
        capture_pcap: cfg.url_test_capture_pcap,
        capture_har: cfg.url_test_capture_har,
        protocol_force: cfg.url_test_protocol_force.clone(),
        http3_repeat_count: cfg.url_test_http3_repeat,
        ignore_tls_validation: cfg.insecure,
        user_agent: None,
        browser_engine: None,
        network_idle_timeout_ms: None,
        artifact_output_dir: Some(cfg.output_dir.clone()),
    };

    let capabilities = UrlDiagnosticOrchestrator::detect_capabilities();
    let orchestrator = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities {
        protocol_probe_available: false,
        ..capabilities
    });

    let out_dir = PathBuf::from(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir).context("Cannot create output directory")?;

    let capture_cfg = make_url_test_capture_config(cfg);
    let capture_plan = if cfg.url_test_capture_pcap {
        capture::build_plan(&capture_cfg, &out_dir)
    } else {
        None
    };

    let mut capture_session = match capture_plan {
        Some(plan) => match capture::start(plan).await {
            Ok(session) => Some(session),
            Err(e) => {
                tracing::warn!("URL diagnostic packet capture unavailable: {e}");
                None
            }
        },
        None => None,
    };

    let plan = orchestrator.plan(request)?;
    let mut run = orchestrator.execute_primary_page_diagnostic(plan).await?;

    if let Some(session) = capture_session.take() {
        match session.finalize().await {
            Ok(Some(summary)) => {
                run.pcap_path = Some(summary.capture_path.clone());
                run.pcap_summary = Some(networker_tester::metrics::UrlPacketCaptureSummary {
                    mode: summary.mode.clone(),
                    interface: summary.interface.clone(),
                    capture_path: summary.capture_path.clone(),
                    total_packets: summary.total_packets,
                    capture_status: summary.capture_status.clone(),
                    note: summary.note.clone(),
                    warnings: summary.warnings.clone(),
                    tcp_packets: summary.tcp_packets,
                    udp_packets: summary.udp_packets,
                    quic_packets: summary.quic_packets,
                    http_packets: summary.http_packets,
                    dns_packets: summary.dns_packets,
                    retransmissions: summary.retransmissions,
                    duplicate_acks: summary.duplicate_acks,
                    resets: summary.resets,
                    transport_shares: summary.transport_shares.clone(),
                    top_endpoints: summary.top_endpoints.clone(),
                    top_ports: summary.top_ports.clone(),
                    observed_quic: summary.observed_quic,
                    observed_tcp_only: summary.observed_tcp_only,
                    observed_mixed_transport: summary.observed_mixed_transport,
                    capture_may_be_ambiguous: summary.capture_may_be_ambiguous,
                });
                if !summary.warnings.is_empty() {
                    run.capture_errors.extend(summary.warnings.clone());
                }
                if summary.capture_status != "captured" {
                    run.capture_errors.push(format!(
                        "pcap capture status: {}{}",
                        summary.capture_status,
                        summary
                            .note
                            .as_deref()
                            .map(|n| format!(" ({n})"))
                            .unwrap_or_default()
                    ));
                }
            }
            Ok(None) => {
                run.capture_errors
                    .push("pcap capture completed without summary output".into());
            }
            Err(e) => {
                run.capture_errors
                    .push(format!("pcap finalize failed: {e}"));
            }
        }
    }

    let ts = run.started_at.format("%Y%m%d-%H%M%S");
    if run.har_path.is_some() {
        let src = PathBuf::from(run.har_path.as_deref().unwrap_or_default());
        if src.exists() {
            let dst = out_dir.join(src.file_name().unwrap_or_default());
            if src != dst {
                std::fs::copy(&src, &dst)
                    .context("Failed to copy HAR artifact to output directory")?;
                run.har_path = Some(dst.display().to_string());
            }
        }
    }

    let json_path = out_dir.join(format!("url-test-{ts}.json"));
    json::save_url_test(&run, &json_path)
        .context("Failed to write URL diagnostic JSON artifact")?;

    if cfg.save_to_db || cfg.save_to_sql {
        let db_url = cfg
            .db_url
            .as_deref()
            .or(cfg.connection_string.as_deref())
            .context(
            "--save-to-db requires --db-url (or legacy --connection-string) for URL diagnostics",
        )?;
        let backend = db::connect(db_url).await?;
        if cfg.db_migrate {
            backend.migrate().await?;
        }
        backend.save_url_test(&run).await?;
    }

    if cfg.url_test_json {
        println!("{}", json::to_string_url_test(&run)?);
    } else {
        print_url_test_summary(&run, &json_path);
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
    progress_reporter: Option<std::sync::Arc<ProgressReporter>>,
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

    let benchmark_environment_check = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        let samples = cfg
            .benchmark_environment_check_samples
            .unwrap_or(DEFAULT_ENVIRONMENT_CHECK_SAMPLES);
        let interval_ms = cfg
            .benchmark_environment_check_interval_ms
            .unwrap_or(DEFAULT_ENVIRONMENT_CHECK_INTERVAL_MS);
        let environment_check = measure_environment_check(&target, samples, interval_ms).await;
        match environment_check.as_ref() {
            Some(check) if check.successful_samples > 0 => {
                info!(
                    "Environment-check: {} | RTT avg={:.2}ms min={:.2}ms max={:.2}ms p50={:.2}ms p95={:.2}ms loss={:.1}% ({} / {} samples)",
                    check.network_type,
                    check.rtt_avg_ms,
                    check.rtt_min_ms,
                    check.rtt_max_ms,
                    check.rtt_p50_ms,
                    check.rtt_p95_ms,
                    check.packet_loss_percent,
                    check.successful_samples,
                    check.attempted_samples,
                );
            }
            Some(check) => {
                warn!(
                    "Environment-check: {} | RTT probes failed (loss={:.1}% across {} attempts)",
                    check.network_type, check.packet_loss_percent, check.attempted_samples,
                );
            }
            None => warn!("Could not perform benchmark environment-check"),
        }
        environment_check
    } else {
        None
    };

    let benchmark_stability_check = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        let samples = cfg
            .benchmark_stability_check_samples
            .unwrap_or(DEFAULT_STABILITY_CHECK_SAMPLES);
        let interval_ms = cfg
            .benchmark_stability_check_interval_ms
            .unwrap_or(DEFAULT_STABILITY_CHECK_INTERVAL_MS);
        let stability = measure_stability_check(&target, samples, interval_ms).await;
        match stability.as_ref() {
            Some(check) if check.successful_samples > 0 => {
                info!(
                    "Stability-check: {} | RTT avg={:.2}ms min={:.2}ms max={:.2}ms p50={:.2}ms p95={:.2}ms jitter={:.2}ms loss={:.1}% ({} / {} samples)",
                    check.network_type,
                    check.rtt_avg_ms,
                    check.rtt_min_ms,
                    check.rtt_max_ms,
                    check.rtt_p50_ms,
                    check.rtt_p95_ms,
                    check.jitter_ms,
                    check.packet_loss_percent,
                    check.successful_samples,
                    check.attempted_samples,
                );
            }
            Some(check) => {
                warn!(
                    "Stability-check: {} | RTT probes failed (loss={:.1}% across {} attempts)",
                    check.network_type, check.packet_loss_percent, check.attempted_samples,
                );
            }
            None => warn!("Could not perform benchmark stability-check"),
        }
        stability
    } else {
        None
    };

    // ── Measure network baseline RTT ────────────────────────────────────────
    let baseline = if let Some(environment_check) = benchmark_environment_check.as_ref() {
        Some(baseline_from_environment_check(environment_check))
    } else if let Some(stability_check) = benchmark_stability_check.as_ref() {
        Some(baseline_from_stability_check(stability_check))
    } else {
        match measure_baseline(&target).await {
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
    #[cfg(feature = "http3")]
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

    let connection_reuse_warmup_attempt_count = all_attempts.len() as u32;
    let collect_iteration = |seq: &mut u32| {
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
                let current_seq = *seq;
                *seq += 1;

                async move {
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

                    let mut attempts = Vec::new();
                    let first_attempt = do_dispatch!();
                    attempts.push(first_attempt);

                    for retry_num in 1..=retries {
                        if attempts.last().is_some_and(|attempt| attempt.success) {
                            break;
                        }
                        let mut retry_attempt = do_dispatch!();
                        retry_attempt.retry_count = retry_num;
                        attempts.push(retry_attempt);
                    }

                    for attempt in &attempts {
                        log_attempt(attempt);
                    }

                    published_logical_attempts(attempts)
                }
            })
            .collect();

        async move {
            use futures::stream::{self, StreamExt};
            let results: Vec<Vec<RequestAttempt>> = stream::iter(futures)
                .buffer_unordered(cfg.concurrency)
                .collect()
                .await;
            results.into_iter().flatten().collect::<Vec<_>>()
        }
    };

    let pilot_criteria = benchmark_pilot_criteria(cfg);
    let pilot_adaptive_criteria = pilot_criteria.map(|criteria| BenchmarkAdaptiveCriteria {
        min_samples: criteria.min_samples,
        max_samples: criteria.max_samples,
        min_duration_ms: criteria.min_duration_ms,
        target_relative_error: None,
        target_absolute_error: None,
    });
    let overhead_runs = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        cfg.benchmark_overhead_samples
            .unwrap_or(DEFAULT_OVERHEAD_SAMPLES)
    } else {
        0
    };
    let cooldown_runs = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        cfg.benchmark_cooldown_samples
            .unwrap_or(DEFAULT_COOLDOWN_SAMPLES)
    } else {
        0
    };
    let mut overhead_attempts = Vec::new();
    let mut pilot_attempts = Vec::new();
    let mut pilot_completed_runs = 0u32;

    if overhead_runs > 0 {
        info!(samples = overhead_runs, "Benchmark overhead phase enabled");
        for overhead_index in 0..overhead_runs {
            info!("Overhead {}/{}", overhead_index + 1, overhead_runs);
            let published_results = collect_iteration(&mut seq).await;
            overhead_attempts.extend(published_results.iter().cloned());
            all_attempts.extend(published_results);
        }
    }

    if let Some(criteria) = pilot_adaptive_criteria {
        info!(
            min_samples = criteria.min_samples,
            max_samples = criteria.max_samples,
            min_duration_ms = criteria.min_duration_ms,
            "Pilot benchmark plan enabled"
        );
        while pilot_completed_runs < criteria.max_samples {
            info!(
                "Pilot {}/{}",
                pilot_completed_runs + 1,
                criteria.max_samples
            );
            let published_results = collect_iteration(&mut seq).await;
            pilot_attempts.extend(published_results.iter().cloned());
            all_attempts.extend(published_results);
            pilot_completed_runs += 1;

            let status = benchmark_adaptive_status(&criteria, &pilot_attempts);
            match status.stop_reason {
                Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Pilot benchmark sample budget satisfied"
                    );
                    break;
                }
                Some(BenchmarkAdaptiveStopReason::MaxSamplesReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Pilot benchmark reached maximum sample budget"
                    );
                    break;
                }
                None => {}
            }
        }
    }

    let benchmark_execution_plan = if !pilot_attempts.is_empty() {
        Some(derive_measured_plan_from_pilot(cfg, &pilot_attempts))
    } else if let Some(criteria) = benchmark_adaptive_criteria(cfg) {
        Some(BenchmarkExecutionPlan {
            source: "explicit".into(),
            min_samples: criteria.min_samples,
            max_samples: criteria.max_samples,
            min_duration_ms: criteria.min_duration_ms,
            target_relative_error: criteria.target_relative_error,
            target_absolute_error: criteria.target_absolute_error,
            pilot_sample_count: 0,
            pilot_elapsed_ms: None,
        })
    } else if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        Some(BenchmarkExecutionPlan {
            source: "fixed-count".into(),
            min_samples: cfg.runs,
            max_samples: cfg.runs,
            min_duration_ms: 0,
            target_relative_error: None,
            target_absolute_error: None,
            pilot_sample_count: 0,
            pilot_elapsed_ms: None,
        })
    } else {
        None
    };
    let adaptive_criteria = benchmark_execution_plan
        .as_ref()
        .filter(|_| cfg.benchmark_mode && cfg.benchmark_phase == "measured")
        .map(|plan| BenchmarkAdaptiveCriteria {
            min_samples: plan.min_samples,
            max_samples: plan.max_samples,
            min_duration_ms: plan.min_duration_ms,
            target_relative_error: plan.target_relative_error,
            target_absolute_error: plan.target_absolute_error,
        });
    let max_run_count = adaptive_criteria.map_or(cfg.runs, |criteria| criteria.max_samples);
    if let Some(plan) = &benchmark_execution_plan {
        info!(
            source = %plan.source,
            min_samples = plan.min_samples,
            max_samples = plan.max_samples,
            min_duration_ms = plan.min_duration_ms,
            target_relative_error = plan.target_relative_error,
            target_absolute_error = plan.target_absolute_error,
            pilot_sample_count = plan.pilot_sample_count,
            "Benchmark measured execution plan resolved"
        );
    }
    let mut measured_attempts = Vec::new();
    let mut cooldown_attempts = Vec::new();
    let mut completed_runs = 0u32;
    let mut progress_request_counter = 0u32;
    let total_estimated_requests = max_run_count.saturating_mul(mode_tasks.len() as u32);

    while completed_runs < max_run_count {
        info!("Run {}/{}", completed_runs + 1, max_run_count);
        let published_results = collect_iteration(&mut seq).await;

        // Report progress for each measured attempt
        if let Some(ref reporter) = progress_reporter {
            for attempt in &published_results {
                progress_request_counter += 1;
                let r = reporter.clone();
                let mode = attempt.protocol.to_string();
                let lat = primary_metric_value(attempt).unwrap_or(0.0);
                let ok = attempt.success;
                let idx = progress_request_counter;
                let total = total_estimated_requests;
                tokio::spawn(async move {
                    r.report(&mode, idx, total, lat, ok).await;
                });
            }
        }

        measured_attempts.extend(published_results.iter().cloned());
        all_attempts.extend(published_results);
        completed_runs += 1;

        if let Some(criteria) = adaptive_criteria {
            let status = benchmark_adaptive_status(&criteria, &measured_attempts);
            match status.stop_reason {
                Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Adaptive benchmark stop criteria satisfied"
                    );
                    break;
                }
                Some(BenchmarkAdaptiveStopReason::MaxSamplesReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Adaptive benchmark reached maximum sample budget"
                    );
                    break;
                }
                None => {}
            }
        }
    }

    if cooldown_runs > 0 {
        info!(samples = cooldown_runs, "Benchmark cooldown phase enabled");
        for cooldown_index in 0..cooldown_runs {
            info!("Cooldown {}/{}", cooldown_index + 1, cooldown_runs);
            let published_results = collect_iteration(&mut seq).await;
            cooldown_attempts.extend(published_results.iter().cloned());
            all_attempts.extend(published_results);
        }
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
                    let mut attempts = Vec::new();
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
                    attempts.push(attempt);

                    // Retry loop
                    for retry_num in 1..=retries {
                        if attempts.last().is_some_and(|candidate| candidate.success) {
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
                        attempts.push(retry_a);
                    }

                    for attempt in &attempts {
                        log_attempt(attempt);
                    }

                    // Report progress for HTTP stack attempts
                    if let Some(ref reporter) = progress_reporter {
                        for attempt in &attempts {
                            progress_request_counter += 1;
                            let r = reporter.clone();
                            let mode = format!("{}:{}", attempt.protocol, stack.name);
                            let lat = primary_metric_value(attempt).unwrap_or(0.0);
                            let ok = attempt.success;
                            let idx = progress_request_counter;
                            let total = total_estimated_requests;
                            tokio::spawn(async move {
                                r.report(&mode, idx, total, lat, ok).await;
                            });
                        }
                    }

                    all_attempts.extend(published_logical_attempts(attempts));
                }
            }
        }
    }

    let finished_at = Utc::now();

    let benchmark_noise_thresholds = cfg.benchmark_mode.then(|| BenchmarkNoiseThresholds {
        max_packet_loss_percent: cfg
            .benchmark_max_packet_loss_percent
            .unwrap_or(BenchmarkNoiseThresholds::default().max_packet_loss_percent),
        max_jitter_ratio: cfg
            .benchmark_max_jitter_ratio
            .unwrap_or(BenchmarkNoiseThresholds::default().max_jitter_ratio),
        max_rtt_spread_ratio: cfg
            .benchmark_max_rtt_spread_ratio
            .unwrap_or(BenchmarkNoiseThresholds::default().max_rtt_spread_ratio),
    });

    let run = TestRun {
        run_id,
        started_at,
        finished_at: Some(finished_at),
        target_url: target_url_str.to_string(),
        target_host,
        modes: modes.iter().map(|m| m.to_string()).collect(),
        total_runs: completed_runs,
        concurrency: cfg.concurrency as u32,
        timeout_ms: cfg.timeout * 1000,
        client_os: std::env::consts::OS.to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        server_info,
        client_info,
        baseline,
        packet_capture_summary: None,
        benchmark_environment_check,
        benchmark_stability_check,
        benchmark_phase: cfg.benchmark_mode.then(|| cfg.benchmark_phase.clone()),
        benchmark_scenario: cfg.benchmark_mode.then(|| cfg.benchmark_scenario.clone()),
        benchmark_launch_index: cfg.benchmark_mode.then_some(cfg.benchmark_launch_index),
        benchmark_warmup_attempt_count: if cfg.benchmark_mode {
            connection_reuse_warmup_attempt_count
        } else {
            0
        },
        benchmark_pilot_attempt_count: if cfg.benchmark_mode {
            pilot_attempts.len().min(u32::MAX as usize) as u32
        } else {
            0
        },
        benchmark_overhead_attempt_count: if cfg.benchmark_mode {
            overhead_attempts.len().min(u32::MAX as usize) as u32
        } else {
            0
        },
        benchmark_cooldown_attempt_count: if cfg.benchmark_mode {
            cooldown_attempts.len().min(u32::MAX as usize) as u32
        } else {
            0
        },
        benchmark_execution_plan: cfg
            .benchmark_mode
            .then_some(benchmark_execution_plan)
            .flatten(),
        benchmark_noise_thresholds,
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


#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use networker_tester::baseline::{
        average_jitter_ms, baseline_from_environment_check, baseline_from_stability_check,
        classify_ip, classify_target, percentile,
    };
    use networker_tester::dispatch::apply_impairment_target;
    use networker_tester::benchmark::{
        adaptive_case_id, benchmark_adaptive_criteria, benchmark_adaptive_status,
        benchmark_attempt_wall_time_ms, benchmark_pilot_criteria, bootstrap_median_interval,
        derive_measured_plan_from_pilot, estimated_samples_for_error_targets, median_error_bounds,
        median_from_sorted, percentile_from_sorted, BenchmarkAdaptiveCriteria,
        BenchmarkAdaptiveStopReason, DeterministicRng, DEFAULT_AUTO_TARGET_RELATIVE_ERROR,
    };
    use networker_tester::cli::{
        ImpairmentProfile, ResolvedImpairmentConfig, ResolvedPacketCaptureConfig,
    };
    use networker_tester::metrics::{
        BenchmarkEnvironmentCheck, BenchmarkStabilityCheck, HttpResult, NetworkType,
    };
    use networker_tester::summary::fmt_bytes;
    use uuid::Uuid;

    fn request_attempt(success: bool, retry_count: u32) -> RequestAttempt {
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Http1,
            sequence_num: 0,
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            success,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        }
    }

    fn measured_http_attempt(
        value_ms: f64,
        start_offset_ms: i64,
        wall_time_ms: i64,
        payload_bytes: usize,
        stack: Option<&str>,
    ) -> RequestAttempt {
        let started_at = Utc.timestamp_millis_opt(start_offset_ms).single().unwrap();
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Http1,
            sequence_num: start_offset_ms.max(0) as u32,
            started_at,
            finished_at: Some(started_at + Duration::milliseconds(wall_time_ms)),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code: 200,
                headers_size_bytes: 128,
                body_size_bytes: payload_bytes,
                ttfb_ms: value_ms / 2.0,
                total_duration_ms: value_ms,
                redirect_count: 0,
                started_at,
                response_headers: Vec::new(),
                payload_bytes,
                throughput_mbps: None,
                goodput_mbps: None,
                cpu_time_ms: None,
                csw_voluntary: None,
                csw_involuntary: None,
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: stack.map(str::to_string),
        }
    }

    fn failed_http_attempt(
        start_offset_ms: i64,
        wall_time_ms: i64,
        _payload_bytes: usize,
        stack: Option<&str>,
    ) -> RequestAttempt {
        let started_at = Utc.timestamp_millis_opt(start_offset_ms).single().unwrap();
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Http1,
            sequence_num: start_offset_ms.max(0) as u32,
            started_at,
            finished_at: Some(started_at + Duration::milliseconds(wall_time_ms)),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: stack.map(str::to_string),
        }
    }

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

    #[test]
    fn published_logical_attempts_keep_only_final_retry_outcome() {
        let published =
            published_logical_attempts(vec![request_attempt(false, 0), request_attempt(true, 1)]);

        assert_eq!(published.len(), 1);
        assert!(published[0].success);
        assert_eq!(published[0].retry_count, 1);
    }

    #[test]
    fn benchmark_adaptive_criteria_requires_controls() {
        let mut cfg = sample_resolved_config(0);
        cfg.benchmark_mode = true;

        assert!(benchmark_adaptive_criteria(&cfg).is_none());

        cfg.benchmark_min_samples = Some(5);
        let criteria = benchmark_adaptive_criteria(&cfg).unwrap();
        assert_eq!(criteria.min_samples, 5);
        assert_eq!(criteria.max_samples, 1);
        assert_eq!(criteria.min_duration_ms, 0);
        assert_eq!(criteria.target_relative_error, None);
        assert_eq!(criteria.target_absolute_error, None);
    }

    #[test]
    fn benchmark_pilot_criteria_defaults_for_measured_benchmark_mode() {
        let mut cfg = sample_resolved_config(0);
        cfg.benchmark_mode = true;
        cfg.runs = 10;

        let criteria = benchmark_pilot_criteria(&cfg).unwrap();
        assert_eq!(criteria.min_samples, 6);
        assert_eq!(criteria.max_samples, 10);
        assert_eq!(criteria.min_duration_ms, 0);
    }

    #[test]
    fn benchmark_pilot_criteria_stays_disabled_when_explicit_measured_controls_exist() {
        let mut cfg = sample_resolved_config(0);
        cfg.benchmark_mode = true;
        cfg.benchmark_min_samples = Some(5);

        assert!(benchmark_pilot_criteria(&cfg).is_none());
    }

    #[test]
    fn benchmark_adaptive_status_stops_when_accuracy_target_is_met() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 3,
            max_samples: 10,
            min_duration_ms: 20,
            target_relative_error: Some(0.05),
            target_absolute_error: None,
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 10, 1024, None),
            measured_http_attempt(100.0, 10, 10, 1024, None),
            measured_http_attempt(100.0, 20, 10, 1024, None),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(status.completed_samples, 3);
        assert_eq!(
            status.stop_reason,
            Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached)
        );
    }

    #[test]
    fn benchmark_adaptive_status_waits_for_min_duration() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 3,
            max_samples: 10,
            min_duration_ms: 100,
            target_relative_error: Some(0.05),
            target_absolute_error: None,
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 10, 1024, None),
            measured_http_attempt(100.0, 10, 10, 1024, None),
            measured_http_attempt(100.0, 20, 10, 1024, None),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(status.completed_samples, 3);
        assert!(status.elapsed_ms < 100.0);
        assert_eq!(status.stop_reason, None);
    }

    #[test]
    fn benchmark_adaptive_status_uses_lowest_case_sample_count() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 3,
            max_samples: 10,
            min_duration_ms: 0,
            target_relative_error: None,
            target_absolute_error: None,
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 5, 1024, None),
            measured_http_attempt(100.0, 10, 5, 1024, None),
            measured_http_attempt(100.0, 20, 5, 1024, None),
            measured_http_attempt(101.0, 0, 5, 1024, Some("nginx")),
            measured_http_attempt(101.0, 10, 5, 1024, Some("nginx")),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(status.completed_samples, 2);
        assert_eq!(status.stop_reason, None);
    }

    #[test]
    fn benchmark_adaptive_status_requires_metrics_for_every_case() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 1,
            max_samples: 5,
            min_duration_ms: 0,
            target_relative_error: Some(0.05),
            target_absolute_error: None,
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 10, 1024, None),
            failed_http_attempt(0, 10, 1024, Some("nginx")),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(status.completed_samples, 1);
        assert_eq!(status.stop_reason, None);
    }

    #[test]
    fn benchmark_adaptive_status_stops_at_max_samples_when_noise_stays_high() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 2,
            max_samples: 4,
            min_duration_ms: 0,
            target_relative_error: Some(0.01),
            target_absolute_error: None,
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 5, 1024, None),
            measured_http_attempt(200.0, 10, 5, 1024, None),
            measured_http_attempt(50.0, 20, 5, 1024, None),
            measured_http_attempt(300.0, 30, 5, 1024, None),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(status.completed_samples, 4);
        assert_eq!(
            status.stop_reason,
            Some(BenchmarkAdaptiveStopReason::MaxSamplesReached)
        );
    }

    #[test]
    fn derive_measured_plan_from_pilot_estimates_targets() {
        let mut cfg = sample_resolved_config(0);
        cfg.benchmark_mode = true;
        cfg.runs = 40;
        let attempts = vec![
            measured_http_attempt(100.0, 0, 10, 1024, None),
            measured_http_attempt(100.0, 10, 10, 1024, None),
            measured_http_attempt(100.0, 20, 10, 1024, None),
            measured_http_attempt(100.0, 30, 10, 1024, None),
            measured_http_attempt(100.0, 40, 10, 1024, None),
            measured_http_attempt(100.0, 50, 10, 1024, None),
        ];

        let plan = derive_measured_plan_from_pilot(&cfg, &attempts);

        assert_eq!(plan.source, "pilot-derived");
        assert_eq!(plan.pilot_sample_count, 6);
        assert_eq!(
            plan.target_relative_error,
            Some(DEFAULT_AUTO_TARGET_RELATIVE_ERROR)
        );
        assert_eq!(plan.target_absolute_error, None);
        assert_eq!(plan.min_samples, 6);
        assert_eq!(plan.max_samples, 6);
        assert!(plan.pilot_elapsed_ms.is_some());
    }

    #[test]
    fn benchmark_adaptive_status_stops_when_absolute_error_target_is_met() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 3,
            max_samples: 10,
            min_duration_ms: 0,
            target_relative_error: None,
            target_absolute_error: Some(1.0),
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 10, 1024, None),
            measured_http_attempt(100.0, 10, 10, 1024, None),
            measured_http_attempt(100.0, 20, 10, 1024, None),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(
            status.stop_reason,
            Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached)
        );
    }

    #[test]
    fn benchmark_adaptive_status_requires_all_configured_error_targets() {
        let criteria = BenchmarkAdaptiveCriteria {
            min_samples: 3,
            max_samples: 10,
            min_duration_ms: 0,
            target_relative_error: Some(0.01),
            target_absolute_error: Some(100.0),
        };
        let attempts = vec![
            measured_http_attempt(100.0, 0, 10, 1024, None),
            measured_http_attempt(110.0, 10, 10, 1024, None),
            measured_http_attempt(90.0, 20, 10, 1024, None),
        ];

        let status = benchmark_adaptive_status(&criteria, &attempts);

        assert_eq!(status.stop_reason, None);
    }

    fn sample_resolved_config(delay_ms: u64) -> ResolvedConfig {
        ResolvedConfig {
            targets: vec!["https://127.0.0.1:8443/health".into()],
            url_test_url: None,
            tls_profile_url: None,
            tls_profile_ip: None,
            tls_profile_sni: None,
            tls_profile_target_kind: None,
            tls_profile_json: false,
            tls_profile_project_id: None,
            url_test_auth_token: None,
            url_test_cookie: None,
            url_test_headers: vec![],
            url_test_capture_har: false,
            url_test_capture_pcap: false,
            url_test_protocol_force: None,
            url_test_http3_repeat: 10,
            url_test_json: false,
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
            benchmark_mode: false,
            benchmark_phase: "measured".into(),
            benchmark_scenario: "default".into(),
            benchmark_launch_index: 0,
            benchmark_min_samples: None,
            benchmark_max_samples: None,
            benchmark_min_duration_ms: None,
            benchmark_target_relative_error: None,
            benchmark_target_absolute_error: None,
            benchmark_pilot_min_samples: None,
            benchmark_pilot_max_samples: None,
            benchmark_pilot_min_duration_ms: None,
            benchmark_environment_check_samples: None,
            benchmark_environment_check_interval_ms: None,
            benchmark_stability_check_samples: None,
            benchmark_stability_check_interval_ms: None,
            benchmark_max_packet_loss_percent: None,
            benchmark_max_jitter_ratio: None,
            benchmark_max_rtt_spread_ratio: None,
            benchmark_overhead_samples: None,
            benchmark_cooldown_samples: None,
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
            progress_url: None,
            progress_token: None,
            progress_interval: 1,
            progress_config_id: None,
            progress_testbed_id: None,
            benchmark_language: None,
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

    #[test]
    fn average_jitter_uses_adjacent_rtt_deltas() {
        let jitter = average_jitter_ms(&[1.0, 1.4, 0.8, 1.1]);
        assert!((jitter - 0.433_333_333_333_333_35).abs() < 1e-12);
    }

    #[test]
    fn baseline_from_stability_check_reuses_rtt_distribution() {
        let stability = BenchmarkStabilityCheck {
            attempted_samples: 12,
            successful_samples: 10,
            failed_samples: 2,
            duration_ms: 500.0,
            rtt_min_ms: 0.8,
            rtt_avg_ms: 1.1,
            rtt_max_ms: 1.6,
            rtt_p50_ms: 1.0,
            rtt_p95_ms: 1.4,
            jitter_ms: 0.1,
            packet_loss_percent: 16.7,
            network_type: NetworkType::Loopback,
        };

        let baseline = baseline_from_stability_check(&stability);
        assert_eq!(baseline.samples, 10);
        assert_eq!(baseline.rtt_min_ms, 0.8);
        assert_eq!(baseline.rtt_avg_ms, 1.1);
        assert_eq!(baseline.rtt_max_ms, 1.6);
        assert_eq!(baseline.rtt_p50_ms, 1.0);
        assert_eq!(baseline.rtt_p95_ms, 1.4);
        assert_eq!(baseline.network_type, NetworkType::Loopback);
    }

    // ─── percentile / median / bootstrap ────────────────────────────────

    #[test]
    fn percentile_empty_returns_zero() {
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn percentile_single_element() {
        assert_eq!(percentile(&[42.0], 0.0), 42.0);
        assert_eq!(percentile(&[42.0], 50.0), 42.0);
        assert_eq!(percentile(&[42.0], 100.0), 42.0);
    }

    #[test]
    fn percentile_interpolates_correctly() {
        let sorted = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&sorted, 0.0), 1.0);
        assert_eq!(percentile(&sorted, 100.0), 5.0);
        assert_eq!(percentile(&sorted, 50.0), 3.0);
        // p25 = index 1.0 → value 2.0
        assert!((percentile(&sorted, 25.0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn percentile_from_sorted_matches_percentile() {
        let sorted = [10.0, 20.0, 30.0, 40.0, 50.0];
        for p in [0.0, 25.0, 50.0, 75.0, 100.0] {
            assert!(
                (percentile(&sorted, p) - percentile_from_sorted(&sorted, p)).abs() < 1e-12,
                "mismatch at p={p}"
            );
        }
    }

    #[test]
    fn median_from_sorted_odd_length() {
        assert_eq!(median_from_sorted(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(median_from_sorted(&[5.0]), 5.0);
    }

    #[test]
    fn median_from_sorted_even_length() {
        assert_eq!(median_from_sorted(&[1.0, 3.0]), 2.0);
        assert_eq!(median_from_sorted(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    }

    #[test]
    fn median_from_sorted_empty() {
        assert_eq!(median_from_sorted(&[]), 0.0);
    }

    #[test]
    fn bootstrap_median_interval_empty() {
        assert_eq!(bootstrap_median_interval(&[]), (0.0, 0.0, 0.0));
    }

    #[test]
    fn bootstrap_median_interval_single() {
        let (se, lo, hi) = bootstrap_median_interval(&[42.0]);
        assert_eq!(se, 0.0);
        assert_eq!(lo, 42.0);
        assert_eq!(hi, 42.0);
    }

    #[test]
    fn bootstrap_median_interval_identical_values() {
        let (se, lo, hi) = bootstrap_median_interval(&[7.0, 7.0, 7.0, 7.0]);
        assert!(se.abs() < 1e-12);
        assert!((lo - 7.0).abs() < 1e-12);
        assert!((hi - 7.0).abs() < 1e-12);
    }

    #[test]
    fn bootstrap_median_interval_is_deterministic() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let a = bootstrap_median_interval(&values);
        let b = bootstrap_median_interval(&values);
        assert_eq!(a, b);
    }

    #[test]
    fn median_error_bounds_too_few_values() {
        assert!(median_error_bounds(&[]).is_none());
        assert!(median_error_bounds(&[1.0]).is_none());
    }

    #[test]
    fn median_error_bounds_filters_nan() {
        assert!(median_error_bounds(&[f64::NAN, f64::NAN]).is_none());
    }

    #[test]
    fn median_error_bounds_returns_some_for_valid_data() {
        let bounds = median_error_bounds(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
        assert!((bounds.median - 3.0).abs() < 1e-12);
        assert!(bounds.absolute_half_width >= 0.0);
    }

    // ─── estimated_samples_for_error_targets ─────────────────────────────

    #[test]
    fn estimated_samples_returns_current_n_when_no_bounds() {
        // Single value → median_error_bounds returns None → returns current_n
        assert_eq!(estimated_samples_for_error_targets(&[1.0], None, None), 1);
    }

    #[test]
    fn estimated_samples_increases_for_tight_relative_target() {
        let values = [100.0, 110.0, 90.0, 105.0, 95.0, 100.0, 98.0, 102.0];
        let loose = estimated_samples_for_error_targets(&values, Some(0.10), None);
        let tight = estimated_samples_for_error_targets(&values, Some(0.01), None);
        assert!(tight >= loose, "tighter target should need more samples");
    }

    // ─── fmt_bytes ───────────────────────────────────────────────────────

    #[test]
    fn fmt_bytes_displays_bytes() {
        assert_eq!(fmt_bytes(0), "0B");
        assert_eq!(fmt_bytes(512), "512B");
        assert_eq!(fmt_bytes(1023), "1023B");
    }

    #[test]
    fn fmt_bytes_displays_kib() {
        assert_eq!(fmt_bytes(1024), "1KiB");
        assert_eq!(fmt_bytes(500 * 1024), "500KiB");
    }

    #[test]
    fn fmt_bytes_displays_mib() {
        assert_eq!(fmt_bytes(1024 * 1024), "1MiB");
        assert_eq!(fmt_bytes(100 * 1024 * 1024), "100MiB");
    }

    #[test]
    fn fmt_bytes_displays_gib() {
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.0GiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.0GiB");
    }

    // ─── classify_ip ─────────────────────────────────────────────────────

    #[test]
    fn classify_ip_loopback_v4() {
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::Loopback);
    }

    #[test]
    fn classify_ip_loopback_v6() {
        let ip: std::net::IpAddr = "::1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::Loopback);
    }

    #[test]
    fn classify_ip_private_10() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_private_172() {
        let ip: std::net::IpAddr = "172.16.0.1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_private_192() {
        let ip: std::net::IpAddr = "192.168.1.1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_cgnat() {
        let ip: std::net::IpAddr = "100.64.0.1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
        let ip2: std::net::IpAddr = "100.127.255.255".parse().unwrap();
        assert_eq!(classify_ip(&ip2), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_link_local_v4() {
        let ip: std::net::IpAddr = "169.254.1.1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_public_v4() {
        let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::Internet);
    }

    #[test]
    fn classify_ip_link_local_v6() {
        let ip: std::net::IpAddr = "fe80::1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_ula_v6() {
        let ip: std::net::IpAddr = "fd00::1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::LAN);
    }

    #[test]
    fn classify_ip_public_v6() {
        let ip: std::net::IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(classify_ip(&ip), NetworkType::Internet);
    }

    // ─── classify_target ─────────────────────────────────────────────────

    #[test]
    fn classify_target_localhost() {
        assert_eq!(classify_target("localhost"), NetworkType::Loopback);
    }

    #[test]
    fn classify_target_ip_string() {
        assert_eq!(classify_target("10.0.0.5"), NetworkType::LAN);
        assert_eq!(classify_target("8.8.4.4"), NetworkType::Internet);
        assert_eq!(classify_target("127.0.0.1"), NetworkType::Loopback);
    }

    // ─── adaptive_case_id ────────────────────────────────────────────────

    #[test]
    fn adaptive_case_id_default_values() {
        let a = request_attempt(true, 0);
        let id = adaptive_case_id(&a);
        assert_eq!(id, "http1:default:default");
    }

    #[test]
    fn adaptive_case_id_with_stack_and_payload() {
        let a = measured_http_attempt(100.0, 0, 10, 4096, Some("nginx"));
        let id = adaptive_case_id(&a);
        assert_eq!(id, "http1:4096:nginx");
    }

    #[test]
    fn adaptive_case_id_escapes_colons_in_stack() {
        let a = measured_http_attempt(100.0, 0, 10, 1024, Some("stack:v2"));
        let id = adaptive_case_id(&a);
        assert!(
            !id.matches(':').count() > 2 || id.contains("stack_v2"),
            "colons in stack should be escaped"
        );
    }

    // ─── benchmark_attempt_wall_time_ms ──────────────────────────────────

    #[test]
    fn benchmark_attempt_wall_time_ms_empty() {
        assert_eq!(benchmark_attempt_wall_time_ms(&[]), 0.0);
    }

    #[test]
    fn benchmark_attempt_wall_time_ms_single() {
        let attempts = vec![measured_http_attempt(100.0, 0, 50, 1024, None)];
        let wall = benchmark_attempt_wall_time_ms(&attempts);
        assert!((wall - 50.0).abs() < 1.0);
    }

    // ─── average_jitter_ms ───────────────────────────────────────────────

    #[test]
    fn average_jitter_empty() {
        assert_eq!(average_jitter_ms(&[]), 0.0);
    }

    #[test]
    fn average_jitter_single_sample() {
        assert_eq!(average_jitter_ms(&[1.0]), 0.0);
    }

    #[test]
    fn average_jitter_identical_samples() {
        assert_eq!(average_jitter_ms(&[5.0, 5.0, 5.0]), 0.0);
    }

    #[test]
    fn baseline_from_environment_check_reuses_rtt_distribution() {
        let environment_check = BenchmarkEnvironmentCheck {
            attempted_samples: 5,
            successful_samples: 4,
            failed_samples: 1,
            duration_ms: 250.0,
            rtt_min_ms: 0.7,
            rtt_avg_ms: 0.9,
            rtt_max_ms: 1.2,
            rtt_p50_ms: 0.85,
            rtt_p95_ms: 1.1,
            packet_loss_percent: 20.0,
            network_type: NetworkType::Loopback,
        };

        let baseline = baseline_from_environment_check(&environment_check);
        assert_eq!(baseline.samples, 4);
        assert_eq!(baseline.rtt_min_ms, 0.7);
        assert_eq!(baseline.rtt_avg_ms, 0.9);
        assert_eq!(baseline.rtt_max_ms, 1.2);
        assert_eq!(baseline.rtt_p50_ms, 0.85);
        assert_eq!(baseline.rtt_p95_ms, 1.1);
        assert_eq!(baseline.network_type, NetworkType::Loopback);
    }

    // ─── DeterministicRng — panic safety & reproducibility ───────────────

    #[test]
    fn deterministic_rng_reproducible_from_same_seed() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        let mut rng1 = DeterministicRng::from_values(&values);
        let mut rng2 = DeterministicRng::from_values(&values);
        for _ in 0..100 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
        }
    }

    #[test]
    fn deterministic_rng_different_seeds_diverge() {
        let mut rng1 = DeterministicRng::from_values(&[1.0, 2.0]);
        let mut rng2 = DeterministicRng::from_values(&[3.0, 4.0]);
        // At least one of the first 10 values should differ.
        let differs = (0..10).any(|_| rng1.next_u64() != rng2.next_u64());
        assert!(
            differs,
            "different seeds should produce different sequences"
        );
    }

    #[test]
    fn deterministic_rng_empty_array() {
        // Empty input should not panic and should produce a valid RNG.
        let mut rng = DeterministicRng::from_values(&[]);
        // Just verify it doesn't panic and produces values.
        let _ = rng.next_u64();
    }

    #[test]
    fn deterministic_rng_state_never_zero() {
        // Values that XOR to zero should still produce non-zero state.
        let rng = DeterministicRng::from_values(&[0.0]);
        assert_ne!(rng.state, 0);
    }

    #[test]
    fn deterministic_rng_next_index_in_bounds() {
        let mut rng = DeterministicRng::from_values(&[42.0]);
        for upper in [1, 2, 5, 100, 1000] {
            for _ in 0..50 {
                let idx = rng.next_index(upper);
                assert!(idx < upper, "index {idx} >= upper {upper}");
            }
        }
    }

    #[test]
    fn deterministic_rng_next_index_upper_one() {
        // upper=1 → always returns 0.
        let mut rng = DeterministicRng::from_values(&[1.0, 2.0]);
        for _ in 0..10 {
            assert_eq!(rng.next_index(1), 0);
        }
    }

    // ─── published_logical_attempts — edge cases ─────────────────────────

    #[test]
    fn published_logical_attempts_empty_vector() {
        let result = published_logical_attempts(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn published_logical_attempts_single_success() {
        let result = published_logical_attempts(vec![request_attempt(true, 0)]);
        assert_eq!(result.len(), 1);
        assert!(result[0].success);
    }

    #[test]
    fn published_logical_attempts_single_failure() {
        let result = published_logical_attempts(vec![request_attempt(false, 0)]);
        assert_eq!(result.len(), 1);
        assert!(!result[0].success);
    }
}
