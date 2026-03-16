//! Job execution: runs networker-tester as a subprocess and streams results.
//!
//! The agent is a thin wrapper around the tester CLI. When it receives a job,
//! it builds the CLI arguments from JobConfig, spawns `networker-tester`, and
//! streams stdout/stderr back to the dashboard as log lines. When complete,
//! it reads the JSON output to build the TestRun.

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;

use networker_common::messages::{AgentMessage, JobConfig};
use networker_common::protocol;
use networker_tester::metrics::TestRun;

/// Execute a job by running networker-tester CLI and streaming results.
pub async fn run_job(job_id: Uuid, config: JobConfig, tx: &mpsc::Sender<String>) {
    let correlation_id = job_id.to_string();

    tracing::info!(
        correlation_id,
        target = %config.target,
        modes = ?config.modes,
        "Job received — sending ACK"
    );

    send(tx, &AgentMessage::JobAck { job_id }, &correlation_id);
    log(
        tx,
        job_id,
        "info",
        &format!(
            "Starting test: {} modes={}",
            config.target,
            config.modes.join(",")
        ),
    );

    // Build CLI args
    let mut args = vec![
        "--target".to_string(),
        config.target.clone(),
        "--modes".to_string(),
        config.modes.join(","),
        "--runs".to_string(),
        config.runs.to_string(),
        "--concurrency".to_string(),
        config.concurrency.to_string(),
        "--timeout".to_string(),
        config.timeout_secs.to_string(),
        "--json-stdout".to_string(), // Output TestRun as JSON to stdout
    ];

    if config.insecure {
        args.push("--insecure".to_string());
    }
    if !config.dns_enabled {
        // dns_enabled defaults to true, only pass when disabling
        args.push("--dns-enabled".to_string());
        args.push("false".to_string());
    }
    if config.ipv4_only {
        args.push("--ipv4-only".to_string());
    }
    if config.ipv6_only {
        args.push("--ipv6-only".to_string());
    }
    if config.connection_reuse {
        args.push("--connection-reuse".to_string());
    }
    if !config.payload_sizes.is_empty() {
        args.push("--payload-sizes".to_string());
        args.push(config.payload_sizes.join(","));
    }
    if let Some(ref preset) = config.page_preset {
        args.push("--page-preset".to_string());
        args.push(preset.clone());
    }
    if let Some(assets) = config.page_assets {
        args.push("--page-assets".to_string());
        args.push(assets.to_string());
    }
    if let Some(ref size) = config.page_asset_size {
        args.push("--page-asset-size".to_string());
        args.push(size.clone());
    }
    if let Some(port) = config.udp_port {
        args.push("--udp-port".to_string());
        args.push(port.to_string());
    }
    if let Some(port) = config.udp_throughput_port {
        args.push("--udp-throughput-port".to_string());
        args.push(port.to_string());
    }
    if let Some(ref mode) = config.capture_mode {
        if mode != "none" && !mode.is_empty() {
            args.push("--capture-mode".to_string());
            args.push(mode.clone());
        }
    }

    // SSRF protection: block probes targeting private, loopback, link-local,
    // or cloud metadata IPs/hostnames.
    if let Ok(parsed) = url::Url::parse(&config.target) {
        if is_private_or_metadata(&parsed) {
            tracing::error!(target = %config.target, "SSRF blocked: target resolves to private/metadata address");
            send(
                tx,
                &AgentMessage::JobError {
                    job_id,
                    message: "Target blocked: private, loopback, or metadata address".into(),
                },
                &correlation_id,
            );
            return;
        }
    }

    // Find tester binary
    let tester_bin = find_tester_binary().await;
    let bin_path = match &tester_bin {
        Some(p) => p.as_str(),
        None => {
            log(tx, job_id, "error", "networker-tester binary not found");
            send(
                tx,
                &AgentMessage::JobError {
                    job_id,
                    message: "networker-tester binary not found on this machine".into(),
                },
                &correlation_id,
            );
            return;
        }
    };

    log(
        tx,
        job_id,
        "info",
        &format!("Running: {bin_path} {}", args.join(" ")),
    );

    // Spawn tester process
    let mut child = match Command::new(bin_path)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log(tx, job_id, "error", &format!("Failed to spawn tester: {e}"));
            send(
                tx,
                &AgentMessage::JobError {
                    job_id,
                    message: format!("Failed to spawn tester: {e}"),
                },
                &correlation_id,
            );
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let mut stderr_reader = BufReader::new(stderr).lines();
    let mut stdout_lines = Vec::new();
    let mut stdout_reader = BufReader::new(stdout).lines();

    let tx_clone = tx.clone();
    let job_id_clone = job_id;

    // Read stderr (tester log output) in background and stream to dashboard
    let stderr_task = tokio::spawn(async move {
        while let Ok(Some(line)) = stderr_reader.next_line().await {
            // Stream log lines to dashboard
            log(&tx_clone, job_id_clone, "info", &line);
        }
    });

    // Read stdout (JSON output when --json is used, or regular output)
    while let Ok(Some(line)) = stdout_reader.next_line().await {
        stdout_lines.push(line);
    }

    // Wait for stderr task and process exit
    stderr_task.await.ok();
    let exit_status = child.wait().await;

    let stdout_text = stdout_lines.join("\n");

    match exit_status {
        Ok(status) if status.success() => {
            // Try to parse JSON TestRun from stdout
            match serde_json::from_str::<TestRun>(&stdout_text) {
                Ok(run) => {
                    let success_count = run.success_count();
                    let failure_count = run.failure_count();
                    let run_id = run.run_id;

                    log(
                        tx,
                        job_id,
                        "info",
                        &format!(
                            "Complete: {} probes, {} OK, {} failed",
                            run.attempts.len(),
                            success_count,
                            failure_count,
                        ),
                    );

                    // Stream individual attempt results for live UI
                    for attempt in &run.attempts {
                        send(
                            tx,
                            &AgentMessage::AttemptResult {
                                job_id,
                                attempt: Box::new(attempt.clone()),
                            },
                            &correlation_id,
                        );
                    }

                    send(
                        tx,
                        &AgentMessage::JobComplete {
                            job_id,
                            run: Box::new(run),
                        },
                        &correlation_id,
                    );
                    tracing::info!(correlation_id, run_id = %run_id, "Job complete");
                }
                Err(e) => {
                    // JSON parse failed — maybe --json isn't supported or output is different
                    // Try to build a minimal TestRun from what we have
                    tracing::warn!(correlation_id, error = %e, "Failed to parse tester JSON output");
                    log(
                        tx,
                        job_id,
                        "warn",
                        &format!("Could not parse test results: {e}"),
                    );

                    // Send as completed with empty run
                    let run = TestRun {
                        run_id: Uuid::new_v4(),
                        started_at: Utc::now(),
                        finished_at: Some(Utc::now()),
                        target_url: config.target.clone(),
                        target_host: url::Url::parse(&config.target)
                            .map(|u| u.host_str().unwrap_or("unknown").to_string())
                            .unwrap_or_else(|_| "unknown".to_string()),
                        modes: config.modes.clone(),
                        total_runs: config.runs,
                        concurrency: config.concurrency as u32,
                        timeout_ms: config.timeout_secs * 1000,
                        client_os: std::env::consts::OS.to_string(),
                        client_version: env!("CARGO_PKG_VERSION").to_string(),
                        server_info: None,
                        client_info: None,
                        baseline: None,
                        attempts: vec![],
                    };
                    send(
                        tx,
                        &AgentMessage::JobComplete {
                            job_id,
                            run: Box::new(run),
                        },
                        &correlation_id,
                    );
                }
            }
        }
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            // Include stderr output in error message
            let msg = if !stdout_text.is_empty() {
                format!("Tester exited with code {code}: {stdout_text}")
            } else {
                format!("Tester exited with code {code}")
            };
            log(tx, job_id, "error", &msg);
            send(
                tx,
                &AgentMessage::JobError {
                    job_id,
                    message: msg,
                },
                &correlation_id,
            );
        }
        Err(e) => {
            log(tx, job_id, "error", &format!("Tester process error: {e}"));
            send(
                tx,
                &AgentMessage::JobError {
                    job_id,
                    message: format!("Tester process error: {e}"),
                },
                &correlation_id,
            );
        }
    }
}

