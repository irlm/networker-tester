use anyhow::Context;
use clap::{Parser, Subcommand};
use networker_endpoint::{generate_static_site, run, ServerConfig};
use serde::Deserialize;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    http_port: Option<u16>,
    https_port: Option<u16>,
    udp_port: Option<u16>,
    udp_throughput_port: Option<u16>,
    log_level: Option<String>,
}

fn load_config(path: &str) -> anyhow::Result<ConfigFile> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config file: {path}"))?;
    serde_json::from_str(&s).with_context(|| format!("Invalid JSON in config file: {path}"))
}

#[derive(Parser, Debug)]
#[command(
    name = "networker-endpoint",
    about = "Diagnostics endpoint for networker-tester",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to a JSON config file. CLI flags override values from the file.
    #[arg(long, short = 'c')]
    config: Option<String>,

    /// HTTP (plain) listening port
    #[arg(long)]
    http_port: Option<u16>,

    /// HTTPS (TLS) listening port (HTTP/1.1 + HTTP/2 via ALPN)
    #[arg(long)]
    https_port: Option<u16>,

    /// UDP echo port
    #[arg(long)]
    udp_port: Option<u16>,

    /// UDP bulk throughput server port (for udpdownload / udpupload probes)
    #[arg(long)]
    udp_throughput_port: Option<u16>,

    /// Log level e.g. "debug", "info,tower_http=debug". Overrides RUST_LOG.
    #[arg(long)]
    log_level: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a static test website for nginx/IIS comparison.
    ///
    /// Creates index.html + asset files matching a page-load preset.
    GenerateSite {
        /// Output directory for the static site.
        dir: PathBuf,

        /// Page-load preset (small, default, mixed).
        #[arg(long, default_value = "mixed")]
        preset: String,

        /// HTTP stack name (e.g. "nginx", "iis") — written to health file.
        #[arg(long, default_value = "static")]
        stack: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Handle subcommands first
    if let Some(Command::GenerateSite {
        dir,
        preset,
        stack,
    }) = cli.command
    {
        // Minimal logging for generate-site
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("info"))
            .init();
        return generate_static_site(&dir, &preset, &stack);
    }

    // Default: run the server
    let f = cli
        .config
        .as_deref()
        .map(load_config)
        .transpose()?
        .unwrap_or_default();

    let http_port = cli.http_port.or(f.http_port).unwrap_or(8080);
    let https_port = cli.https_port.or(f.https_port).unwrap_or(8443);
    let udp_port = cli.udp_port.or(f.udp_port).unwrap_or(9999);
    let udp_tp_port = cli
        .udp_throughput_port
        .or(f.udp_throughput_port)
        .unwrap_or(9998);
    let log_level = cli.log_level.or(f.log_level);

    let log_filter = if let Some(ref level) = log_level {
        EnvFilter::new(level)
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    let cfg = ServerConfig {
        http_port,
        https_port,
        udp_port,
        udp_throughput_port: udp_tp_port,
    };

    run(cfg).await
}
