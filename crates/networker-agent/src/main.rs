mod commands;
mod config;
mod executor;
mod heartbeat;
mod ws_client;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _log_guard = networker_log::LogBuilder::new("agent")
        .with_console(networker_log::Stream::Stderr)
        .init()
        .await?;

    // Install rustls crypto provider before any TLS operations
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cfg = config::AgentConfig::from_env()?;
    tracing::info!(
        dashboard_url = %cfg.dashboard_url,
        "Networker tester starting"
    );

    // Main reconnect loop with graceful shutdown
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("Shutdown signal received");
                break;
            }
            result = ws_client::run(&cfg) => {
                match result {
                    Ok(()) => tracing::info!("WebSocket connection closed normally"),
                    Err(e) => tracing::error!("WebSocket connection error: {e:#}"),
                }
                tracing::info!("Reconnecting in 5 seconds...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }

    Ok(())
}
