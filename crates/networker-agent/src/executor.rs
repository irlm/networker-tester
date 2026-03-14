//! Job execution: orchestrates probe runs using networker-tester library functions.
//! Mirrors the logic from networker-tester/src/main.rs::run_for_target() but with
//! streaming callbacks for each completed RequestAttempt.
//!
//! Every log line includes a correlation_id (= job_id) and run_id for tracing.

use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

use networker_common::messages::{AgentMessage, JobConfig};
use networker_common::protocol;
use networker_tester::metrics::{Protocol, RequestAttempt, TestRun};
use networker_tester::runner::http::RunConfig;

/// Execute a job and stream results back via the WebSocket sender channel.
pub async fn run_job(job_id: Uuid, config: JobConfig, tx: &mpsc::UnboundedSender<String>) {
    let run_id = Uuid::new_v4();
    let correlation_id = job_id.to_string();
    let span = tracing::info_span!("job", correlation_id = %correlation_id, run_id = %run_id);
    let _guard = span.enter();

    tracing::info!(
        target = %config.target,
        modes = ?config.modes,
        runs = config.runs,
        insecure = config.insecure,
        "Job received — sending ACK"
    );

    // Acknowledge the job
    send(tx, &AgentMessage::JobAck { job_id }, &correlation_id);

    let started_at = Utc::now();

    // Parse modes
    let modes: Vec<Protocol> = config
        .modes
        .iter()
        .filter_map(|m| {
            let parsed = m.parse::<Protocol>();
            if parsed.is_err() {
                tracing::warn!(mode = %m, "Skipping unrecognized mode");
            }
            parsed.ok()
        })
        .collect();

    if modes.is_empty() {
        tracing::error!("No valid modes — aborting job");
        send(
            tx,
            &AgentMessage::JobError {
                job_id,
                message: "No valid modes specified".into(),
            },
            &correlation_id,
        );
        return;
    }

    // Parse target URL
    let target = match url::Url::parse(&config.target) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(target = %config.target, error = %e, "Invalid target URL — aborting job");
            send(
                tx,
                &AgentMessage::JobError {
                    job_id,
                    message: format!("Invalid target URL: {e}"),
                },
                &correlation_id,
            );
            return;
        }
    };

    // Parse payload sizes
    let payload_sizes: Vec<usize> = config
        .payload_sizes
        .iter()
        .filter_map(|s| parse_size(s))
        .collect();

    tracing::info!(
        modes = ?modes.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        payload_sizes = ?payload_sizes,
        "Parsed config — starting probe loop"
    );

    // Build RunConfig for HTTP probes
    let run_cfg = RunConfig {
        timeout_ms: config.timeout_secs * 1000,
        dns_enabled: config.dns_enabled,
        ipv4_only: config.ipv4_only,
        ipv6_only: config.ipv6_only,
        insecure: config.insecure,
        payload_size: 0,
        path: target.path().to_string(),
        ca_bundle: None,
        proxy: None,
        no_proxy: false,
    };

    let mut attempts: Vec<RequestAttempt> = Vec::new();
    let mut seq: u32 = 0;
    let mut success_count: usize = 0;
    let mut failure_count: usize = 0;

    // Main probe loop
    for run_num in 0..config.runs {
        for mode in &modes {
            let sizes = if needs_payload(mode) && !payload_sizes.is_empty() {
                payload_sizes.clone()
            } else {
                vec![0]
            };

            for &sz in &sizes {
                tracing::info!(
                    run = run_num + 1,
                    total_runs = config.runs,
                    mode = %mode,
                    seq = seq,
                    payload_bytes = sz,
                    "Dispatching probe"
                );

                let attempt =
                    dispatch_probe(run_id, seq, mode, &target, &run_cfg, sz, &correlation_id).await;

                if attempt.success {
                    success_count += 1;
                    let ttfb = attempt.http.as_ref().map(|h| h.ttfb_ms);
                    let total = attempt.http.as_ref().map(|h| h.total_duration_ms);
                    tracing::info!(
                        seq = seq,
                        mode = %mode,
                        ttfb_ms = ?ttfb,
                        total_ms = ?total,
                        "Probe OK"
                    );
                } else {
                    failure_count += 1;
                    let err_msg = attempt
                        .error
                        .as_ref()
                        .map(|e| e.message.as_str())
                        .unwrap_or("unknown");
                    let err_cat = attempt
                        .error
                        .as_ref()
                        .map(|e| format!("{:?}", e.category))
                        .unwrap_or_default();
                    tracing::warn!(
                        seq = seq,
                        mode = %mode,
                        error_category = %err_cat,
                        error = %err_msg,
                        "Probe FAILED"
                    );
                }

                // Stream the result immediately
                tracing::debug!(seq = seq, "Streaming attempt result to dashboard");
                send(
                    tx,
                    &AgentMessage::AttemptResult {
                        job_id,
                        attempt: attempt.clone(),
                    },
                    &correlation_id,
                );

                attempts.push(attempt);
                seq += 1;
            }
        }
    }

    tracing::info!(
        total_probes = seq,
        success = success_count,
        failures = failure_count,
        "Probe loop complete — building TestRun"
    );

    // Build the complete TestRun
    let run = TestRun {
        run_id,
        started_at,
        finished_at: Some(Utc::now()),
        target_url: config.target.clone(),
        target_host: target.host_str().unwrap_or("unknown").to_string(),
        modes: config.modes.clone(),
        total_runs: config.runs,
        concurrency: config.concurrency as u32,
        timeout_ms: config.timeout_secs * 1000,
        client_os: std::env::consts::OS.to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        server_info: None,
        client_info: None,
        baseline: None,
        attempts,
    };

    tracing::info!("Sending JobComplete to dashboard");
    send(
        tx,
        &AgentMessage::JobComplete { job_id, run },
        &correlation_id,
    );
    tracing::info!("Job finished");
}

