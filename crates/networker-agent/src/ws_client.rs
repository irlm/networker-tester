//! WebSocket client that connects to the dashboard control plane.

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::config::AgentConfig;
use networker_common::messages::ControlMessage;
use networker_common::protocol;

/// Connect to the dashboard and process messages until disconnected.
pub async fn run(cfg: &AgentConfig) -> anyhow::Result<()> {
    let url = format!("{}?key={}", cfg.dashboard_url, cfg.api_key);
    tracing::info!("Connecting to {}", cfg.dashboard_url);

    let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await?;
    tracing::info!("Connected to dashboard");

    let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Spawn heartbeat
    let heartbeat_tx = tx.clone();
    let heartbeat_handle = tokio::spawn(crate::heartbeat::run(heartbeat_tx));

    // Forward outbound messages to WebSocket
    let sink_handle = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if ws_sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    // Process inbound messages
    while let Some(msg_result) = ws_stream_rx.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Ok(ctrl_msg) = protocol::decode::<ControlMessage>(&text) {
                    handle_control_message(ctrl_msg, &tx).await;
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

    heartbeat_handle.abort();
    sink_handle.abort();
    Ok(())
}

async fn handle_control_message(msg: ControlMessage, tx: &mpsc::UnboundedSender<String>) {
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
            // Spawn job execution in a separate task
            tokio::spawn(async move {
                crate::executor::run_job(job_id, config, &tx).await;
            });
        }
        ControlMessage::JobCancel { job_id } => {
            tracing::warn!(job_id = %job_id, "Job cancellation requested (not yet implemented)");
            // TODO: implement cancellation via CancellationToken
        }
    }
}
