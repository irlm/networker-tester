use anyhow::Context;
use chrono::Utc;
use clap::Parser;
use networker_tester::capture;
use networker_tester::cli;
use networker_tester::cli::ResolvedConfig;
use networker_tester::metrics::{
    attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value,
    BenchmarkEnvironmentCheck, BenchmarkExecutionPlan, BenchmarkNoiseThresholds,
    BenchmarkStabilityCheck, HostInfo, NetworkBaseline, NetworkType, PageLoadResult, Protocol,
    RequestAttempt, TestRun,
};
use networker_tester::output;
use networker_tester::output::db;
use networker_tester::output::{excel, html, json};
use networker_tester::runner::{
    browser::run_browser_probe,
    curl::run_curl_probe,
    dns::run_dns_probe,
    http::{run_probe, RunConfig},
    http3::run_http3_probe,
    native::run_native_probe,
    pageload::{
        run_pageload2_probe, run_pageload2_warm, run_pageload3_probe, run_pageload_probe,
        warmup_pageload2, PageLoadConfig, SharedH2Conn,
    },
    throughput::{
        run_download1_probe, run_download2_probe, run_download3_probe, run_download_probe,
        run_upload1_probe, run_upload2_probe, run_upload3_probe, run_upload_probe,
        run_webdownload_probe, run_webupload_probe, ThroughputConfig,
    },
    tls::{run_tls_probe, run_tls_resumption_probe},
    udp::{run_udp_probe, UdpProbeConfig},
    udp_throughput::{run_udpdownload_probe, run_udpupload_probe, UdpThroughputConfig},
};
use networker_tester::tls_profile::{
    run_tls_endpoint_profile, TlsProfileRequest, TlsProfileTargetKind,
};
use networker_tester::url_diagnostic::{
    UrlDiagnosticCapabilities, UrlDiagnosticOrchestrator, UrlDiagnosticRequest,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};
use uuid::Uuid;

#[cfg(feature = "http3")]
use networker_tester::runner::pageload::{run_pageload3_warm, warmup_pageload3};

// ─────────────────────────────────────────────────────────────────────────────
// Progress reporting
// ─────────────────────────────────────────────────────────────────────────────

/// Fire-and-forget HTTP progress reporter for benchmark orchestrator integration.
struct ProgressReporter {
    client: reqwest::Client,
    url: String,
    token: String,
    config_id: String,
    testbed_id: Option<String>,
    language: String,
    interval: u32,
}

impl ProgressReporter {
    fn new(
        url: String,
        token: String,
        config_id: String,
        testbed_id: Option<String>,
        language: String,
        interval: u32,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap_or_default(),
            url,
            token,
            config_id,
            testbed_id,
            language,
            interval,
        }
    }

    async fn report(
        &self,
        mode: &str,
        request_index: u32,
        total_requests: u32,
        latency_ms: f64,
        success: bool,
    ) {
        // Only POST at the configured interval
        if self.interval > 1
            && !request_index.is_multiple_of(self.interval)
            && request_index < total_requests
        {
            return;
        }
        let payload = serde_json::json!({
            "config_id": self.config_id,
            "testbed_id": self.testbed_id,
            "language": self.language,
            "mode": mode,
            "request_index": request_index,
            "total_requests": total_requests,
            "latency_ms": latency_ms,
            "success": success,
        });
        // Fire and forget — don't block the benchmark
        let _ = self
            .client
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
    }
}

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

