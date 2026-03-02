use anyhow::Context;
use clap::Parser;
use serde::Deserialize;

/// Networker Tester – cross-platform network diagnostics client.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "networker-tester",
    about = "Collect detailed network telemetry across TCP/HTTP/UDP",
    version
)]
pub struct Cli {
    /// Path to a JSON config file. CLI flags override values from the file.
    #[arg(long, short = 'c')]
    pub config: Option<String>,

    // ── Target ────────────────────────────────────────────────────────────────
    /// Target URL (e.g. https://host:8443/health)
    #[arg(long)]
    pub target: Option<String>,

    // ── Modes ─────────────────────────────────────────────────────────────────
    /// Comma-separated probe modes:
    /// tcp,http1,http2,http3,udp,download,upload,webdownload,webupload,udpdownload,udpupload,
    /// dns,tls,native,curl,pageload,pageload2,pageload3,browser.
    /// native: DNS + TCP + platform TLS (SChannel/SecureTransport/OpenSSL) + HTTP/1.1.
    ///   Requires --features native at compile time.
    /// curl: DNS + TCP + TLS + HTTP via the system curl binary.
    ///   Captures per-phase timing from curl --write-out.
    /// webdownload: GET /download?bytes=N on the target host (path rewritten to /download),
    ///   measures HTTP phase timing + response body throughput. Requires --payload-sizes.
    /// webupload: POST /upload with N-byte body on the target host (path rewritten to /upload),
    ///   measures HTTP phase timing + upload throughput. Requires --payload-sizes.
    /// udpdownload: UDP bulk download from networker-endpoint (requires --payload-sizes).
    /// udpupload: UDP bulk upload to networker-endpoint (requires --payload-sizes).
    /// pageload: fetch /page manifest then download all assets over up to 6 parallel HTTP/1.1
    ///   connections (browser-like). Use --page-assets and --page-asset-size to configure.
    /// pageload2: same assets multiplexed over a single HTTP/2 TLS connection. Requires HTTPS.
    /// pageload3: same assets multiplexed over a single QUIC/HTTP/3 connection. Requires HTTPS
    ///   and --features http3.
    /// browser: drive a real headless Chromium via CDP; measures load time, DCL, TTFB, resource
    ///   counts, transferred bytes, and per-protocol resource counts. Requires --features browser
    ///   and Chrome/Chromium installed (or NETWORKER_CHROME_PATH env var).
    #[arg(long, value_delimiter = ',')]
    pub modes: Option<Vec<String>>,

    // ── Repetition ────────────────────────────────────────────────────────────
    /// Number of sequential runs per mode
    #[arg(long)]
    pub runs: Option<u32>,

    /// Number of concurrent requests per run (best-effort)
    #[arg(long)]
    pub concurrency: Option<usize>,

    // ── Timing ────────────────────────────────────────────────────────────────
    /// Per-request timeout in seconds
    #[arg(long)]
    pub timeout: Option<u64>,

    // ── Payload ───────────────────────────────────────────────────────────────
    /// Bytes to send in POST /echo requests (0 = GET)
    #[arg(long)]
    pub payload_size: Option<usize>,

    /// Payload sizes for download/upload probes. Comma-separated, accepts k/m/g suffixes.
    /// Required when --modes includes download or upload (e.g. --payload-sizes 4k,64k,1m).
    #[arg(long, value_delimiter = ',')]
    pub payload_sizes: Option<Vec<String>>,

    // ── UDP ───────────────────────────────────────────────────────────────────
    /// UDP echo server port on the target host
    #[arg(long)]
    pub udp_port: Option<u16>,

    /// UDP bulk throughput server port (for udpdownload / udpupload probes)
    #[arg(long)]
    pub udp_throughput_port: Option<u16>,

    /// Number of UDP probe packets per run
    #[arg(long)]
    pub udp_probes: Option<u32>,

    // ── Connection options ────────────────────────────────────────────────────
    /// Reuse a single TCP connection across HTTP requests
    #[arg(long)]
    pub connection_reuse: bool,

