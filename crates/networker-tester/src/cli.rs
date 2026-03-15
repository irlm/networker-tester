use anyhow::Context;
use clap::Parser;
use serde::{Deserialize, Serialize};

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
    /// Target URL(s) to test. Repeat the flag for multiple targets:
    /// --target https://local/health --target https://remote/health
    #[arg(long, action = clap::ArgAction::Append, num_args = 1)]
    pub target: Vec<String>,

    // ── Modes ─────────────────────────────────────────────────────────────────
    /// Comma-separated probe modes:
    /// tcp,http1,http2,http3,udp,download,download1,download2,download3,upload,upload1,upload2,upload3,webdownload,webupload,udpdownload,udpupload,
    /// dns,tls,native,curl,pageload,pageload1,pageload2,pageload3,browser,browser1,browser2,browser3.
    /// pageload: shorthand that runs pageload1+pageload2+pageload3 (all three HTTP versions).
    /// pageload1: HTTP/1.1 page-load (same as the original pageload single-version mode).
    /// browser: shorthand that runs browser1+browser2+browser3 (all three HTTP versions).
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
    /// browser1: same as browser but forced to HTTP/1.1 (--disable-http2).
    /// browser2: same as browser but forced to HTTP/2 (--disable-quic). Requires HTTPS.
    /// browser3: same as browser but forced to HTTP/3 QUIC (--enable-quic). Requires HTTPS.
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
    /// Reuse connections across pageload runs (warmup + warm probes).
    /// A warmup probe establishes the connection (cold); subsequent runs reuse it (warm).
    /// Applies to pageload2 (HTTP/2) and pageload3 (HTTP/3). Compare cold vs warm in the report.
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

    // ── Database ──────────────────────────────────────────────────────────────
    /// Insert results into a database (auto-detects backend from URL scheme)
    #[arg(long)]
    pub save_to_db: bool,

    /// Database URL (postgres://..., or ADO.NET-style for SQL Server)
    #[arg(long, env = "NETWORKER_DB_URL")]
    pub db_url: Option<String>,

    /// Run database migrations before inserting
    #[arg(long)]
    pub db_migrate: bool,

    // ── SQL Server (legacy aliases, hidden) ──────────────────────────────────
    /// Insert results into SQL Server (legacy alias for --save-to-db)
    #[arg(long, hide = true)]
    pub save_to_sql: bool,

    /// ADO.NET-style connection string (legacy alias for --db-url)
    #[arg(long, env = "NETWORKER_SQL_CONN", hide = true)]
    pub connection_string: Option<String>,

    // ── Misc ──────────────────────────────────────────────────────────────────
    // ── Page-load ─────────────────────────────────────────────────────────────
    /// Number of assets per page-load probe cycle (default: 50, max: 500).
    /// Overridden by --page-preset.
    #[arg(long)]
    pub page_assets: Option<usize>,

    /// Asset size for page-load probes, accepts k/m suffixes (default: 50k).
    /// Overridden by --page-preset.
    #[arg(long)]
    pub page_asset_size: Option<String>,

    /// Named page-load preset (overrides --page-assets and --page-asset-size).
    /// Valid: tiny (10 assets, ~100KB), small (25 assets, ~800KB),
    ///        default (50 assets, ~4MB), medium (100 assets, ~8MB),
    ///        large (200 assets, ~16MB), mixed (50 assets, ~4MB).
    #[arg(long)]
    pub page_preset: Option<String>,

    // ── HTTP Stacks ────────────────────────────────────────────────────────
    /// Compare HTTP stacks: run browser/pageload probes against additional
    /// servers installed on the same VM. Comma-separated list.
    /// Valid: nginx, iis (e.g. --http-stacks nginx,iis)
    #[arg(long, value_delimiter = ',')]
    pub http_stacks: Option<Vec<String>>,

    // ── Misc ──────────────────────────────────────────────────────────────────
    /// Enable verbose output (equivalent to --log-level debug)
    #[arg(long, short)]
    pub verbose: bool,

    /// Log level e.g. "debug", "info,tower_http=debug". Overrides --verbose and RUST_LOG.
    #[arg(long)]
    pub log_level: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PacketCaptureMode {
    #[default]
    None,
    Tester,
    Endpoint,
    Both,
}

impl PacketCaptureMode {
    pub fn captures_tester(self) -> bool {
        matches!(self, Self::Tester | Self::Both)
    }