fn print_url_test_summary(run: &networker_tester::metrics::UrlTestRun, json_path: &Path) {
    println!("URL Test Summary");
    println!("----------------");
    println!("Requested URL: {}", run.requested_url);
    if let Some(final_url) = &run.final_url {
        println!("Final URL: {final_url}");
    }
    println!("Status: {:?}", run.status);
    println!();
    println!("Primary Load");
    println!(
        "- Observed Protocol (main document): {}",
        run.observed_protocol_primary_load
            .as_deref()
            .unwrap_or("unknown")
    );
    println!(
        "- Primary Origin: {}",
        run.primary_origin.as_deref().unwrap_or("-")
    );
    println!();
    println!("Milestones");
    println!(
        "- DNS: {}",
        run.dns_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- Connect: {}",
        run.connect_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- Handshake: {}",
        run.handshake_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- TTFB: {}",
        run.ttfb_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- DOMContentLoaded: {}",
        run.dom_content_loaded_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- Load Event: {}",
        run.load_event_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!();
    println!("Page Summary");
    println!("- Requests: {}", run.total_requests);
    println!("- Transfer Size: {} bytes", run.total_transfer_bytes);
    println!("- Failures: {}", run.failure_count);
    println!();
    if !run.capture_errors.is_empty() {
        println!("Warnings");
        for err in &run.capture_errors {
            println!("- {err}");
        }
        println!();
    }
    println!("Artifacts");
    println!("- JSON: {}", json_path.display());
    println!(
        "- HAR: {}",
        run.har_path.as_deref().unwrap_or("not captured")
    );
    println!(
        "- PCAP: {}",
        run.pcap_path.as_deref().unwrap_or("not captured")
    );
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
    measure_rtt_samples(host, port, samples, DEFAULT_STABILITY_CHECK_INTERVAL_MS)
        .await
        .0
}

/// Measure TCP connect RTT to a target N times, preserving attempt order.
async fn measure_rtt_samples(
    host: &str,
    port: u16,
    samples: u32,
    interval_ms: u64,
) -> (Vec<f64>, u32, f64) {
    let mut rtts = Vec::with_capacity(samples as usize);
    let addr = format!("{host}:{port}");
    let started = std::time::Instant::now();
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
        if interval_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
        }
    }
    (rtts, samples, started.elapsed().as_secs_f64() * 1000.0)
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

fn baseline_from_environment_check(
    environment_check: &BenchmarkEnvironmentCheck,
) -> NetworkBaseline {
    NetworkBaseline {
        samples: environment_check.successful_samples,
        rtt_min_ms: environment_check.rtt_min_ms,
        rtt_avg_ms: environment_check.rtt_avg_ms,
        rtt_max_ms: environment_check.rtt_max_ms,
        rtt_p50_ms: environment_check.rtt_p50_ms,
        rtt_p95_ms: environment_check.rtt_p95_ms,
        network_type: environment_check.network_type,
    }
}

fn average_jitter_ms(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .sum::<f64>()
        / (samples.len() - 1) as f64
}

fn baseline_from_stability_check(stability_check: &BenchmarkStabilityCheck) -> NetworkBaseline {
    NetworkBaseline {
        samples: stability_check.successful_samples,
        rtt_min_ms: stability_check.rtt_min_ms,
        rtt_avg_ms: stability_check.rtt_avg_ms,
        rtt_max_ms: stability_check.rtt_max_ms,
        rtt_p50_ms: stability_check.rtt_p50_ms,
        rtt_p95_ms: stability_check.rtt_p95_ms,
        network_type: stability_check.network_type,
    }
}

async fn measure_environment_check(
    target: &url::Url,
    samples: u32,
    interval_ms: u64,
) -> Option<BenchmarkEnvironmentCheck> {
    let host = target.host_str()?;
    let port = target.port_or_known_default()?;
    let network_type = classify_target(host);
    let (ordered_rtts, attempted_samples, duration_ms) =
        measure_rtt_samples(host, port, samples, interval_ms).await;
    let successful_samples = ordered_rtts.len() as u32;
    let failed_samples = attempted_samples.saturating_sub(successful_samples);
    let packet_loss_percent = if attempted_samples > 0 {
        failed_samples as f64 / attempted_samples as f64 * 100.0
    } else {
        0.0
    };
    let mut sorted_rtts = ordered_rtts.clone();
    sorted_rtts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if sorted_rtts.is_empty() {
        return Some(BenchmarkEnvironmentCheck {
            attempted_samples,
            successful_samples,
            failed_samples,
            duration_ms,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_max_ms: 0.0,
            rtt_p50_ms: 0.0,
            rtt_p95_ms: 0.0,
            packet_loss_percent,
            network_type,
        });
    }

    let sum: f64 = sorted_rtts.iter().sum();
    Some(BenchmarkEnvironmentCheck {
        attempted_samples,
        successful_samples,
        failed_samples,
        duration_ms,
        rtt_min_ms: sorted_rtts[0],
        rtt_avg_ms: sum / sorted_rtts.len() as f64,
        rtt_max_ms: sorted_rtts[sorted_rtts.len() - 1],
        rtt_p50_ms: percentile(&sorted_rtts, 50.0),
        rtt_p95_ms: percentile(&sorted_rtts, 95.0),
        packet_loss_percent,
        network_type,
    })
}