    /// Perform DNS resolution (disable to connect by raw IP)
    #[arg(long)]
    pub dns_enabled: Option<bool>,

    /// Prefer IPv4 addresses
    #[arg(long, conflicts_with = "ipv6_only")]
    pub ipv4_only: bool,

    /// Prefer IPv6 addresses
    #[arg(long, conflicts_with = "ipv4_only")]
    pub ipv6_only: bool,

    /// Bypass any system proxy (disables HTTP_PROXY / HTTPS_PROXY env var detection)
    #[arg(long)]
    pub no_proxy: bool,

    /// Explicit HTTP proxy URL (e.g. http://proxy.corp:3128).
    /// Overrides HTTP_PROXY / HTTPS_PROXY env vars.
    #[arg(long)]
    pub proxy: Option<String>,

    /// Path to a PEM-format CA certificate bundle to add to the trust store.
    /// Useful for corporate CAs not in the OS trust store.
    #[arg(long)]
    pub ca_bundle: Option<String>,

    /// Skip TLS certificate verification (useful with self-signed endpoint certs)
    #[arg(long)]
    pub insecure: bool,

    // ── Retry ─────────────────────────────────────────────────────────────────
    /// Retry failed probes up to N times. Each retry increments retry_count on the attempt.
    #[arg(long)]
    pub retries: Option<u32>,

    // ── Output ────────────────────────────────────────────────────────────────
    /// Directory for JSON artifact and HTML report
    #[arg(long)]
    pub output_dir: Option<String>,

    /// HTML report filename (relative to --output-dir)
    #[arg(long)]
    pub html_report: Option<String>,

    /// Path to CSS file embedded as <link> in the HTML report
    #[arg(long)]
    pub css: Option<String>,

    /// Write an Excel (.xlsx) report alongside JSON + HTML.
    #[arg(long)]
    pub excel: bool,

    // ── SQL Server ────────────────────────────────────────────────────────────
    /// Insert results into SQL Server
    #[arg(long)]
    pub save_to_sql: bool,

    /// ADO.NET-style connection string
    /// e.g. "Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true"
    #[arg(long, env = "NETWORKER_SQL_CONN")]
    pub connection_string: Option<String>,

    // ── Misc ──────────────────────────────────────────────────────────────────
    // ── Page-load ─────────────────────────────────────────────────────────────
    /// Number of assets per page-load probe cycle (default: 20, max: 500).
    /// Overridden by --page-preset.
    #[arg(long)]
    pub page_assets: Option<usize>,

    /// Asset size for page-load probes, accepts k/m suffixes (default: 10k).
    /// Overridden by --page-preset.
    #[arg(long)]
    pub page_asset_size: Option<String>,

    /// Named page-load preset (overrides --page-assets and --page-asset-size).
    /// Valid: tiny (100×1KB), small (50×5KB), default (20×10KB),
    ///        medium (10×100KB), large (5×1MB), mixed (30 assets, ~820KB).
    #[arg(long)]
    pub page_preset: Option<String>,

    // ── Misc ──────────────────────────────────────────────────────────────────
    /// Enable verbose output (equivalent to --log-level debug)
    #[arg(long, short)]
    pub verbose: bool,

    /// Log level e.g. "debug", "info,tower_http=debug". Overrides --verbose and RUST_LOG.
    #[arg(long)]
    pub log_level: Option<String>,
}