/// Dispatch a single probe based on the protocol mode.
async fn dispatch_probe(
    run_id: Uuid,
    seq: u32,
    mode: &Protocol,
    target: &url::Url,
    cfg: &RunConfig,
    _payload_size: usize,
    correlation_id: &str,
) -> RequestAttempt {
    use networker_tester::runner;

    let start = std::time::Instant::now();

    let result = match mode {
        Protocol::Http1 | Protocol::Http2 | Protocol::Tcp => {
            tracing::debug!(correlation_id, seq, mode = %mode, "Using HTTP runner");
            runner::http::run_probe(run_id, seq, mode.clone(), target, cfg).await
        }
        Protocol::Http3 => {
            tracing::debug!(correlation_id, seq, "Using HTTP/3 runner");
            runner::http3::run_http3_probe(
                run_id,
                seq,
                target,
                cfg.timeout_ms,
                cfg.insecure,
                cfg.ca_bundle.as_deref(),
            )
            .await
        }
        Protocol::Dns => {
            tracing::debug!(correlation_id, seq, "Using DNS runner");
            runner::dns::run_dns_probe(
                run_id,
                seq,
                target.host_str().unwrap_or("localhost"),
                cfg.ipv4_only,
                cfg.ipv6_only,
            )
            .await
        }
        Protocol::Tls => {
            tracing::debug!(correlation_id, seq, "Using TLS runner");
            runner::tls::run_tls_probe(run_id, seq, target, cfg).await
        }
        _ => {
            tracing::warn!(correlation_id, seq, mode = %mode, "Mode not yet supported by agent");
            RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: mode.clone(),
                sequence_num: seq,
                retry_count: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: false,
                dns: None,
                tcp: None,
                tls: None,
                http: None,
                udp: None,
                error: Some(networker_tester::metrics::ErrorRecord {
                    category: networker_tester::metrics::ErrorCategory::Config,
                    message: format!("Mode {mode} not yet supported by agent executor"),
                    detail: None,
                    occurred_at: Utc::now(),
                }),
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
                http_stack: None,
            }
        }
    };

    let elapsed = start.elapsed();
    tracing::debug!(
        correlation_id,
        seq,
        elapsed_ms = elapsed.as_millis() as u64,
        "Probe dispatch complete"
    );

    result
}

fn needs_payload(mode: &Protocol) -> bool {
    matches!(
        mode,
        Protocol::Download
            | Protocol::Upload
            | Protocol::WebDownload
            | Protocol::WebUpload
            | Protocol::UdpDownload
            | Protocol::UdpUpload
    )
}

fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim().to_lowercase();
    if let Some(n) = s.strip_suffix('k') {
        n.parse::<usize>().ok().map(|v| v * 1024)
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<usize>().ok().map(|v| v * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('g') {
        n.parse::<usize>().ok().map(|v| v * 1024 * 1024 * 1024)
    } else {
        s.parse().ok()
    }
}

fn send(tx: &mpsc::UnboundedSender<String>, msg: &AgentMessage, correlation_id: &str) {
    match protocol::encode(msg) {
        Ok(text) => {
            let msg_type = match msg {
                AgentMessage::Heartbeat { .. } => "heartbeat",
                AgentMessage::JobAck { .. } => "job_ack",
                AgentMessage::AttemptResult { .. } => "attempt_result",
                AgentMessage::JobComplete { .. } => "job_complete",
                AgentMessage::JobError { .. } => "job_error",
            };
            tracing::debug!(
                correlation_id,
                msg_type,
                bytes = text.len(),
                "Sending WS message"
            );
            if tx.send(text).is_err() {
                tracing::error!(
                    correlation_id,
                    msg_type,
                    "Failed to send message: channel closed"
                );
            }
        }
        Err(e) => tracing::error!(correlation_id, error = %e, "Failed to encode message"),
    }
}
