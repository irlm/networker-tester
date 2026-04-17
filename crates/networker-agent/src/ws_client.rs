//! WebSocket client that connects to the dashboard control plane (WS v2).
//!
//! v0.28.0 — speaks WS protocol v2 (`AssignRun` / `CancelRun` / `Shutdown`).
//! Legacy v1 `JobAssign` / `JobCancel` are still handled for backward
//! compatibility during the parallel-agent transition (the dashboard may still
//! dispatch v1 messages until Agent B cuts over to v2).

use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::Instrument;
use uuid::Uuid;

use crate::config::AgentConfig;
use networker_common::messages::{AgentCommandLog, AgentMessage, ControlMessage};
use networker_common::protocol;

/// Maximum number of probe runs that can execute concurrently.
const MAX_CONCURRENT_RUNS: usize = 4;
/// Bounded channel capacity for outbound WebSocket messages.
const WS_CHANNEL_CAPACITY: usize = 4096;

/// Per-run handle: the JoinHandle for the spawned task plus a cancellation
/// sender. Dropping the sender or sending `()` triggers cooperative cancel.
struct RunHandle {
    task: JoinHandle<()>,
    cancel_tx: mpsc::Sender<()>,
}

/// Connect to the dashboard and process messages until disconnected.
pub async fn run(cfg: &AgentConfig) -> anyhow::Result<()> {
    let url = format!("{}?key={}", cfg.dashboard_url, cfg.api_key);
    tracing::info!("Connecting to {}", cfg.dashboard_url);

    let mut ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    ws_config.max_message_size = Some(64 * 1024 * 1024);
    ws_config.max_frame_size = Some(64 * 1024 * 1024);
    ws_config.max_write_buffer_size = 64 * 1024 * 1024;
    let (ws_stream, _) =
        tokio_tungstenite::connect_async_with_config(&url, Some(ws_config), false).await?;
    tracing::info!("Connected to dashboard (WS v2)");

    let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<String>(WS_CHANNEL_CAPACITY);

    let run_semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS));
    let running: Arc<Mutex<HashMap<Uuid, RunHandle>>> = Arc::new(Mutex::new(HashMap::new()));

    // Spawn heartbeat
    let heartbeat_tx = tx.clone();
    let heartbeat_handle = tokio::spawn(crate::heartbeat::run(heartbeat_tx));

    // Forward outbound messages to WebSocket
    let sink_handle = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if ws_sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // Process inbound messages
    while let Some(msg_result) = ws_stream_rx.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Ok(ctrl) = protocol::decode::<ControlMessage>(&text) {
                    handle_control_message(ctrl, &tx, &run_semaphore, &running).await;
                }
            }
            Ok(Message::Close(_)) => {
                tracing::info!("Server closed connection");
                break;
            }
            Ok(Message::Ping(_)) => {} // pong handled by tungstenite
            Err(e) => {
                tracing::error!("WebSocket error: {e}");
                break;
            }
            _ => {}
        }
    }

    // Abort all running tasks on disconnect
    {
        let mut runs = running.lock().await;
        for (run_id, handle) in runs.drain() {
            tracing::warn!(run_id = %run_id, "Aborting run due to disconnect");
            // Signal cooperative cancel first, then abort.
            let _ = handle.cancel_tx.try_send(());
            handle.task.abort();
        }
    }

    heartbeat_handle.abort();
    sink_handle.abort();
    Ok(())
}

/// Build an `mpsc::Sender<AgentCommandLog>` whose output is forwarded —
/// encoded as `AgentMessage::CommandLog` JSON — onto the outbound WebSocket
/// channel.
fn build_log_tx(out: mpsc::Sender<String>) -> mpsc::Sender<AgentCommandLog> {
    let (tx, mut rx) = mpsc::channel::<AgentCommandLog>(64);
    tokio::spawn(async move {
        while let Some(log) = rx.recv().await {
            match protocol::encode(&AgentMessage::CommandLog(log)) {
                Ok(text) => {
                    if out.send(text).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to encode CommandLog: {e}");
                }
            }
        }
    });
    tx
}

