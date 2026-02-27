use clap::Parser;

/// Networker Tester – cross-platform network diagnostics client.
#[derive(Parser, Debug, Clone)]
#[command(name = "networker-tester", about = "Collect detailed network telemetry across TCP/HTTP/UDP", version)]
pub struct Cli {
    // ── Target ────────────────────────────────────────────────────────────────
    /// Target URL (e.g. https://host:8443/health)
    #[arg(long, default_value = "http://localhost:8080/health")]
    pub target: String,

    // ── Modes ─────────────────────────────────────────────────────────────────
    /// Comma-separated probe modes: tcp,http1,http2,http3,udp
    #[arg(long, value_delimiter = ',', default_value = "http1,http2,udp")]
    pub modes: Vec<String>,

    // ── Repetition ────────────────────────────────────────────────────────────
    /// Number of sequential runs per mode
    #[arg(long, default_value_t = 3)]
    pub runs: u32,

    /// Number of concurrent requests per run (best-effort)
    #[arg(long, default_value_t = 1)]
    pub concurrency: usize,

    // ── Timing ────────────────────────────────────────────────────────────────
    /// Per-request timeout in seconds
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,

    // ── Payload ───────────────────────────────────────────────────────────────
    /// Bytes to send in POST /echo requests (0 = GET)
    #[arg(long, default_value_t = 0)]
    pub payload_size: usize,

    // ── UDP ───────────────────────────────────────────────────────────────────
    /// UDP echo server port on the target host
    #[arg(long, default_value_t = 9999)]
    pub udp_port: u16,

    /// Number of UDP probe packets per run
    #[arg(long, default_value_t = 10)]
    pub udp_probes: u32,

    // ── Connection options ────────────────────────────────────────────────────
    /// Reuse a single TCP connection across HTTP requests
    #[arg(long, default_value_t = false)]
    pub connection_reuse: bool,

    /// Perform DNS resolution (disable to connect by raw IP)
    #[arg(long, default_value_t = true)]
    pub dns_enabled: bool,

    /// Prefer IPv4 addresses
    #[arg(long, conflicts_with = "ipv6_only")]
    pub ipv4_only: bool,

    /// Prefer IPv6 addresses
    #[arg(long, conflicts_with = "ipv4_only")]
    pub ipv6_only: bool,

    /// Bypass any system proxy
    #[arg(long)]
    pub no_proxy: bool,

    /// Skip TLS certificate verification (useful with self-signed endpoint certs)
    #[arg(long)]
    pub insecure: bool,

    // ── Output ────────────────────────────────────────────────────────────────
    /// Directory for JSON artifact and HTML report
    #[arg(long, default_value = "./output")]
    pub output_dir: String,

    /// HTML report filename (relative to --output-dir)
    #[arg(long, default_value = "report.html")]
    pub html_report: String,

    /// Path to CSS file embedded as <link> in the HTML report
    #[arg(long)]
    pub css: Option<String>,

    // ── SQL Server ────────────────────────────────────────────────────────────
    /// Insert results into SQL Server
    #[arg(long)]
    pub save_to_sql: bool,

    /// ADO.NET-style connection string
    /// e.g. "Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true"
    #[arg(long, env = "NETWORKER_SQL_CONN")]
    pub connection_string: Option<String>,

    // ── Misc ──────────────────────────────────────────────────────────────────
    /// Enable verbose output
    #[arg(long, short)]
    pub verbose: bool,
}

impl Cli {
    /// Validate combinations of flags; return user-friendly errors.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.save_to_sql && self.connection_string.is_none() {
            anyhow::bail!(
                "--save-to-sql requires --connection-string (or NETWORKER_SQL_CONN env var)"
            );
        }
        if self.modes.is_empty() {
            anyhow::bail!("At least one --modes value is required");
        }
        Ok(())
    }

    pub fn parsed_modes(&self) -> Vec<crate::metrics::Protocol> {
        use std::str::FromStr;
        self.modes
            .iter()
            .filter_map(|m| crate::metrics::Protocol::from_str(m).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn defaults_parse() {
        let cli = Cli::parse_from(["networker-tester"]);
        assert_eq!(cli.runs, 3);
        assert_eq!(cli.udp_port, 9999);
        assert_eq!(cli.payload_size, 0);
        assert!(!cli.insecure);
    }

    #[test]
    fn modes_split_by_comma() {
        let cli = Cli::parse_from(["networker-tester", "--modes", "http1,http2,udp"]);
        assert_eq!(cli.modes, vec!["http1", "http2", "udp"]);
    }

    #[test]
    fn validate_save_to_sql_without_conn_string_fails() {
        let cli = Cli::parse_from(["networker-tester", "--save-to-sql"]);
        assert!(cli.validate().is_err());
    }
}