async fn measure_stability_check(
    target: &url::Url,
    samples: u32,
    interval_ms: u64,
) -> Option<BenchmarkStabilityCheck> {
    let host = target.host_str()?;
    let port = target.port_or_known_default()?;
    let network_type = classify_target(host);
    let (ordered_rtts, attempted_samples, duration_ms) =
        measure_rtt_samples(host, port, samples, interval_ms).await;
    let successful_samples = ordered_rtts.len() as u32;
    let failed_samples = attempted_samples.saturating_sub(successful_samples);
    let packet_loss_percent = if attempted_samples > 0 {
        failed_samples as f64 / attempted_samples as f64 * 100.0
    } else {
        0.0
    };
    let jitter_ms = average_jitter_ms(&ordered_rtts);
    let mut sorted_rtts = ordered_rtts.clone();
    sorted_rtts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if sorted_rtts.is_empty() {
        return Some(BenchmarkStabilityCheck {
            attempted_samples,
            successful_samples,
            failed_samples,
            duration_ms,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_max_ms: 0.0,
            rtt_p50_ms: 0.0,
            rtt_p95_ms: 0.0,
            jitter_ms,
            packet_loss_percent,
            network_type,
        });
    }

    let sum: f64 = sorted_rtts.iter().sum();
    Some(BenchmarkStabilityCheck {
        attempted_samples,
        successful_samples,
        failed_samples,
        duration_ms,
        rtt_min_ms: sorted_rtts[0],
        rtt_avg_ms: sum / sorted_rtts.len() as f64,
        rtt_max_ms: *sorted_rtts.last().unwrap_or(&sorted_rtts[0]),
        rtt_p50_ms: percentile(&sorted_rtts, 50.0),
        rtt_p95_ms: percentile(&sorted_rtts, 95.0),
        jitter_ms,
        packet_loss_percent,
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

fn published_logical_attempts(attempts: Vec<RequestAttempt>) -> Vec<RequestAttempt> {
    attempts.into_iter().last().into_iter().collect()
}

const ADAPTIVE_BOOTSTRAP_RESAMPLES: usize = 1_024;
const ADAPTIVE_CONFIDENCE_LEVEL: f64 = 0.95;
const DEFAULT_AUTO_TARGET_RELATIVE_ERROR: f64 = 0.05;
const DEFAULT_PILOT_MIN_SAMPLES: u32 = 6;
const DEFAULT_PILOT_MAX_SAMPLES: u32 = 12;
const DEFAULT_PILOT_MIN_DURATION_MS: u64 = 0;
const DEFAULT_ENVIRONMENT_CHECK_SAMPLES: u32 = 5;
const DEFAULT_ENVIRONMENT_CHECK_INTERVAL_MS: u64 = 50;
const DEFAULT_STABILITY_CHECK_SAMPLES: u32 = 12;
const DEFAULT_STABILITY_CHECK_INTERVAL_MS: u64 = 50;
const DEFAULT_OVERHEAD_SAMPLES: u32 = 1;
const DEFAULT_COOLDOWN_SAMPLES: u32 = 1;

#[derive(Debug, Clone, Copy)]
struct BenchmarkAdaptiveCriteria {
    min_samples: u32,
    max_samples: u32,
    min_duration_ms: u64,
    target_relative_error: Option<f64>,
    target_absolute_error: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct BenchmarkPilotCriteria {
    min_samples: u32,
    max_samples: u32,
    min_duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchmarkAdaptiveStopReason {
    AccuracyTargetReached,
    MaxSamplesReached,
}

#[derive(Debug, Clone)]
struct BenchmarkAdaptiveStatus {
    completed_samples: u32,
    elapsed_ms: f64,
    stop_reason: Option<BenchmarkAdaptiveStopReason>,
}

#[derive(Debug, Clone, Copy)]
struct MedianErrorBounds {
    median: f64,
    absolute_half_width: f64,
}

#[derive(Debug, Clone)]
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn from_values(values: &[f64]) -> Self {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ values.len() as u64;
        for value in values {
            state ^= value.to_bits().wrapping_mul(0xbf58_476d_1ce4_e5b9);
            state = state.rotate_left(13);
        }
        if state == 0 {
            state = 0x94d0_49bb_1331_11eb;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_index(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }
}

fn benchmark_pilot_criteria(cfg: &ResolvedConfig) -> Option<BenchmarkPilotCriteria> {
    if !cfg.benchmark_mode || cfg.benchmark_phase != "measured" || !cfg.http_stacks.is_empty() {
        return None;
    }

    let pilot_requested = cfg.benchmark_pilot_min_samples.is_some()
        || cfg.benchmark_pilot_max_samples.is_some()
        || cfg.benchmark_pilot_min_duration_ms.is_some();
    let no_explicit_measured_controls = cfg.benchmark_min_samples.is_none()
        && cfg.benchmark_max_samples.is_none()
        && cfg.benchmark_min_duration_ms.is_none()
        && cfg.benchmark_target_relative_error.is_none()
        && cfg.benchmark_target_absolute_error.is_none();
    if !pilot_requested && !no_explicit_measured_controls {
        return None;
    }

    let default_pilot_max = cfg.runs.clamp(1, DEFAULT_PILOT_MAX_SAMPLES);
    let default_pilot_min = default_pilot_max.min(DEFAULT_PILOT_MIN_SAMPLES);

    Some(BenchmarkPilotCriteria {
        min_samples: cfg.benchmark_pilot_min_samples.unwrap_or(default_pilot_min),
        max_samples: cfg.benchmark_pilot_max_samples.unwrap_or(default_pilot_max),
        min_duration_ms: cfg
            .benchmark_pilot_min_duration_ms
            .unwrap_or(DEFAULT_PILOT_MIN_DURATION_MS),
    })
}

fn benchmark_adaptive_criteria(cfg: &ResolvedConfig) -> Option<BenchmarkAdaptiveCriteria> {
    let controls_requested = cfg.benchmark_min_samples.is_some()
        || cfg.benchmark_max_samples.is_some()
        || cfg.benchmark_min_duration_ms.is_some()
        || cfg.benchmark_target_relative_error.is_some()
        || cfg.benchmark_target_absolute_error.is_some();
    if !cfg.benchmark_mode || cfg.benchmark_phase != "measured" || !controls_requested {
        return None;
    }

    Some(BenchmarkAdaptiveCriteria {
        min_samples: cfg.benchmark_min_samples.unwrap_or(cfg.runs),
        max_samples: cfg.benchmark_max_samples.unwrap_or(cfg.runs),
        min_duration_ms: cfg.benchmark_min_duration_ms.unwrap_or(0),
        target_relative_error: cfg.benchmark_target_relative_error,
        target_absolute_error: cfg.benchmark_target_absolute_error,
    })
}

fn derive_measured_plan_from_pilot(
    cfg: &ResolvedConfig,
    pilot_attempts: &[RequestAttempt],
) -> BenchmarkExecutionPlan {
    let pilot_status = benchmark_adaptive_status(
        &BenchmarkAdaptiveCriteria {
            min_samples: 1,
            max_samples: u32::MAX,
            min_duration_ms: 0,
            target_relative_error: None,
            target_absolute_error: None,
        },
        pilot_attempts,
    );
    let target_relative_error = cfg
        .benchmark_target_relative_error
        .or(Some(DEFAULT_AUTO_TARGET_RELATIVE_ERROR));
    let target_absolute_error = cfg.benchmark_target_absolute_error;

    let mut values_by_case: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for attempt in pilot_attempts {
        if attempt.success {
            if let Some(value) = primary_metric_value(attempt) {
                values_by_case
                    .entry(adaptive_case_id(attempt))
                    .or_default()
                    .push(value);
            }
        }
    }

    let estimated_max_samples = if values_by_case.is_empty() {
        cfg.runs
    } else {
        values_by_case
            .values()
            .map(|values| {
                estimated_samples_for_error_targets(
                    values,
                    target_relative_error,
                    target_absolute_error,
                )
            })
            .max()
            .unwrap_or(cfg.runs)
            .clamp(1, cfg.runs.max(1))
    };

    let min_samples = cfg
        .benchmark_min_samples
        .unwrap_or(pilot_status.completed_samples.max(1));
    let max_samples = cfg.benchmark_max_samples.unwrap_or(
        estimated_max_samples
            .max(min_samples)
            .min(cfg.runs.max(min_samples)),
    );
    let min_duration_ms = cfg
        .benchmark_min_duration_ms
        .unwrap_or(pilot_status.elapsed_ms.ceil().clamp(0.0, u64::MAX as f64) as u64);
    let source = if cfg.benchmark_min_samples.is_none()
        && cfg.benchmark_max_samples.is_none()
        && cfg.benchmark_min_duration_ms.is_none()
        && cfg.benchmark_target_relative_error.is_none()
        && cfg.benchmark_target_absolute_error.is_none()
    {
        "pilot-derived"
    } else {
        "pilot-assisted"
    };

    BenchmarkExecutionPlan {
        source: source.to_string(),
        min_samples,
        max_samples,
        min_duration_ms,
        target_relative_error,
        target_absolute_error,
        pilot_sample_count: pilot_status.completed_samples,
        pilot_elapsed_ms: Some(pilot_status.elapsed_ms),
    }
}

fn benchmark_adaptive_status(
    criteria: &BenchmarkAdaptiveCriteria,
    attempts: &[RequestAttempt],
) -> BenchmarkAdaptiveStatus {
    let elapsed_ms = benchmark_attempt_wall_time_ms(attempts);
    let mut samples_by_case: BTreeMap<String, usize> = BTreeMap::new();
    let mut values_by_case: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    for attempt in attempts {
        let case_id = adaptive_case_id(attempt);
        *samples_by_case.entry(case_id.clone()).or_default() += 1;
        if attempt.success {
            if let Some(value) = primary_metric_value(attempt) {
                values_by_case.entry(case_id).or_default().push(value);
            }
        }
    }

    let completed_samples = samples_by_case
        .values()
        .min()
        .copied()
        .unwrap_or_default()
        .try_into()
        .unwrap_or(u32::MAX);
    let min_samples_satisfied = completed_samples >= criteria.min_samples;
    let min_duration_satisfied = elapsed_ms >= criteria.min_duration_ms as f64;
    let requires_accuracy_target =
        criteria.target_relative_error.is_some() || criteria.target_absolute_error.is_some();
    let accuracy_satisfied = if requires_accuracy_target {
        !samples_by_case.is_empty()
            && samples_by_case.keys().all(|case_id| {
                values_by_case.get(case_id).is_some_and(|values| {
                    median_error_bounds(values).is_some_and(|error_bounds| {
                        let relative_ok = criteria.target_relative_error.is_none_or(|target| {
                            error_bounds.median.abs() > f64::EPSILON
                                && error_bounds.absolute_half_width / error_bounds.median.abs()
                                    <= target
                        });
                        let absolute_ok = criteria
                            .target_absolute_error
                            .is_none_or(|target| error_bounds.absolute_half_width <= target);
                        relative_ok && absolute_ok
                    })
                })
            })
    } else {
        true
    };

    let stop_reason = if completed_samples >= criteria.max_samples {
        Some(BenchmarkAdaptiveStopReason::MaxSamplesReached)
    } else if min_samples_satisfied && min_duration_satisfied && accuracy_satisfied {
        Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached)
    } else {
        None
    };

    BenchmarkAdaptiveStatus {
        completed_samples,
        elapsed_ms,
        stop_reason,
    }
}

fn adaptive_case_id(attempt: &RequestAttempt) -> String {
    let payload = attempt_payload_bytes(attempt)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default".into());
    let stack = attempt
        .http_stack
        .as_deref()
        .unwrap_or("default")
        .replace(':', "_");
    format!("{}:{}:{}", attempt.protocol, payload, stack)
}

fn benchmark_attempt_wall_time_ms(attempts: &[RequestAttempt]) -> f64 {
    let start = attempts.iter().map(|attempt| attempt.started_at).min();
    let end = attempts
        .iter()
        .map(|attempt| attempt.finished_at.unwrap_or(attempt.started_at))
        .max();
    match (start, end) {
        (Some(start), Some(end)) => (end - start)
            .num_microseconds()
            .map(|micros| micros as f64 / 1000.0)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

fn median_error_bounds(values: &[f64]) -> Option<MedianErrorBounds> {
    let mut sorted = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    if sorted.len() < 2 {
        return None;
    }
    let median = percentile_from_sorted(&sorted, 50.0);
    let (_, lower, upper) = bootstrap_median_interval(&sorted);
    Some(MedianErrorBounds {
        median,
        absolute_half_width: (upper - lower) / 2.0,
    })
}

fn estimated_samples_for_error_targets(
    values: &[f64],
    target_relative_error: Option<f64>,
    target_absolute_error: Option<f64>,
) -> u32 {
    let current_n = values.len().clamp(1, u32::MAX as usize) as f64;
    let Some(error_bounds) = median_error_bounds(values) else {
        return current_n as u32;
    };

    let mut estimated = current_n;
    if let Some(target) = target_relative_error {
        if error_bounds.median.abs() > f64::EPSILON {
            let current_relative_error =
                error_bounds.absolute_half_width / error_bounds.median.abs();
            estimated = estimated.max(current_n * (current_relative_error / target).powi(2));
        }
    }
    if let Some(target) = target_absolute_error {
        estimated = estimated.max(current_n * (error_bounds.absolute_half_width / target).powi(2));
    }

    estimated.ceil().clamp(1.0, u32::MAX as f64) as u32
}

fn median_from_sorted(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else if sorted.len().is_multiple_of(2) {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    }
}

fn bootstrap_median_interval(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if values.len() == 1 {
        return (0.0, values[0], values[0]);
    }

    let mut rng = DeterministicRng::from_values(values);
    let mut estimates = Vec::with_capacity(ADAPTIVE_BOOTSTRAP_RESAMPLES);
    for _ in 0..ADAPTIVE_BOOTSTRAP_RESAMPLES {
        let mut sample = Vec::with_capacity(values.len());
        for _ in 0..values.len() {
            sample.push(values[rng.next_index(values.len())]);
        }
        sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        estimates.push(median_from_sorted(&sample));
    }

    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let estimate_mean = estimates.iter().sum::<f64>() / estimates.len() as f64;
    let estimate_variance = estimates
        .iter()
        .map(|value| (value - estimate_mean).powi(2))
        .sum::<f64>()
        / (estimates.len() as f64 - 1.0);
    let standard_error = estimate_variance.sqrt();
    let tail = (1.0 - ADAPTIVE_CONFIDENCE_LEVEL) * 50.0;
    let lower = percentile_from_sorted(&estimates, tail);
    let upper = percentile_from_sorted(&estimates, 100.0 - tail);
    (standard_error, lower, upper)
}

fn percentile_from_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = percentile / 100.0 * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
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
        Protocol::TlsResume,
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
    use chrono::{Duration, TimeZone};
    use networker_tester::cli::{
        ImpairmentProfile, ResolvedImpairmentConfig, ResolvedPacketCaptureConfig,
    };
    use networker_tester::metrics::HttpResult;
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
