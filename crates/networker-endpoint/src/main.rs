use clap::Parser;
use networker_endpoint::{run, ServerConfig};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "networker-endpoint", about = "Diagnostics endpoint for networker-tester", version)]
struct Cli {
    /// HTTP (plain) listening port
    #[arg(long, default_value_t = 8080)]
    http_port: u16,

    /// HTTPS (TLS) listening port (HTTP/1.1 + HTTP/2 via ALPN)
    #[arg(long, default_value_t = 8443)]
    https_port: u16,

    /// UDP echo port
    #[arg(long, default_value_t = 9999)]
    udp_port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = ServerConfig {
        http_port: cli.http_port,
        https_port: cli.https_port,
        udp_port: cli.udp_port,
    };

    run(cfg).await
}