/// Keys that may appear in a JSON config file.
/// Unknown keys are silently ignored (no `deny_unknown_fields`).
#[derive(Debug, Default, Deserialize)]
pub struct ConfigFile {
    pub target: Option<String>,
    pub modes: Option<Vec<String>>,
    pub runs: Option<u32>,
    pub concurrency: Option<usize>,
    pub timeout: Option<u64>,
    pub payload_size: Option<usize>,
    pub payload_sizes: Option<Vec<String>>,
    pub udp_port: Option<u16>,
    pub udp_throughput_port: Option<u16>,
    pub udp_probes: Option<u32>,
    pub connection_reuse: Option<bool>,
    pub dns_enabled: Option<bool>,
    pub ipv4_only: Option<bool>,
    pub ipv6_only: Option<bool>,
    pub no_proxy: Option<bool>,
    pub proxy: Option<String>,
    pub ca_bundle: Option<String>,
    pub insecure: Option<bool>,
    pub retries: Option<u32>,
    pub output_dir: Option<String>,
    pub html_report: Option<String>,
    pub css: Option<String>,
    pub excel: Option<bool>,
    pub save_to_sql: Option<bool>,
    pub connection_string: Option<String>,
    pub log_level: Option<String>,
    pub page_assets: Option<usize>,
    pub page_asset_size: Option<String>,
    pub page_preset: Option<String>,
}

/// Fully resolved configuration with all defaults applied.
/// Priority: CLI arg > JSON config key > built-in default.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub target: String,
    pub modes: Vec<String>,
    pub runs: u32,
    pub concurrency: usize,
    pub timeout: u64,
    pub payload_size: usize,
    pub payload_sizes: Vec<String>,
    pub udp_port: u16,
    pub udp_throughput_port: u16,
    pub udp_probes: u32,
    pub connection_reuse: bool,
    pub dns_enabled: bool,
    pub ipv4_only: bool,
    pub ipv6_only: bool,
    pub no_proxy: bool,
    pub proxy: Option<String>,
    pub ca_bundle: Option<String>,
    pub insecure: bool,
    pub retries: u32,
    pub output_dir: String,
    pub html_report: String,
    pub css: Option<String>,
    pub excel: bool,
    pub save_to_sql: bool,
    pub connection_string: Option<String>,
    pub log_level: Option<String>,
    /// One entry per asset; value = byte count for that asset.
    pub page_asset_sizes: Vec<usize>,
    /// Display name of the active preset, if any (e.g. "mixed").
    pub page_preset_name: Option<String>,
}

impl Cli {
    /// Merge CLI flags with an optional config file and bake in built-in defaults.
    pub fn resolve(self, file: Option<ConfigFile>) -> ResolvedConfig {
        let f = file.unwrap_or_default();
        // Capture Copy fields before partial moves consume self.
        let verbose = self.verbose;

        macro_rules! pick {
            ($field:ident, $default:expr) => {
                self.$field.or(f.$field).unwrap_or($default)
            };
        }
        macro_rules! flag {
            ($field:ident) => {
                self.$field || f.$field.unwrap_or(false)
            };
        }

        // Pre-compute page-load fields before the struct literal partially moves self/f.
        let page_preset_raw = self.page_preset.or(f.page_preset);
        let page_assets_count = self.page_assets.or(f.page_assets).unwrap_or(20);
        let page_asset_size_bytes = {
            let s = self
                .page_asset_size
                .or(f.page_asset_size)
                .unwrap_or_else(|| "10k".into());
            parse_size(&s).unwrap_or(10_240)
        };
        let (page_asset_sizes, page_preset_name) = match page_preset_raw.as_deref() {
            Some(p) => match crate::runner::pageload::resolve_preset(p) {
                Ok(sizes) => (sizes, Some(p.to_string())),
                Err(_) => (vec![page_asset_size_bytes; page_assets_count], None),
            },
            None => (vec![page_asset_size_bytes; page_assets_count], None),
        };

        ResolvedConfig {
            target: pick!(target, "http://localhost:8080/health".into()),
            modes: pick!(modes, vec!["http1".into(), "http2".into(), "udp".into()]),
            runs: pick!(runs, 3),
            concurrency: pick!(concurrency, 1),
            timeout: pick!(timeout, 30),
            payload_size: pick!(payload_size, 0),
            payload_sizes: pick!(payload_sizes, vec![]),
            udp_port: pick!(udp_port, 9999),
            udp_throughput_port: pick!(udp_throughput_port, 9998),
            udp_probes: pick!(udp_probes, 10),
            connection_reuse: flag!(connection_reuse),
            dns_enabled: self.dns_enabled.or(f.dns_enabled).unwrap_or(true),
            ipv4_only: flag!(ipv4_only),
            ipv6_only: flag!(ipv6_only),
            no_proxy: flag!(no_proxy),
            proxy: self.proxy.or(f.proxy),
            ca_bundle: self.ca_bundle.or(f.ca_bundle),
            insecure: flag!(insecure),
            retries: pick!(retries, 0),
            output_dir: pick!(output_dir, "./output".into()),
            html_report: pick!(html_report, "report.html".into()),
            css: self.css.or(f.css),
            excel: flag!(excel),
            save_to_sql: flag!(save_to_sql),
            connection_string: self.connection_string.or(f.connection_string),
            log_level: self
                .log_level
                .or(f.log_level)
                .or_else(|| verbose.then(|| "debug".into())),
            page_asset_sizes,
            page_preset_name,
        }
    }
}

