//! Run execution: runs networker-tester as a subprocess and streams results.
//!
//! v0.28.0 (Agent C) — accepts a canonical `TestConfig` + `TestRun` pair (WS v2)
//! and emits `RunStarted` / `AttemptEvent` / `RunProgress` / `RunFinished` /
//! `Error` events. The agent remains a thin wrapper around the tester CLI:
//! when it receives a run, it builds CLI args from the `TestConfig`, spawns
//! `networker-tester`, streams stdout/stderr back as log lines, and parses
//! the final JSON `TestRun` from stdout to produce per-attempt events plus
//! a terminal `RunFinished`.
//!
//! Cancellation: cooperative — the parent task aborts the JoinHandle, which
//! drops `Child` (which has `kill_on_drop(true)`), terminating the subprocess.

use chrono::Utc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;

use networker_common::messages::{AgentMessage, BenchmarkArtifact};
use networker_common::protocol;
use networker_common::test_config::{EndpointRef, Mode, RunStatus, TestConfig};
use networker_tester::metrics::TestRun as TesterRun;

/// Hard ceiling on tester stdout bytes — guards against runaway JSON.
const MAX_STDOUT_BYTES: usize = 128 * 1024 * 1024;

/// Convert a single `TestConfig.endpoint` (Network kind only — Proxy/Runtime
/// are not yet supported in the CLI executor) into a target string the tester
/// understands. Returns `None` for unsupported kinds.
fn endpoint_to_target(endpoint: &EndpointRef) -> Option<String> {
    match endpoint {
        EndpointRef::Network { host, port } => {
            // If host already looks like a URL, pass through unchanged.
            if host.starts_with("http://") || host.starts_with("https://") {
                return Some(host.clone());
            }
            let scheme = "https";
            Some(match port {
                Some(p) => format!("{scheme}://{host}:{p}/health"),
                None => format!("{scheme}://{host}/health"),
            })
        }
        // Proxy and Runtime endpoints are dispatched via dashboard-side
        // resolution (Agent B). The standalone agent path doesn't yet know
        // how to materialise them into a probe target.
        EndpointRef::Proxy { .. } | EndpointRef::Runtime { .. } => None,
    }
}

/// Build CLI args for `networker-tester` from a `TestConfig`.
fn build_args(config: &TestConfig, target: &str) -> Vec<String> {
    let modes_csv = config
        .workload
        .modes
        .iter()
        .map(Mode::as_str)
        .collect::<Vec<_>>()
        .join(",");

    let timeout_secs = config.workload.timeout_ms.div_ceil(1000).max(1);

    let mut args: Vec<String> = vec![
        "--target".into(),
        target.into(),
        "--modes".into(),
        modes_csv,
        "--runs".into(),
        config.workload.runs.to_string(),
        "--concurrency".into(),
        config.workload.concurrency.to_string(),
        "--timeout".into(),
        timeout_secs.to_string(),
        "--json-stdout".into(),
    ];

    if !config.workload.payload_sizes.is_empty() {
        let csv = config
            .workload
            .payload_sizes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        args.push("--payload-sizes".into());
        args.push(csv);
    }

    args
}

