//! WebSocket client that connects to the dashboard control plane.

use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::Instrument;
use uuid::Uuid;

use crate::config::AgentConfig;
use networker_common::messages::ControlMessage;
use networker_common::protocol;

/// Maximum number of probe jobs that can run concurrently.
const MAX_CONCURRENT_JOBS: usize = 4;
/// Bounded channel capacity for outbound WebSocket messages.
/// Must be large enough for all AttemptResult + JobLog + JobComplete messages
/// from a full test run (e.g., 27 modes × 5 runs = 135+ attempts + logs).
const WS_CHANNEL_CAPACITY: usize = 4096;

/// Connect to the dashboard and process messages until disconnected.
pub async fn run(cfg: &AgentConfig) -> anyhow::Result<()> {
    let url = format!("{}?key={}", cfg.dashboard_url, cfg.api_key);
    tracing::info!("Connecting to {}", cfg.dashboard_url);

    // Large test runs (many modes × runs) produce multi-MB JSON in JobComplete.
    // Default tungstenite limits (64KB frame, 64KB message) silently drop these.
    let mut ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    ws_config.max_message_size = Some(64 * 1024 * 1024); // 64 MB receive
    ws_config.max_frame_size = Some(64 * 1024 * 1024); // 64 MB frame
    ws_config.max_write_buffer_size = 64 * 1024 * 1024; // 64 MB write buffer
    let (ws_stream, _) =
        tokio_tungstenite::connect_async_with_config(&url, Some(ws_config), false).await?;
    tracing::info!("Connected to dashboard");

    let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<String>(WS_CHANNEL_CAPACITY);

    let job_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS));
    let running_jobs: Arc<Mutex<HashMap<Uuid, JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

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
                if let Ok(ctrl_msg) = protocol::decode::<ControlMessage>(&text) {
                    handle_control_message(ctrl_msg, &tx, &job_semaphore, &running_jobs).await;
                }
            }
            Ok(Message::Close(_)) => {
                tracing::info!("Server closed connection");
                break;
            }
            Ok(Message::Ping(_)) => {
                // Pong is handled automatically by tungstenite
            }
            Err(e) => {
                tracing::error!("WebSocket error: {e}");
                break;
            }
            _ => {}
        }
    }

    // Abort all running jobs on disconnect
    {
        let mut jobs = running_jobs.lock().await;
        for (job_id, handle) in jobs.drain() {
            tracing::warn!(job_id = %job_id, "Aborting running job due to disconnect");
            handle.abort();
        }
    }

    heartbeat_handle.abort();
    sink_handle.abort();
    Ok(())
}

async fn handle_control_message(
    msg: ControlMessage,
    tx: &mpsc::Sender<String>,
    semaphore: &Arc<Semaphore>,
    running_jobs: &Arc<Mutex<HashMap<Uuid, JoinHandle<()>>>>,
) {
    match msg {
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
        ControlMessage::JobAssign { job_id, config } => {
            tracing::info!(job_id = %job_id, target = %config.target, "Received job");
            let tx = tx.clone();
            let sem = semaphore.clone();
            let jobs = running_jobs.clone();
            let span = tracing::info_span!("job", %job_id);
            let handle = tokio::spawn(
                async move {
                    let _permit = match sem.acquire().await {
                        Ok(p) => p,
                        Err(_) => return, // semaphore closed
                    };
                    crate::executor::run_job(job_id, *config, &tx).await;
                    jobs.lock().await.remove(&job_id);
                }
                .instrument(span),
            );
            running_jobs.lock().await.insert(job_id, handle);
        }
        ControlMessage::JobCancel { job_id } => {
            tracing::warn!(job_id = %job_id, "Cancelling job");
            if let Some(handle) = running_jobs.lock().await.remove(&job_id) {
                handle.abort();
            }
        }
        ControlMessage::Command(cmd) => {
            // Command execution lands in a later task of the orchestration plan.
            tracing::debug!(
                command_id = %cmd.command_id,
                verb = %cmd.verb,
                "Ignoring Command message (handler not yet implemented)"
            );
        }
        ControlMessage::Cancel(cancel) => {
            tracing::debug!(
                command_id = %cancel.command_id,
                "Ignoring command Cancel message (handler not yet implemented)"
            );
        }
    }
}