impl ResolvedConfig {
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
        if self.ipv4_only && self.ipv6_only {
            anyhow::bail!("--ipv4-only and --ipv6-only are mutually exclusive");
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

    pub fn parsed_payload_sizes(&self) -> anyhow::Result<Vec<usize>> {
        self.payload_sizes.iter().map(|s| parse_size(s)).collect()
    }
}

/// Load and deserialize a JSON config file.
pub fn load_config(path: &str) -> anyhow::Result<ConfigFile> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config file: {path}"))?;
    serde_json::from_str(&s).with_context(|| format!("Invalid JSON in config file: {path}"))
}

/// Returns true if the current process is running as root / Administrator.
/// On Unix this checks `getuid() == 0`; on Windows it always returns true
/// (elevated privilege detection requires Windows-specific APIs).
#[cfg(unix)]
pub fn running_as_root() -> bool {
    // SAFETY: getuid() is always safe to call.
    unsafe { libc::getuid() == 0 }
}

#[cfg(not(unix))]
pub fn running_as_root() -> bool {
    true
}

pub(crate) fn parse_size(s: &str) -> anyhow::Result<usize> {
    let s = s.trim().to_lowercase();
    let (num, mul) = if s.ends_with('g') {
        (&s[..s.len() - 1], 1usize << 30)
    } else if s.ends_with('m') {
        (&s[..s.len() - 1], 1usize << 20)
    } else if s.ends_with('k') {
        (&s[..s.len() - 1], 1usize << 10)
    } else {
        (s.as_str(), 1usize)
    };
    let n: usize = num
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid size: {s}"))?;
    n.checked_mul(mul)
        .ok_or_else(|| anyhow::anyhow!("size overflow: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn defaults_parse() {
        let cli = Cli::parse_from(["networker-tester"]);
        assert!(cli.runs.is_none());
        assert!(cli.target.is_none());
        assert!(cli.udp_port.is_none());
        assert!(!cli.insecure);
        assert!(cli.retries.is_none());
        assert!(!cli.excel);
        assert!(cli.udp_throughput_port.is_none());
    }

    #[test]
    fn resolved_defaults() {
        let cfg = Cli::parse_from(["networker-tester"]).resolve(None);
        assert_eq!(cfg.runs, 3);
        assert_eq!(cfg.target, "http://localhost:8080/health");
        assert_eq!(cfg.udp_port, 9999);
        assert!(!cfg.insecure);
        assert_eq!(cfg.retries, 0);
        assert!(!cfg.excel);
        assert_eq!(cfg.udp_throughput_port, 9998);
        assert!(cfg.dns_enabled);
        assert_eq!(cfg.modes, vec!["http1", "http2", "udp"]);
    }

    #[test]
    fn config_file_overrides_defaults() {
        let file = ConfigFile {
            runs: Some(7),
            target: Some("http://myhost/health".into()),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
        assert_eq!(cfg.runs, 7);
        assert_eq!(cfg.target, "http://myhost/health");
    }

    #[test]
    fn cli_overrides_config_file() {
        let file = ConfigFile {
            runs: Some(7),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester", "--runs", "2"]).resolve(Some(file));
        assert_eq!(cfg.runs, 2);
    }

    #[test]
    fn modes_split_by_comma() {
        let cli = Cli::parse_from(["networker-tester", "--modes", "http1,http2,udp"]);
        assert_eq!(
            cli.modes,
            Some(vec![
                "http1".to_string(),
                "http2".to_string(),
                "udp".to_string()
            ])
        );
    }

    #[test]
    fn validate_save_to_sql_without_conn_string_fails() {
        // Skip when NETWORKER_SQL_CONN is set: clap picks it up automatically,
        // so validation would correctly pass — but the test expects an error.
        if std::env::var("NETWORKER_SQL_CONN").is_ok() {
            return;
        }
        let cli = Cli::parse_from(["networker-tester", "--save-to-sql"]);
        assert!(cli.resolve(None).validate().is_err());
    }

    #[test]
    fn parse_size_suffixes() {
        assert_eq!(super::parse_size("4k").unwrap(), 4096);
        assert_eq!(super::parse_size("64k").unwrap(), 65536);
        assert_eq!(super::parse_size("1m").unwrap(), 1048576);
        assert_eq!(super::parse_size("1024").unwrap(), 1024);
        assert!(super::parse_size("abc").is_err());
    }

    #[test]
    fn payload_sizes_parsed_via_cli() {
        let cli = Cli::parse_from(["networker-tester", "--payload-sizes", "4k,64k,1m"]);
        let cfg = cli.resolve(None);
        let sizes = cfg.parsed_payload_sizes().unwrap();
        assert_eq!(sizes, vec![4096, 65536, 1048576]);
    }

    #[test]
    fn parse_size_gigabyte_suffix() {
        assert_eq!(super::parse_size("1g").unwrap(), 1_073_741_824);
        assert_eq!(super::parse_size("2g").unwrap(), 2_147_483_648);
    }

    #[test]
    fn validate_empty_modes_fails() {
        // Supply an empty modes vec via ConfigFile (CLI always has a fallback default).
        let file = ConfigFile {
            modes: Some(vec![]),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn verbose_flag_sets_log_level_debug() {
        let cli = Cli::parse_from(["networker-tester", "--verbose"]);
        let cfg = cli.resolve(None);
        assert_eq!(cfg.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn log_level_flag_overrides_verbose() {
        let cli = Cli::parse_from(["networker-tester", "--verbose", "--log-level", "warn"]);
        let cfg = cli.resolve(None);
        assert_eq!(cfg.log_level.as_deref(), Some("warn"));
    }

    #[test]
    fn parsed_modes_filters_invalid_strings() {
        let cli = Cli::parse_from(["networker-tester", "--modes", "http1,notamode,http2"]);
        let cfg = cli.resolve(None);
        let protos = cfg.parsed_modes();
        assert_eq!(protos.len(), 2);
        use crate::metrics::Protocol;
        assert!(protos.contains(&Protocol::Http1));
        assert!(protos.contains(&Protocol::Http2));
    }

    #[test]
    fn page_preset_tiny_resolves_to_100_assets() {
        let cli = Cli::parse_from(["networker-tester", "--page-preset", "tiny"]);
        let cfg = cli.resolve(None);
        assert_eq!(cfg.page_asset_sizes.len(), 100);
        assert!(cfg.page_asset_sizes.iter().all(|&s| s == 1024));
        assert_eq!(cfg.page_preset_name.as_deref(), Some("tiny"));
    }

    #[test]
    fn page_preset_invalid_falls_back_to_default() {
        let cli = Cli::parse_from([
            "networker-tester",
            "--page-preset",
            "nonexistent",
            "--page-assets",
            "5",
        ]);
        let cfg = cli.resolve(None);
        // invalid preset → falls back to page_assets × page_asset_size
        assert_eq!(cfg.page_asset_sizes.len(), 5);
        assert!(cfg.page_preset_name.is_none());
    }

    #[test]
    fn load_config_nonexistent_path_returns_error() {
        let result = super::load_config("/this/path/does/not/exist.json");
        assert!(result.is_err());
    }
}