/// Send a log line to the dashboard AND to tracing.
fn log(tx: &mpsc::Sender<String>, job_id: Uuid, level: &str, line: &str) {
    match level {
        "error" => tracing::error!(job_id = %job_id, "{}", line),
        "warn" => tracing::warn!(job_id = %job_id, "{}", line),
        _ => tracing::info!(job_id = %job_id, "{}", line),
    }
    send(
        tx,
        &AgentMessage::JobLog {
            job_id,
            line: line.to_string(),
            level: level.to_string(),
        },
        &job_id.to_string(),
    );
}

/// Returns `true` if the URL targets a private, loopback, link-local,
/// or cloud metadata IP/hostname — used to prevent SSRF via agent job dispatch.
fn is_private_or_metadata(url: &url::Url) -> bool {
    use std::net::IpAddr;
    if let Some(host) = url.host_str() {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return is_private_ip(&ip);
        }
        // Block well-known cloud metadata hostnames
        let h = host.to_lowercase();
        if h == "metadata.google.internal" || h == "169.254.169.254" {
            return true;
        }
    }
    false
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.octets()[..2] == [169, 254]
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.segments()[0] == 0xfe80
            // link-local
        }
    }
}

fn send(tx: &mpsc::Sender<String>, msg: &AgentMessage, correlation_id: &str) {
    match protocol::encode(msg) {
        Ok(text) => {
            if tx.try_send(text).is_err() {
                tracing::error!(
                    correlation_id,
                    "Failed to send message: channel full or closed"
                );
            }
        }
        Err(e) => tracing::error!(correlation_id, error = %e, "Failed to encode message"),
    }
}

/// Find the networker-tester binary.
async fn find_tester_binary() -> Option<String> {
    // Try common locations
    for path in &[
        "target/debug/networker-tester",
        "target/release/networker-tester",
    ] {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(path.to_string());
        }
    }

    // Try workspace root (walk up)
    if let Ok(cwd) = std::env::current_dir() {
        for sub in &[
            "target/debug/networker-tester",
            "target/release/networker-tester",
        ] {
            let p = cwd.join(sub);
            if tokio::fs::metadata(&p).await.is_ok() {
                return Some(p.to_string_lossy().to_string());
            }
        }
        let mut dir = cwd.as_path();
        for _ in 0..5 {
            if let Some(parent) = dir.parent() {
                for sub in &[
                    "target/debug/networker-tester",
                    "target/release/networker-tester",
                ] {
                    let p = parent.join(sub);
                    if tokio::fs::metadata(&p).await.is_ok() {
                        return Some(p.to_string_lossy().to_string());
                    }
                }
                dir = parent;
            }
        }
    }

    // Try PATH
    if let Ok(output) = Command::new("which").arg("networker-tester").output().await {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}