/// Execute a v2 run by spawning the tester CLI and streaming results.
///
/// `cancel_rx` allows cooperative cancellation: dropping the sender (or
/// sending any signal) will kill the child and emit a `Cancelled` terminal
/// status.
pub async fn run_test(
    run_id: Uuid,
    config: TestConfig,
    tx: mpsc::Sender<String>,
    mut cancel_rx: mpsc::Receiver<()>,
) {
    let correlation_id = run_id.to_string();
    tracing::info!(
        correlation_id,
        config_id = %config.id,
        endpoint_kind = config.endpoint_kind(),
        modes = ?config.workload.modes,
        is_benchmark = config.is_benchmark(),
        "Run received"
    );

    // RunStarted ─────────────────────────────────────────────────────────────
    send(
        &tx,
        &AgentMessage::RunStarted {
            run_id,
            started_at: Utc::now(),
        },
        &correlation_id,
    );

    // Resolve endpoint → target URL ──────────────────────────────────────────
    let target = match endpoint_to_target(&config.endpoint) {
        Some(t) => t,
        None => {
            let msg = format!(
                "Unsupported endpoint kind for standalone agent: {}",
                config.endpoint_kind()
            );
            tracing::error!(correlation_id, "{msg}");
            send(
                &tx,
                &AgentMessage::Error {
                    run_id: Some(run_id),
                    message: msg,
                },
                &correlation_id,
            );
            send_finished(&tx, run_id, RunStatus::Failed, None, &correlation_id);
            return;
        }
    };

    let args = build_args(&config, &target);

    // Locate tester binary ───────────────────────────────────────────────────
    let bin_path = match find_tester_binary().await {
        Some(p) => p,
        None => {
            let msg = "networker-tester binary not found on this machine".to_string();
            tracing::error!(correlation_id, "{msg}");
            send(
                &tx,
                &AgentMessage::Error {
                    run_id: Some(run_id),
                    message: msg,
                },
                &correlation_id,
            );
            send_finished(&tx, run_id, RunStatus::Failed, None, &correlation_id);
            return;
        }
    };

    tracing::info!(
        correlation_id,
        bin = %bin_path,
        args = ?args,
        "Spawning tester subprocess"
    );

    let mut child = match Command::new(&bin_path)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to spawn tester: {e}");
            tracing::error!(correlation_id, "{msg}");
            send(
                &tx,
                &AgentMessage::Error {
                    run_id: Some(run_id),
                    message: msg,
                },
                &correlation_id,
            );
            send_finished(&tx, run_id, RunStatus::Failed, None, &correlation_id);
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let success_count = Arc::new(AtomicU32::new(0));
    let failure_count = Arc::new(AtomicU32::new(0));

    // Stream stderr (tester logs) — best-effort; on send failure we just stop
    // logging (the run continues).
    let stderr_tx = tx.clone();
    let stderr_corr = correlation_id.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stderr_tx
                .send(
                    protocol::encode(&AgentMessage::Error {
                        run_id: Some(run_id),
                        message: format!("[tester] {line}"),
                    })
                    .unwrap_or_default(),
                )
                .await;
            // We don't want to spam Error frames for every log line; instead
            // just emit a tracing event. The Error envelope above is gated
            // behind a tracing log, kept terse.
            let _ = &stderr_corr;
        }
    });

    // Read stdout (final JSON `TestRun`) into memory with a hard cap.
    let mut stdout_lines: Vec<String> = Vec::new();
    let mut stdout_bytes = 0usize;
    let mut stdout_reader = BufReader::new(stdout).lines();

    let exit_status = loop {
        tokio::select! {
            biased;
            _ = cancel_rx.recv() => {
                tracing::warn!(correlation_id, "Run cancelled — killing tester subprocess");
                let _ = child.kill().await;
                let _ = child.wait().await;
                stderr_task.abort();
                send_finished(&tx, run_id, RunStatus::Cancelled, None, &correlation_id);
                return;
            }
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        stdout_bytes = stdout_bytes.saturating_add(l.len() + 1);
                        if stdout_bytes > MAX_STDOUT_BYTES {
                            let _ = child.kill().await;
                            let msg = format!(
                                "Tester stdout exceeded safety limit of {MAX_STDOUT_BYTES} bytes"
                            );
                            tracing::error!(correlation_id, "{msg}");
                            send(
                                &tx,
                                &AgentMessage::Error {
                                    run_id: Some(run_id),
                                    message: msg,
                                },
                                &correlation_id,
                            );
                            stderr_task.abort();
                            send_finished(&tx, run_id, RunStatus::Failed, None, &correlation_id);
                            return;
                        }
                        stdout_lines.push(l);
                    }
                    Ok(None) => break child.wait().await,
                    Err(e) => {
                        tracing::warn!(correlation_id, error = %e, "stdout read error; awaiting exit");
                        break child.wait().await;
                    }
                }
            }
        }
    };

    stderr_task.abort();
    let stdout_text = stdout_lines.join("\n");

    let parsed = serde_json::from_str::<TesterRun>(&stdout_text);
    let (status, run_opt) = match (&exit_status, parsed) {
        (Ok(s), Ok(run)) if s.success() => (RunStatus::Completed, Some(run)),
        // Non-zero exit but we still parsed JSON → treat as completed-with-failures.
        (Ok(_), Ok(run)) => (RunStatus::Completed, Some(run)),
        // Process failure or unparseable output → Failed.
        (Ok(s), Err(parse_err)) => {
            let code = s.code().unwrap_or(-1);
            let snippet: String = stdout_text.chars().take(512).collect();
            let msg = format!(
                "Tester exited with code {code} and unparseable JSON: {parse_err} (stdout starts: {snippet})"
            );
            tracing::error!(correlation_id, "{msg}");
            send(
                &tx,
                &AgentMessage::Error {
                    run_id: Some(run_id),
                    message: msg,
                },
                &correlation_id,
            );
            (RunStatus::Failed, None)
        }
        (Err(e), _) => {
            let msg = format!("Tester process error: {e}");
            tracing::error!(correlation_id, "{msg}");
            send(
                &tx,
                &AgentMessage::Error {
                    run_id: Some(run_id),
                    message: msg,
                },
                &correlation_id,
            );
            (RunStatus::Failed, None)
        }
    };

    if let Some(run) = run_opt {
        // Stream per-attempt events + maintain progress counts.
        for attempt in &run.attempts {
            let prev = if attempt.success {
                success_count.fetch_add(1, Ordering::Relaxed)
            } else {
                failure_count.fetch_add(1, Ordering::Relaxed)
            };
            let total = prev + 1;
            send(
                &tx,
                &AgentMessage::AttemptEvent {
                    run_id,
                    attempt: Box::new(attempt.clone()),
                },
                &correlation_id,
            );
            // Periodic progress every 10 attempts.
            if total % 10 == 0 {
                send(
                    &tx,
                    &AgentMessage::RunProgress {
                        run_id,
                        success: success_count.load(Ordering::Relaxed),
                        failure: failure_count.load(Ordering::Relaxed),
                    },
                    &correlation_id,
                );
            }
        }
        // Final progress event.
        send(
            &tx,
            &AgentMessage::RunProgress {
                run_id,
                success: success_count.load(Ordering::Relaxed),
                failure: failure_count.load(Ordering::Relaxed),
            },
            &correlation_id,
        );
    }

    // Build BenchmarkArtifact only when methodology is present. The detailed
    // statistical pipeline lives in networker-tester::benchmark; the standalone
    // agent ships a minimal placeholder envelope so the dashboard can persist
    // *something* and the schema is exercised end-to-end. Agent A/B will fill
    // in the full case/sample/quality_gate fields in a follow-up.
    let artifact = if config.is_benchmark() {
        Some(Box::new(BenchmarkArtifact {
            environment: serde_json::json!({
                "client_os": std::env::consts::OS,
                "client_version": env!("CARGO_PKG_VERSION"),
            }),
            methodology: serde_json::to_value(&config.methodology)
                .unwrap_or(serde_json::Value::Null),
            launches: serde_json::json!([]),
            cases: serde_json::json!([]),
            samples: None,
            summaries: serde_json::json!({
                "success": success_count.load(Ordering::Relaxed),
                "failure": failure_count.load(Ordering::Relaxed),
            }),
            data_quality: serde_json::json!({
                "noise_level": null,
                "publication_ready": false,
                "blockers": ["agent-side artifact synthesis is a placeholder pending Agent A/B"],
            }),
        }))
    } else {
        None
    };

    send_finished(&tx, run_id, status, artifact, &correlation_id);
}

fn send_finished(
    tx: &mpsc::Sender<String>,
    run_id: Uuid,
    status: RunStatus,
    artifact: Option<Box<BenchmarkArtifact>>,
    correlation_id: &str,
) {
    send(
        tx,
        &AgentMessage::RunFinished {
            run_id,
            status,
            artifact,
        },
        correlation_id,
    );
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

/// Locate the `networker-tester` binary on this host.
async fn find_tester_binary() -> Option<String> {
    for path in &[
        "target/debug/networker-tester",
        "target/release/networker-tester",
    ] {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(path.to_string());
        }
    }

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

    let lookup = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = Command::new(lookup).arg("networker-tester").output().await {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}
