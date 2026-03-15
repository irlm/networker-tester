mod config;
mod executor;
mod heartbeat;
mod ws_client;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Install rustls crypto provider before any TLS operations
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cfg = config::AgentConfig::from_env()?;
    tracing::info!(
        dashboard_url = %cfg.dashboard_url,
        "Networker tester starting"
    );

    // Main reconnect loop
    loop {
        match ws_client::run(&cfg).await {
            Ok(()) => {
                tracing::info!("WebSocket connection closed normally");
            }
            Err(e) => {
                tracing::error!("WebSocket connection error: {e:#}");
            }
        }
        tracing::info!("Reconnecting in 5 seconds...");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}
