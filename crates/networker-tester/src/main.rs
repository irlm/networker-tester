use networker_tester::cli;
use networker_tester::metrics::{Protocol, TestRun};
use networker_tester::output::{html, json, sql};
use networker_tester::runner::{
    http::{run_probe, RunConfig},
    http3::run_http3_probe,
    udp::{run_udp_probe, UdpProbeConfig},
};
use anyhow::Context;
use chrono::Utc;
use clap::Parser;
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
    cli.validate()?;

    let level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .init();

    let target = url::Url::parse(&cli.target)
        .context("Invalid --target URL")?;
    let target_host = target
        .host_str()
        .unwrap_or("unknown")
        .to_string();

    let modes = cli.parsed_modes();
    if modes.is_empty() {
        anyhow::bail!("No valid modes specified. Use: tcp,http1,http2,http3,udp");
    }

    let run_id = Uuid::new_v4();
    let started_at = Utc::now();

    info!(
        run_id = %run_id,
        target = %cli.target,
        modes  = ?modes.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        runs   = cli.runs,
        "Starting networker-tester"
    );

    let cfg = RunConfig {
        timeout_ms: cli.timeout * 1000,
        dns_enabled: cli.dns_enabled,
        ipv4_only: cli.ipv4_only,
        ipv6_only: cli.ipv6_only,
        insecure: cli.insecure,
        payload_size: cli.payload_size,
        path: target.path().to_string(),
    };

    let udp_cfg = UdpProbeConfig {
        target_host: target_host.clone(),
        target_port: cli.udp_port,
        probe_count: cli.udp_probes,
        timeout_ms: cli.timeout * 1000,
        payload_size: 64,
    };

    // ── Collect all attempts ──────────────────────────────────────────────────
    let mut all_attempts = Vec::new();
    let mut seq = 0u32;

    for run_num in 0..cli.runs {
        info!("Run {}/{}", run_num + 1, cli.runs);

        let futures: Vec<_> = modes
            .iter()
            .map(|proto| {
                let proto = proto.clone();
                let target_clone = target.clone();
                let cfg_clone = cfg.clone();
                let udp_cfg_clone = udp_cfg.clone();
                let current_seq = seq;
                seq += 1;

                async move {
                    let a = match &proto {
                        Protocol::Http1 | Protocol::Http2 | Protocol::Tcp => {
                            run_probe(run_id, current_seq, proto, &target_clone, &cfg_clone).await
                        }
                        Protocol::Http3 => {
                            run_http3_probe(run_id, current_seq, &target_clone, cfg_clone.timeout_ms)
                                .await
                        }
                        Protocol::Udp => {
                            run_udp_probe(run_id, current_seq, &udp_cfg_clone).await
                        }
                    };

                    // Progress logging
                    log_attempt(&a);
                    a
                }
            })
            .collect();

        // Run futures with bounded concurrency
        use futures::stream::{self, StreamExt};
        let results: Vec<_> = stream::iter(futures)
            .buffer_unordered(cli.concurrency)
            .collect()
            .await;
        all_attempts.extend(results);
    }

    let finished_at = Utc::now();

    let run = TestRun {
        run_id,
        started_at,
        finished_at: Some(finished_at),
        target_url: cli.target.clone(),
        target_host: target_host.clone(),
        modes: modes.iter().map(|m| m.to_string()).collect(),
        total_runs: cli.runs,
        concurrency: cli.concurrency as u32,
        timeout_ms: cli.timeout * 1000,
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
    let out_dir = PathBuf::from(&cli.output_dir);
    std::fs::create_dir_all(&out_dir).context("Cannot create output directory")?;

    // ── JSON artifact ─────────────────────────────────────────────────────────
    let json_path = out_dir.join(format!(
        "run-{}.json",
        started_at.format("%Y%m%d-%H%M%S")
    ));
    json::save(&run, &json_path).context("Failed to write JSON artifact")?;
    info!(path = %json_path.display(), "JSON artifact saved");

    // ── HTML report ───────────────────────────────────────────────────────────
    let html_path = out_dir.join(&cli.html_report);
    let css_href = cli.css.as_deref().or(Some("report.css"));
    html::save(&run, &html_path, css_href).context("Failed to write HTML report")?;
    info!(path = %html_path.display(), "HTML report saved");

    // Copy default CSS to output dir if it doesn't exist yet
    copy_default_css(&out_dir);

    // ── SQL insert ────────────────────────────────────────────────────────────
    if cli.save_to_sql {
        if let Some(conn_str) = &cli.connection_string {
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
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn log_attempt(a: &networker_tester::metrics::RequestAttempt) {
    use networker_tester::metrics::Protocol::*;
    let status = if a.success { "✓" } else { "✗" };

    match &a.protocol {
        Http1 | Http2 | Http3 | Tcp => {
            let dns_ms   = a.dns.as_ref().map(|d| d.duration_ms).unwrap_or(0.0);
            let tcp_ms   = a.tcp.as_ref().map(|t| t.connect_duration_ms).unwrap_or(0.0);
            let tls_ms   = a.tls.as_ref().map(|t| t.handshake_duration_ms).unwrap_or(0.0);
            let ttfb_ms  = a.http.as_ref().map(|h| h.ttfb_ms).unwrap_or(0.0);
            let total_ms = a.http.as_ref().map(|h| h.total_duration_ms).unwrap_or(0.0);
            let ver      = a.http.as_ref().map(|h| h.negotiated_version.clone()).unwrap_or_default();
            let status_code = a.http.as_ref().map(|h| h.status_code.to_string()).unwrap_or_default();

            info!(
                "{status} #{seq} [{proto}] {status_code} {ver} \
                 DNS:{dns:.1}ms TCP:{tcp:.1}ms TLS:{tls:.1}ms TTFB:{ttfb:.1}ms Total:{total:.1}ms",
                seq    = a.sequence_num,
                proto  = a.protocol,
                dns    = dns_ms,
                tcp    = tcp_ms,
                tls    = tls_ms,
                ttfb   = ttfb_ms,
                total  = total_ms,
            );
        }
        Udp => {
            if let Some(u) = &a.udp {
                info!(
                    "{status} #{seq} [udp] RTT avg={avg:.1}ms p95={p95:.1}ms loss={loss:.1}%",
                    seq  = a.sequence_num,
                    avg  = u.rtt_avg_ms,
                    p95  = u.rtt_p95_ms,
                    loss = u.loss_percent,
                );
            }
        }
    }

    if let Some(e) = &a.error {
        warn!("  Error [{cat}] {msg}", cat = e.category, msg = e.message);
    }
}

fn print_summary(run: &TestRun) {
    let ok = run.success_count();
    let fail = run.failure_count();
    let total = run.attempts.len();

    println!("\n══════════════════════════════════════════════");
    println!(" Networker Tester – Run {}", run.run_id);
    println!("══════════════════════════════════════════════");
    println!(" Target  : {}", run.target_url);
    println!(" Modes   : {}", run.modes.join(", "));
    println!(" Results : {ok}/{total} succeeded  ({fail} failed)");

    if let Some(fin) = run.finished_at {
        let dur = (fin - run.started_at).num_milliseconds();
        println!(" Duration: {dur}ms total");
    }

    // Per-protocol table
    println!("\n Protocol  │ #   │ Avg DNS │ Avg TCP │ Avg TLS │ Avg TTFB │ Avg Total");
    println!("───────────┼─────┼─────────┼─────────┼─────────┼──────────┼───────────");

    for proto in &[Protocol::Http1, Protocol::Http2, Protocol::Http3, Protocol::Tcp, Protocol::Udp] {
        let rows: Vec<_> = run.attempts.iter().filter(|a| &a.protocol == proto).collect();
        if rows.is_empty() { continue; }

        let avg_f = |f: fn(&networker_tester::metrics::RequestAttempt) -> Option<f64>| -> String {
            let vals: Vec<f64> = rows.iter().filter_map(|a| f(a)).collect();
            if vals.is_empty() { "—".into() }
            else { format!("{:.1}ms", vals.iter().sum::<f64>() / vals.len() as f64) }
        };

        println!(" {proto:<9} │ {n:<3} │ {dns:<7} │ {tcp:<7} │ {tls:<7} │ {ttfb:<8} │ {total}",
            n     = rows.len(),
            dns   = avg_f(|a| a.dns.as_ref().map(|d| d.duration_ms)),
            tcp   = avg_f(|a| a.tcp.as_ref().map(|t| t.connect_duration_ms)),
            tls   = avg_f(|a| a.tls.as_ref().map(|t| t.handshake_duration_ms)),
            ttfb  = avg_f(|a| a.http.as_ref().map(|h| h.ttfb_ms)),
            total = avg_f(|a| a.http.as_ref().map(|h| h.total_duration_ms)
                            .or_else(|| a.udp.as_ref().map(|u| u.rtt_avg_ms))),
        );
    }

    println!("══════════════════════════════════════════════\n");
}

/// Copy the bundled `report.css` from the binary's embedded bytes to the
/// output directory so the HTML report can link to it.
fn copy_default_css(out_dir: &Path) {
    let dest = out_dir.join("report.css");
    if dest.exists() { return; }
    if let Ok(src) = std::fs::read("assets/report.css") {
        let _ = std::fs::write(&dest, src);
    } else {
        let _ = std::fs::write(&dest, networker_tester::output::html::FALLBACK_CSS);
    }
}
