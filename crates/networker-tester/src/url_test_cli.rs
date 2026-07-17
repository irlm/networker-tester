//! `--url-test-url` CLI mode: one-shot URL diagnostic run.

use super::*;

fn make_url_test_capture_config(cfg: &ResolvedConfig) -> ResolvedConfig {
    let mut resolved = cfg.clone();
    resolved.targets = vec![cfg
        .url_test_url
        .clone()
        .unwrap_or_else(|| "https://example.com".into())];
    resolved
}
pub(crate) async fn run_url_test_cli(cfg: &ResolvedConfig) -> anyhow::Result<()> {
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