async fn handle_control_message(
    msg: ControlMessage,
    tx: &mpsc::Sender<String>,
    semaphore: &Arc<tokio::sync::Semaphore>,
    running: &Arc<Mutex<HashMap<Uuid, RunHandle>>>,
) {
    match msg {
        // ── WS v2 ────────────────────────────────────────────────────────────
        ControlMessage::AssignRun { run, config } => {
            let run_id = run.id;
            tracing::info!(
                run_id = %run_id,
                config_name = %config.name,
                endpoint_kind = config.endpoint_kind(),
                "AssignRun received (v2)"
            );
            let tx = tx.clone();
            let sem = semaphore.clone();
            let runs_map = running.clone();
            let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);
            let span = tracing::info_span!("run", %run_id);
            let handle = tokio::spawn(
                async move {
                    let _permit = match sem.acquire().await {
                        Ok(p) => p,
                        Err(_) => return, // semaphore closed
                    };
                    crate::executor::run_test(run_id, *config, tx, cancel_rx).await;
                    runs_map.lock().await.remove(&run_id);
                }
                .instrument(span),
            );
            running.lock().await.insert(
                run_id,
                RunHandle {
                    task: handle,
                    cancel_tx,
                },
            );
        }
        ControlMessage::CancelRun { run_id } => {
            tracing::warn!(run_id = %run_id, "CancelRun received (v2)");
            if let Some(handle) = running.lock().await.remove(&run_id) {
                let _ = handle.cancel_tx.try_send(());
                // Give the task a moment to notice the cancel signal before
                // hard-aborting. In practice `kill_on_drop` + channel recv
                // in the executor will terminate it quickly.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                handle.task.abort();
            }
        }
        ControlMessage::HeartbeatPing { now } => {
            tracing::trace!(server_time = %now, "HeartbeatPing from dashboard");
        }
        ControlMessage::Shutdown => {
            tracing::info!("Shutdown request from dashboard — aborting all runs");
            let mut runs = running.lock().await;
            for (run_id, handle) in runs.drain() {
                tracing::warn!(run_id = %run_id, "Aborting run due to shutdown");
                let _ = handle.cancel_tx.try_send(());
                handle.task.abort();
            }
            // The main loop will exit when the connection closes.
        }

        // ── Legacy v1 (backward compat — dashboard may still send these) ──
        ControlMessage::JobAssign { .. } => {
            tracing::warn!(
                "Received legacy JobAssign (v1) — ignoring. Dashboard should send AssignRun (v2)."
            );
        }
        ControlMessage::JobCancel { job_id } => {
            tracing::warn!(job_id = %job_id, "Received legacy JobCancel (v1) — ignoring.");
        }

        // ── Common ───────────────────────────────────────────────────────────
        ControlMessage::Welcome {
            agent_id,
            agent_name,
        } => {
            tracing::info!(
                agent_id = %agent_id,
                name = %agent_name,
                "Registered with dashboard"
            );
        }
        ControlMessage::Command(cmd) => {
            tracing::info!(
                command_id = %cmd.command_id,
                verb = %cmd.verb,
                "Received command"
            );
            let out_tx = tx.clone();
            let log_tx = build_log_tx(out_tx.clone());
            let command_id = cmd.command_id;
            tokio::spawn(async move {
                let result = crate::commands::run_command(cmd, log_tx).await;
                match protocol::encode(&AgentMessage::CommandResult(result)) {
                    Ok(text) => {
                        if out_tx.send(text).await.is_err() {
                            tracing::warn!(
                                command_id = %command_id,
                                "Failed to send command result: outbound channel closed"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            command_id = %command_id,
                            "Failed to encode CommandResult: {e}"
                        );
                    }
                }
            });
        }
        ControlMessage::Cancel(cancel) => {
            tracing::debug!(
                command_id = %cancel.command_id,
                "Received Cancel for command; no in-flight commands are cancellable yet"
            );
        }
    }
}