    pub fn captures_endpoint(self) -> bool {
        matches!(self, Self::Endpoint | Self::Both)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PacketCaptureConfig {
    pub mode: Option<PacketCaptureMode>,
    pub install_requirements: Option<bool>,
    pub interface: Option<String>,
    pub write_pcap: Option<bool>,
    pub write_summary_json: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPacketCaptureConfig {
    pub mode: PacketCaptureMode,
    pub install_requirements: bool,
    pub interface: String,
    pub write_pcap: bool,
    pub write_summary_json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpairmentProfile {
    #[default]
    None,
    Wan,
    Slow,
    Satellite,
}

impl ImpairmentProfile {
    pub fn default_delay_ms(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Wan => 40,
            Self::Slow => 150,
            Self::Satellite => 600,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ImpairmentConfig {
    pub profile: Option<ImpairmentProfile>,
    pub delay_ms: Option<u64>,
}

pub const MAX_IMPAIRMENT_DELAY_MS: u64 = 10_000;

#[derive(Debug, Clone)]
pub struct ResolvedImpairmentConfig {
    pub profile: ImpairmentProfile,
    pub delay_ms: u64,
}

/// Keys that may appear in a JSON config file.
/// Unknown keys are silently ignored (no `deny_unknown_fields`).
#[derive(Debug, Default, Deserialize)]
pub struct ConfigFile {
    pub target: Option<String>,       // kept for backward compat
    pub targets: Option<Vec<String>>, // list of targets; merged with CLI --target flags
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
    pub save_to_db: Option<bool>,
    pub db_url: Option<String>,
    pub db_migrate: Option<bool>,
    pub save_to_sql: Option<bool>,
    pub connection_string: Option<String>,
    pub log_level: Option<String>,
    pub page_assets: Option<usize>,
    pub page_asset_size: Option<String>,
    pub page_preset: Option<String>,
    pub http_stacks: Option<Vec<String>>,
    pub packet_capture: Option<PacketCaptureConfig>,
    pub impairment: Option<ImpairmentConfig>,
}

/// Fully resolved configuration with all defaults applied.
/// Priority: CLI arg > JSON config key > built-in default.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// One or more target URLs to probe. Always non-empty (defaults to localhost).
    pub targets: Vec<String>,
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
    pub save_to_db: bool,
    pub db_url: Option<String>,
    pub db_migrate: bool,
    pub save_to_sql: bool,
    pub connection_string: Option<String>,
    pub log_level: Option<String>,
    /// One entry per asset; value = byte count for that asset.
    pub page_asset_sizes: Vec<usize>,
    /// Display name of the active preset, if any (e.g. "mixed").
    pub page_preset_name: Option<String>,
    /// HTTP stacks to compare (e.g. ["nginx", "iis"]).
    pub http_stacks: Vec<HttpStack>,
    pub packet_capture: ResolvedPacketCaptureConfig,
    pub impairment: ResolvedImpairmentConfig,
}

/// An HTTP stack to probe alongside the default networker-endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpStack {
    pub name: String,
    pub http_port: u16,
    pub https_port: u16,
}

impl HttpStack {
    pub fn from_name(name: &str) -> anyhow::Result<Self> {
        match name.to_lowercase().as_str() {
            "nginx" => Ok(Self {
                name: "nginx".into(),
                http_port: 8081,
                https_port: 8444,
            }),
            "iis" => Ok(Self {
                name: "iis".into(),
                http_port: 8082,
                https_port: 8445,
            }),
            "caddy" => Ok(Self {
                name: "caddy".into(),
                http_port: 8083,
                https_port: 8446,
            }),
            "apache" => Ok(Self {
                name: "apache".into(),
                http_port: 8084,
                https_port: 8447,
            }),
            other => Err(anyhow::anyhow!(
                "Unknown HTTP stack '{other}'. Valid: nginx, iis, caddy, apache"
            )),
        }
    }
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
        let page_assets_count = self.page_assets.or(f.page_assets).unwrap_or(50);
        let page_asset_size_bytes = {
            let s = self
                .page_asset_size
                .or(f.page_asset_size)
                .unwrap_or_else(|| "50k".into());
            parse_size(&s).unwrap_or(51_200)
        };
        let (page_asset_sizes, page_preset_name) = match page_preset_raw.as_deref() {
            Some(p) => match crate::runner::pageload::resolve_preset(p) {
                Ok(sizes) => (sizes, Some(p.to_string())),
                Err(_) => (vec![page_asset_size_bytes; page_assets_count], None),
            },
            None => (vec![page_asset_size_bytes; page_assets_count], None),
        };

        let packet_capture = {
            let pc = f.packet_capture.unwrap_or_default();
            ResolvedPacketCaptureConfig {
                mode: pc.mode.unwrap_or_default(),
                install_requirements: pc.install_requirements.unwrap_or(false),
                interface: pc.interface.unwrap_or_else(|| "auto".into()),
                write_pcap: pc.write_pcap.unwrap_or(true),
                write_summary_json: pc.write_summary_json.unwrap_or(true),
            }
        };
        let impairment = {
            let ic = f.impairment.unwrap_or_default();
            let profile = ic.profile.unwrap_or_default();
            let requested_delay = ic.delay_ms.unwrap_or_else(|| profile.default_delay_ms());
            ResolvedImpairmentConfig {
                profile,
                delay_ms: requested_delay.min(MAX_IMPAIRMENT_DELAY_MS),
            }
        };

        ResolvedConfig {
            targets: {
                // CLI --target flags take priority (already a Vec); then config `targets`
                // list; then the legacy single `target` key; finally the built-in default.
                let mut ts: Vec<String> = self.target;
                if let Some(ref ts2) = f.targets {
                    for t in ts2 {
                        if !ts.contains(t) {
                            ts.push(t.clone());
                        }
                    }
                }
                if let Some(ref t) = f.target {
                    if !ts.contains(t) {
                        ts.push(t.clone());
                    }
                }
                if ts.is_empty() {
                    ts.push("http://localhost:8080/health".into());
                }
                ts
            },
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
            save_to_db: self.save_to_db
                || f.save_to_db.unwrap_or(false)
                || self.save_to_sql
                || f.save_to_sql.unwrap_or(false),
            db_url: self.db_url.or(f.db_url).or_else(|| {
                self.connection_string
                    .clone()
                    .or_else(|| f.connection_string.clone())
            }),
            db_migrate: self.db_migrate || f.db_migrate.unwrap_or(false),
            save_to_sql: flag!(save_to_sql),
            connection_string: self.connection_string.or(f.connection_string),
            log_level: self
                .log_level
                .or(f.log_level)
                .or_else(|| verbose.then(|| "debug".into())),
            page_asset_sizes,
            page_preset_name,
            http_stacks: {
                let raw = self.http_stacks.or(f.http_stacks).unwrap_or_default();
                raw.iter()
                    .filter_map(|s| HttpStack::from_name(s).ok())
                    .collect()
            },
            packet_capture,
            impairment,
        }
    }
}

impl ResolvedConfig {
    /// Validate combinations of flags; return user-friendly errors.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.save_to_db && self.db_url.is_none() && !self.save_to_sql {
            anyhow::bail!("--save-to-db requires --db-url (or NETWORKER_DB_URL env var)");
        }
        if self.save_to_sql && self.connection_string.is_none() && self.db_url.is_none() {
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
        use crate::metrics::Protocol;

        // Aggregate shorthand modes:
        //   "pageload"  → [pageload H1.1, pageload2 H2, pageload3 H3]
        //   "pageload1" → [pageload H1.1]  (explicit H1.1 alias)
        //   "browser"   → [browser1 H1.1, browser2 H2, browser3 H3]
        let expand = |s: &str| -> Vec<Protocol> {
            match s.to_lowercase().as_str() {
                "pageload" => {
                    vec![Protocol::PageLoad, Protocol::PageLoad2, Protocol::PageLoad3]
                }
                "pageload1" => vec![Protocol::PageLoad],
                "browser" => {
                    vec![Protocol::Browser1, Protocol::Browser2, Protocol::Browser3]
                }
                other => match other.parse::<Protocol>() {
                    Ok(p) => vec![p],
                    Err(_) => vec![],
                },
            }
        };

        // Expand and deduplicate while preserving order.
        let mut seen = std::collections::HashSet::new();
        self.modes
            .iter()
            .flat_map(|m| expand(m))
            .filter(|p| seen.insert(p.clone()))
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
        assert!(cli.target.is_empty());
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
        assert_eq!(cfg.targets, vec!["http://localhost:8080/health"]);
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
        assert_eq!(cfg.targets, vec!["http://myhost/health"]);
    }

    #[test]
    fn multi_target_from_cli() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--target",
            "http://host1/health",
            "--target",
            "http://host2/health",
        ])
        .resolve(None);
        assert_eq!(
            cfg.targets,
            vec!["http://host1/health", "http://host2/health"]
        );
    }

    #[test]
    fn multi_target_from_config_file() {
        let file = ConfigFile {
            targets: Some(vec![
                "http://host1/health".into(),
                "http://host2/health".into(),
            ]),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
        assert_eq!(
            cfg.targets,
            vec!["http://host1/health", "http://host2/health"]
        );
    }

    #[test]
    fn cli_targets_override_config_targets() {
        let file = ConfigFile {
            targets: Some(vec!["http://config/health".into()]),
            ..Default::default()
        };
        // CLI --target takes priority; config target is appended if not a dupe
        let cfg = Cli::parse_from(["networker-tester", "--target", "http://cli/health"])
            .resolve(Some(file));
        assert_eq!(cfg.targets[0], "http://cli/health");
        assert_eq!(cfg.targets[1], "http://config/health");
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
    fn page_preset_tiny_resolves_to_10_assets() {
        let cli = Cli::parse_from(["networker-tester", "--page-preset", "tiny"]);
        let cfg = cli.resolve(None);
        assert_eq!(cfg.page_asset_sizes.len(), 10);
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

    fn with_env_var_cleared<T>(key: &str, f: impl FnOnce() -> T) -> T {
        use std::sync::{Mutex, OnceLock};

        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock poisoned");

        let saved = std::env::var(key).ok();
        std::env::remove_var(key);
        let result = f();
        match saved {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        result
    }

    // ── Database flag tests ────────────────────────────────────────────────────

    #[test]
    fn save_to_db_flag_parsed() {
        let cli = Cli::parse_from(["networker-tester", "--save-to-db"]);
        assert!(cli.save_to_db);
        assert!(!cli.save_to_sql);
    }

    #[test]
    fn db_url_flag_parsed() {
        let cli = Cli::parse_from(["networker-tester", "--db-url", "postgres://localhost/diag"]);
        assert_eq!(cli.db_url.as_deref(), Some("postgres://localhost/diag"));
    }

    #[test]
    fn db_migrate_flag_parsed() {
        let cli = Cli::parse_from(["networker-tester", "--db-migrate"]);
        assert!(cli.db_migrate);
    }

    #[test]
    fn validate_save_to_db_without_db_url_fails() {
        // Skip if NETWORKER_DB_URL is set in the environment — clap picks it up
        // automatically so validate() would correctly pass.
        if std::env::var("NETWORKER_DB_URL").is_ok() {
            return;
        }
        let cfg = Cli::parse_from(["networker-tester", "--save-to-db"]).resolve(None);
        assert!(
            cfg.validate().is_err(),
            "--save-to-db without --db-url should fail validation"
        );
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("--db-url"),
            "error should mention --db-url"
        );
    }

    #[test]
    fn validate_save_to_db_with_db_url_passes() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--save-to-db",
            "--db-url",
            "postgres://localhost/diag",
        ])
        .resolve(None);
        assert!(
            cfg.validate().is_ok(),
            "--save-to-db with --db-url should pass validation"
        );
    }

    #[test]
    fn save_to_sql_hidden_alias_still_parses() {
        // --save-to-sql is a hidden alias for --save-to-db
        let cli = Cli::parse_from(["networker-tester", "--save-to-sql"]);
        assert!(cli.save_to_sql, "save_to_sql field should be set");
    }

    #[test]
    fn resolve_save_to_db_true_when_save_to_sql_set() {
        // save_to_db in ResolvedConfig is true when either --save-to-db or
        // --save-to-sql is passed on the CLI.
        let cfg = Cli::parse_from([
            "networker-tester",
            "--save-to-sql",
            "--db-url",
            "Server=localhost;Database=D;User Id=sa;Password=P",
        ])
        .resolve(None);
        assert!(
            cfg.save_to_db,
            "save_to_db should be true when --save-to-sql is used"
        );
    }

    #[test]
    fn resolve_db_url_falls_back_to_connection_string() {
        with_env_var_cleared("NETWORKER_DB_URL", || {
            // When --db-url is absent, the legacy --connection-string should be used
            // as the db_url in the resolved config.
            let cfg = Cli::parse_from([
                "networker-tester",
                "--connection-string",
                "Server=localhost;Database=D;User Id=sa;Password=P",
            ])
            .resolve(None);
            assert_eq!(
                cfg.db_url.as_deref(),
                Some("Server=localhost;Database=D;User Id=sa;Password=P"),
                "db_url should fall back to --connection-string"
            );
        });
    }

    #[test]
    fn resolve_db_url_takes_priority_over_connection_string() {
        // When both --db-url and --connection-string are present, --db-url wins.
        let cfg = Cli::parse_from([
            "networker-tester",
            "--db-url",
            "postgres://primary",
            "--connection-string",
            "Server=fallback",
        ])
        .resolve(None);
        assert_eq!(
            cfg.db_url.as_deref(),
            Some("postgres://primary"),
            "--db-url should take priority over --connection-string"
        );
    }

    #[test]
    fn resolve_db_migrate_from_config_file() {
        let file = ConfigFile {
            db_migrate: Some(true),
            db_url: Some("postgres://localhost/diag".into()),
            save_to_db: Some(true),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
        assert!(cfg.db_migrate, "db_migrate should be true from config file");
    }

    #[test]
    fn resolve_save_to_db_from_config_file() {
        with_env_var_cleared("NETWORKER_DB_URL", || {
            let file = ConfigFile {
                save_to_db: Some(true),
                db_url: Some("postgres://localhost/diag".into()),
                ..Default::default()
            };
            let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
            assert!(cfg.save_to_db, "save_to_db should be true from config file");
            assert_eq!(cfg.db_url.as_deref(), Some("postgres://localhost/diag"));
        });
    }

    #[test]
    fn impairment_delay_is_clamped_to_maximum() {
        let file = ConfigFile {
            impairment: Some(ImpairmentConfig {
                profile: Some(ImpairmentProfile::Satellite),
                delay_ms: Some(MAX_IMPAIRMENT_DELAY_MS + 1234),
            }),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
        assert_eq!(cfg.impairment.delay_ms, MAX_IMPAIRMENT_DELAY_MS);
    }

    #[test]
    fn resolve_save_to_db_or_merge_with_config_save_to_sql() {
        // Config file sets save_to_sql = true; that should propagate into
        // save_to_db on the resolved config.
        let file = ConfigFile {
            save_to_sql: Some(true),
            connection_string: Some("Server=localhost;Database=D;User Id=sa;Password=P".into()),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
        assert!(
            cfg.save_to_db,
            "save_to_db should be true when config save_to_sql is set"
        );
    }

    #[test]
    fn config_file_db_url_used_when_cli_absent() {
        with_env_var_cleared("NETWORKER_DB_URL", || {
            let file = ConfigFile {
                db_url: Some("postgres://config-host/diag".into()),
                ..Default::default()
            };
            let cfg = Cli::parse_from(["networker-tester"]).resolve(Some(file));
            assert_eq!(cfg.db_url.as_deref(), Some("postgres://config-host/diag"));
        });
    }

    #[test]
    fn cli_db_url_overrides_config_file_db_url() {
        let file = ConfigFile {
            db_url: Some("postgres://config-host/diag".into()),
            ..Default::default()
        };
        let cfg = Cli::parse_from(["networker-tester", "--db-url", "postgres://cli-host/diag"])
            .resolve(Some(file));
        assert_eq!(
            cfg.db_url.as_deref(),
            Some("postgres://cli-host/diag"),
            "CLI --db-url should override config file db_url"
        );
    }

    #[test]
    fn db_migrate_false_by_default() {
        let cfg = Cli::parse_from(["networker-tester"]).resolve(None);
        assert!(!cfg.db_migrate, "--db-migrate should default to false");
    }

    #[test]
    fn save_to_db_false_by_default() {
        let cfg = Cli::parse_from(["networker-tester"]).resolve(None);
        assert!(!cfg.save_to_db, "--save-to-db should default to false");
    }

    #[test]
    fn db_url_none_by_default() {
        // Skip if NETWORKER_DB_URL is set so the env-var based test doesn't
        // accidentally fail here.
        if std::env::var("NETWORKER_DB_URL").is_ok() {
            return;
        }
        let cfg = Cli::parse_from(["networker-tester"]).resolve(None);
        assert!(cfg.db_url.is_none(), "--db-url should default to None");
    }

    // ── HttpStack::from_name() tests ──────────────────────────────────────────

    #[test]
    fn http_stack_nginx_ports() {
        let stack = HttpStack::from_name("nginx").unwrap();
        assert_eq!(stack.name, "nginx");
        assert_eq!(stack.http_port, 8081);
        assert_eq!(stack.https_port, 8444);
    }

    #[test]
    fn http_stack_iis_ports() {
        let stack = HttpStack::from_name("iis").unwrap();
        assert_eq!(stack.name, "iis");
        assert_eq!(stack.http_port, 8082);
        assert_eq!(stack.https_port, 8445);
    }

    #[test]
    fn http_stack_caddy_ports() {
        let stack = HttpStack::from_name("caddy").unwrap();
        assert_eq!(stack.name, "caddy");
        assert_eq!(stack.http_port, 8083);
        assert_eq!(stack.https_port, 8446);
    }

    #[test]
    fn http_stack_apache_ports() {
        let stack = HttpStack::from_name("apache").unwrap();
        assert_eq!(stack.name, "apache");
        assert_eq!(stack.http_port, 8084);
        assert_eq!(stack.https_port, 8447);
    }

    #[test]
    fn http_stack_nginx_case_insensitive_upper() {
        let stack = HttpStack::from_name("NGINX").unwrap();
        assert_eq!(stack.name, "nginx");
        assert_eq!(stack.http_port, 8081);
        assert_eq!(stack.https_port, 8444);
    }

    #[test]
    fn http_stack_iis_case_insensitive_mixed() {
        let stack = HttpStack::from_name("Iis").unwrap();
        assert_eq!(stack.name, "iis");
        assert_eq!(stack.http_port, 8082);
        assert_eq!(stack.https_port, 8445);
    }

    #[test]
    fn http_stack_caddy_case_insensitive_upper() {
        let stack = HttpStack::from_name("CADDY").unwrap();
        assert_eq!(stack.name, "caddy");
        assert_eq!(stack.https_port, 8446);
    }

    #[test]
    fn http_stack_apache_case_insensitive_mixed() {
        let stack = HttpStack::from_name("Apache").unwrap();
        assert_eq!(stack.name, "apache");
        assert_eq!(stack.https_port, 8447);
    }

    #[test]
    fn http_stack_unknown_name_returns_error() {
        let result = HttpStack::from_name("haproxy");
        assert!(result.is_err(), "unknown stack name should return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("haproxy"),
            "error message should mention the unknown name; got: {msg}"
        );
    }

    #[test]
    fn http_stack_empty_string_returns_error() {
        let result = HttpStack::from_name("");
        assert!(result.is_err(), "empty stack name should return Err");
    }

    #[test]
    fn http_stack_all_names_produce_distinct_ports() {
        // Each stack must have a unique HTTP port and a unique HTTPS port —
        // port collisions would cause endpoint conflicts at runtime.
        let stacks: Vec<HttpStack> = ["nginx", "iis", "caddy", "apache"]
            .iter()
            .map(|n| HttpStack::from_name(n).unwrap())
            .collect();

        let http_ports: std::collections::HashSet<u16> =
            stacks.iter().map(|s| s.http_port).collect();
        let https_ports: std::collections::HashSet<u16> =
            stacks.iter().map(|s| s.https_port).collect();

        assert_eq!(http_ports.len(), 4, "all HTTP ports must be distinct");
        assert_eq!(https_ports.len(), 4, "all HTTPS ports must be distinct");
    }

    #[test]
    fn http_stacks_from_cli_flag_resolves_correctly() {
        let cfg = Cli::parse_from(["networker-tester", "--http-stacks", "nginx,iis"]).resolve(None);
        assert_eq!(cfg.http_stacks.len(), 2);
        assert_eq!(cfg.http_stacks[0].name, "nginx");
        assert_eq!(cfg.http_stacks[1].name, "iis");
    }

    #[test]
    fn http_stacks_unknown_name_silently_skipped_in_resolve() {
        // HttpStack::from_name returns Err for unknowns; resolve() uses filter_map
        // so invalid names are silently dropped rather than aborting.
        let cfg =
            Cli::parse_from(["networker-tester", "--http-stacks", "nginx,notastack"]).resolve(None);
        assert_eq!(
            cfg.http_stacks.len(),
            1,
            "unknown stack name should be silently skipped during resolve"
        );
        assert_eq!(cfg.http_stacks[0].name, "nginx");
    }

    #[test]
    fn http_stacks_empty_by_default() {
        let cfg = Cli::parse_from(["networker-tester"]).resolve(None);
        assert!(
            cfg.http_stacks.is_empty(),
            "--http-stacks should default to empty"
        );
    }
}
