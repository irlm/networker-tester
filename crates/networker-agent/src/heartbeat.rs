use tokio::sync::mpsc;

use networker_common::messages::AgentMessage;
use networker_common::protocol;

/// Spawn a heartbeat task that sends a heartbeat message every 30 seconds.
/// Returns when the sender is dropped (connection closed).
pub async fn run(tx: mpsc::Sender<String>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        let msg = AgentMessage::Heartbeat {
            load: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        };
        match protocol::encode(&msg) {
            Ok(text) => {
                if tx.try_send(text).is_err() {
                    break; // Channel closed
                }
            }
            Err(e) => {
                tracing::warn!("Failed to encode heartbeat: {e}");
            }
        }
    }
}
