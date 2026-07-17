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

mod target_runner;
mod tls_profile_cli;
mod url_test_cli;

#[cfg(test)]
mod main_tests;

use target_runner::run_for_target;
use tls_profile_cli::run_tls_profile_cli;
use url_test_cli::run_url_test_cli;

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

    // Always write logs to stderr to keep stdout clean for JSON output
    let mut builder =
        networker_log::LogBuilder::new("tester").with_console(networker_log::Stream::Stderr);
    if let Some(ref filter) = cfg.log_level {
        builder = builder.with_filter(filter);
    }
    // Optional DB logging (used when orchestrator passes --log-db-url)
    if let Some(ref url) = cfg.log_db_url {
        builder = builder.with_db(url);
    }
    let _log_guard = builder.init().await?;

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
