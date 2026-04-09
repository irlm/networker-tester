use anyhow::Context;
use clap::{Parser, ValueEnum};
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
    /// dns,tls,tlsresume,native,curl,pageload,pageload1,pageload2,pageload3,browser,browser1,browser2,browser3.
    /// pageload: shorthand that runs pageload1+pageload2+pageload3 (all three HTTP versions).
    /// pageload1: HTTP/1.1 page-load (same as the original pageload single-version mode).
    /// browser: shorthand that runs browser1+browser2+browser3 (all three HTTP versions).
    /// tlsresume: two fresh TLS handshakes to the same origin with a real HTTP request on each;
    ///   the probe passes when the second handshake resumes.
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

    /// Back-compat packet capture mode override (deprecated; use packet_capture.mode in config).
    #[arg(long)]
    pub capture_mode: Option<String>,

    /// Write TestRun JSON to stdout (for agent/automation integration).
    /// Suppresses normal file output when used.
    #[arg(long)]
    pub json_stdout: bool,

    /// Emit a normalized benchmark contract in JSON stdout mode.
    /// Intended for benchmark orchestration and comparison workflows.
    #[arg(long)]
    pub benchmark_mode: bool,

    /// Primary benchmark phase for this invocation.
    /// Valid: environment-check, stability-check, pilot, warmup, measured, cooldown, overhead.
    #[arg(long)]
    pub benchmark_phase: Option<String>,

    /// Scenario label for this benchmark invocation (for example cold or warm).
    #[arg(long)]
    pub benchmark_scenario: Option<String>,

    /// Launch index for repeated benchmark invocations coordinated by an orchestrator.
    #[arg(long)]
    pub benchmark_launch_index: Option<u32>,

    /// Minimum measured samples per benchmark case before adaptive stopping is allowed.
    /// Defaults to `--runs` when omitted.
    #[arg(long)]
    pub benchmark_min_samples: Option<u32>,

    /// Maximum measured samples per benchmark case for adaptive benchmark stopping.
    /// Defaults to `--runs` when omitted.
    #[arg(long)]
    pub benchmark_max_samples: Option<u32>,

    /// Minimum measured wall-clock duration in milliseconds before adaptive stopping is allowed.
    #[arg(long)]
    pub benchmark_min_duration_ms: Option<u64>,

    /// Target maximum relative error for the median primary metric.
    /// Interpreted as half-width of the 95% CI divided by the median.
    #[arg(long)]
    pub benchmark_target_relative_error: Option<f64>,

    /// Target maximum absolute error for the median primary metric.
    /// Interpreted as half-width of the 95% CI in the workload's native metric units.
    #[arg(long)]
    pub benchmark_target_absolute_error: Option<f64>,

    /// Minimum pilot samples per benchmark case before deriving a measured plan.
    #[arg(long)]
    pub benchmark_pilot_min_samples: Option<u32>,

    /// Maximum pilot samples per benchmark case when estimating a measured plan.
    #[arg(long)]
    pub benchmark_pilot_max_samples: Option<u32>,

    /// Minimum pilot wall-clock duration in milliseconds before deriving a measured plan.
    #[arg(long)]
    pub benchmark_pilot_min_duration_ms: Option<u64>,

    /// Number of RTT probes to run during the internal environment-check phase.
    #[arg(long)]
    pub benchmark_environment_check_samples: Option<u32>,

    /// Delay between RTT probes in the internal environment-check phase.
    #[arg(long)]
    pub benchmark_environment_check_interval_ms: Option<u64>,

    /// Number of RTT probes to run during the internal stability-check phase.
    #[arg(long)]
    pub benchmark_stability_check_samples: Option<u32>,

    /// Delay between RTT probes in the internal stability-check phase.
    #[arg(long)]
    pub benchmark_stability_check_interval_ms: Option<u64>,

    /// Maximum packet loss percentage allowed for publication-ready benchmark claims.
    #[arg(long)]
    pub benchmark_max_packet_loss_percent: Option<f64>,

    /// Maximum stability-check jitter ratio (jitter / RTT p50) allowed for publication-ready claims.
    #[arg(long)]
    pub benchmark_max_jitter_ratio: Option<f64>,

    /// Maximum RTT spread ratio (RTT p95 / RTT p50) allowed for publication-ready benchmark claims.
    #[arg(long)]
    pub benchmark_max_rtt_spread_ratio: Option<f64>,

    /// Number of excluded overhead iterations to collect before the measured phase.
    #[arg(long)]
    pub benchmark_overhead_samples: Option<u32>,

    /// Number of excluded cooldown iterations to collect after the measured phase.
    #[arg(long)]
    pub benchmark_cooldown_samples: Option<u32>,

    // ── Benchmark progress reporting ────────────────────────────────────────
    /// URL to POST per-request progress (used by orchestrator integration)
    #[arg(long, hide = true)]
    pub progress_url: Option<String>,

    /// Bearer token for progress URL authentication
    #[arg(long, hide = true)]
    pub progress_token: Option<String>,

    /// POST progress every N requests (default: 1 = every request)
    #[arg(long, hide = true, default_value = "1")]
    pub progress_interval: u32,

    /// Config ID for progress reporting (passed by orchestrator)
    #[arg(long, hide = true)]
    pub progress_config_id: Option<String>,

    /// Testbed ID for progress reporting (passed by orchestrator)
    #[arg(long, hide = true)]
    pub progress_testbed_id: Option<String>,

    /// Language name for progress reporting (passed by orchestrator)
    #[arg(long, hide = true)]
    pub benchmark_language: Option<String>,

    // ── URL diagnostic (PR-04 path) ─────────────────────────────────────────
    /// Run the URL page-load diagnostic workflow against the provided URL.
    #[arg(long)]
    pub url_test_url: Option<String>,

    /// Optional bearer token for URL diagnostic requests.
    #[arg(long)]
    pub url_test_auth_token: Option<String>,

    /// Optional cookie header value for URL diagnostic requests.
    #[arg(long)]
    pub url_test_cookie: Option<String>,

    /// Extra headers for URL diagnostics, repeatable: --url-test-header 'Name: value'
    #[arg(long = "url-test-header", action = clap::ArgAction::Append, num_args = 1)]
    pub url_test_headers: Vec<String>,

    /// Enable HAR capture for URL diagnostics when supported.
    #[arg(long)]
    pub url_test_capture_har: bool,

    /// Enable packet capture for URL diagnostics when supported.
    #[arg(long)]
    pub url_test_capture_pcap: bool,

    /// Preferred protocol mode for URL diagnostics: auto|h1|h2|h3.
    #[arg(long)]
    pub url_test_protocol_force: Option<String>,

    /// Repetition count for HTTP/3 validation probes in URL diagnostics.
    #[arg(long)]
    pub url_test_http3_repeat: Option<u32>,

    /// Emit the URL diagnostic result as JSON to stdout.
    #[arg(long)]
    pub url_test_json: bool,

    /// Run a TLS endpoint profile against the provided https URL.
    #[arg(long)]
    pub tls_profile_url: Option<String>,

    /// Optional IP override for the TLS endpoint profile connection target.
    #[arg(long)]
    pub tls_profile_ip: Option<String>,

    /// Optional SNI override for the TLS endpoint profile.
    #[arg(long)]
    pub tls_profile_sni: Option<String>,

    /// Target kind for the TLS endpoint profile: managed-endpoint|external-url|external-host.
    #[arg(long)]
    pub tls_profile_target_kind: Option<String>,

    /// Emit the TLS endpoint profile as JSON to stdout.
    #[arg(long)]
    pub tls_profile_json: bool,

    /// Optional project UUID to attribute persisted TLS profile runs.
    #[arg(long)]
    pub tls_profile_project_id: Option<String>,

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

    /// Optional: persist logs to this PostgreSQL URL (TimescaleDB)
    #[arg(long)]
    pub log_db_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, ValueEnum)]
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
    pub json_stdout: Option<bool>,
    pub benchmark_mode: Option<bool>,
    pub benchmark_phase: Option<String>,
    pub benchmark_scenario: Option<String>,
    pub benchmark_launch_index: Option<u32>,
    pub benchmark_min_samples: Option<u32>,
    pub benchmark_max_samples: Option<u32>,
    pub benchmark_min_duration_ms: Option<u64>,
    pub benchmark_target_relative_error: Option<f64>,
    pub benchmark_target_absolute_error: Option<f64>,
    pub benchmark_pilot_min_samples: Option<u32>,
    pub benchmark_pilot_max_samples: Option<u32>,
    pub benchmark_pilot_min_duration_ms: Option<u64>,
    pub benchmark_environment_check_samples: Option<u32>,
    pub benchmark_environment_check_interval_ms: Option<u64>,
    pub benchmark_stability_check_samples: Option<u32>,
    pub benchmark_stability_check_interval_ms: Option<u64>,
    pub benchmark_max_packet_loss_percent: Option<f64>,
    pub benchmark_max_jitter_ratio: Option<f64>,
    pub benchmark_max_rtt_spread_ratio: Option<f64>,
    pub benchmark_overhead_samples: Option<u32>,
    pub benchmark_cooldown_samples: Option<u32>,
    pub save_to_db: Option<bool>,
    pub db_url: Option<String>,
    pub db_migrate: Option<bool>,
    pub tls_profile_project_id: Option<String>,
    pub save_to_sql: Option<bool>,
    pub connection_string: Option<String>,
    pub log_level: Option<String>,
    pub log_db_url: Option<String>,
    pub page_assets: Option<usize>,
    pub page_asset_size: Option<String>,
    pub page_preset: Option<String>,
    pub http_stacks: Option<Vec<String>>,
    pub packet_capture: Option<PacketCaptureConfig>,
    pub capture_mode: Option<String>,
    pub impairment: Option<ImpairmentConfig>,
}

/// Fully resolved configuration with all defaults applied.
/// Priority: CLI arg > JSON config key > built-in default.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// One or more target URLs to probe. Always non-empty (defaults to localhost).
    pub targets: Vec<String>,
    pub url_test_url: Option<String>,
    pub tls_profile_url: Option<String>,
    pub tls_profile_ip: Option<String>,
    pub tls_profile_sni: Option<String>,
    pub tls_profile_target_kind: Option<String>,
    pub tls_profile_json: bool,
    pub tls_profile_project_id: Option<String>,
    pub url_test_auth_token: Option<String>,
    pub url_test_cookie: Option<String>,
    pub url_test_headers: Vec<String>,
    pub url_test_capture_har: bool,
    pub url_test_capture_pcap: bool,
    pub url_test_protocol_force: Option<String>,
    pub url_test_http3_repeat: u32,
    pub url_test_json: bool,
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
    pub json_stdout: bool,
    pub benchmark_mode: bool,
    pub benchmark_phase: String,
    pub benchmark_scenario: String,
    pub benchmark_launch_index: u32,
    pub benchmark_min_samples: Option<u32>,
    pub benchmark_max_samples: Option<u32>,
    pub benchmark_min_duration_ms: Option<u64>,
    pub benchmark_target_relative_error: Option<f64>,
    pub benchmark_target_absolute_error: Option<f64>,
    pub benchmark_pilot_min_samples: Option<u32>,
    pub benchmark_pilot_max_samples: Option<u32>,
    pub benchmark_pilot_min_duration_ms: Option<u64>,
    pub benchmark_environment_check_samples: Option<u32>,
    pub benchmark_environment_check_interval_ms: Option<u64>,
    pub benchmark_stability_check_samples: Option<u32>,
    pub benchmark_stability_check_interval_ms: Option<u64>,
    pub benchmark_max_packet_loss_percent: Option<f64>,
    pub benchmark_max_jitter_ratio: Option<f64>,
    pub benchmark_max_rtt_spread_ratio: Option<f64>,
    pub benchmark_overhead_samples: Option<u32>,
    pub benchmark_cooldown_samples: Option<u32>,
    pub progress_url: Option<String>,
    pub progress_token: Option<String>,
    pub progress_interval: u32,
    pub progress_config_id: Option<String>,
    pub progress_testbed_id: Option<String>,
    pub benchmark_language: Option<String>,
    pub save_to_db: bool,
    pub db_url: Option<String>,
    pub db_migrate: bool,
    pub save_to_sql: bool,
    pub connection_string: Option<String>,
    pub log_level: Option<String>,
    pub log_db_url: Option<String>,
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
            let mode = self
                .capture_mode
                .clone()
                .or(f.capture_mode)
                .and_then(|s| {
                    let normalized = s.trim().to_ascii_lowercase();
                    let normalized = match normalized.as_str() {
                        "off" | "disabled" | "none" => "off",
                        "auto" => "auto",
                        "tshark" | "t-shark" => "tshark",
                        "raw-socket" | "raw_socket" | "rawsocket" => "raw-socket",
                        other => other,
                    };
                    <PacketCaptureMode as clap::ValueEnum>::from_str(normalized, true).ok()
                })
                .or(pc.mode)
                .unwrap_or_default();
            ResolvedPacketCaptureConfig {
                mode,
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
            url_test_url: self.url_test_url,
            tls_profile_url: self.tls_profile_url,
            tls_profile_ip: self.tls_profile_ip,
            tls_profile_sni: self.tls_profile_sni,
            tls_profile_target_kind: self.tls_profile_target_kind,
            tls_profile_json: self.tls_profile_json,
            tls_profile_project_id: self.tls_profile_project_id.or(f.tls_profile_project_id),
            url_test_auth_token: self.url_test_auth_token,
            url_test_cookie: self.url_test_cookie,
            url_test_headers: self.url_test_headers,
            url_test_capture_har: self.url_test_capture_har,
            url_test_capture_pcap: self.url_test_capture_pcap,
            url_test_protocol_force: self.url_test_protocol_force,
            url_test_http3_repeat: self.url_test_http3_repeat.unwrap_or(10),
            url_test_json: self.url_test_json,
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
            json_stdout: self.json_stdout || f.json_stdout.unwrap_or(false),
            benchmark_mode: self.benchmark_mode || f.benchmark_mode.unwrap_or(false),
            benchmark_phase: self
                .benchmark_phase
                .or(f.benchmark_phase)
                .unwrap_or_else(|| "measured".into()),
            benchmark_scenario: self
                .benchmark_scenario
                .or(f.benchmark_scenario)
                .unwrap_or_else(|| "default".into()),
            benchmark_launch_index: self
                .benchmark_launch_index
                .or(f.benchmark_launch_index)
                .unwrap_or(0),
            benchmark_min_samples: self.benchmark_min_samples.or(f.benchmark_min_samples),
            benchmark_max_samples: self.benchmark_max_samples.or(f.benchmark_max_samples),
            benchmark_min_duration_ms: self
                .benchmark_min_duration_ms
                .or(f.benchmark_min_duration_ms),
            benchmark_target_relative_error: self
                .benchmark_target_relative_error
                .or(f.benchmark_target_relative_error),
            benchmark_target_absolute_error: self
                .benchmark_target_absolute_error
                .or(f.benchmark_target_absolute_error),
            benchmark_pilot_min_samples: self
                .benchmark_pilot_min_samples
                .or(f.benchmark_pilot_min_samples),
            benchmark_pilot_max_samples: self
                .benchmark_pilot_max_samples
                .or(f.benchmark_pilot_max_samples),
            benchmark_pilot_min_duration_ms: self
                .benchmark_pilot_min_duration_ms
                .or(f.benchmark_pilot_min_duration_ms),
            benchmark_environment_check_samples: self
                .benchmark_environment_check_samples
                .or(f.benchmark_environment_check_samples),
            benchmark_environment_check_interval_ms: self
                .benchmark_environment_check_interval_ms
                .or(f.benchmark_environment_check_interval_ms),
            benchmark_stability_check_samples: self
                .benchmark_stability_check_samples
                .or(f.benchmark_stability_check_samples),
            benchmark_stability_check_interval_ms: self
                .benchmark_stability_check_interval_ms
                .or(f.benchmark_stability_check_interval_ms),
            benchmark_max_packet_loss_percent: self
                .benchmark_max_packet_loss_percent
                .or(f.benchmark_max_packet_loss_percent),
            benchmark_max_jitter_ratio: self
                .benchmark_max_jitter_ratio
                .or(f.benchmark_max_jitter_ratio),
            benchmark_max_rtt_spread_ratio: self
                .benchmark_max_rtt_spread_ratio
                .or(f.benchmark_max_rtt_spread_ratio),
            benchmark_overhead_samples: self
                .benchmark_overhead_samples
                .or(f.benchmark_overhead_samples),
            benchmark_cooldown_samples: self
                .benchmark_cooldown_samples
                .or(f.benchmark_cooldown_samples),
            progress_url: self.progress_url,
            progress_token: self.progress_token,
            progress_interval: self.progress_interval,
            progress_config_id: self.progress_config_id,
            progress_testbed_id: self.progress_testbed_id,
            benchmark_language: self.benchmark_language,
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
            log_db_url: self.log_db_url.or(f.log_db_url),
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
        if !matches!(
            self.benchmark_phase.as_str(),
            "environment-check"
                | "stability-check"
                | "pilot"
                | "warmup"
                | "measured"
                | "cooldown"
                | "overhead"
        ) {
            anyhow::bail!(
                "--benchmark-phase must be one of: environment-check, stability-check, pilot, warmup, measured, cooldown, overhead"
            );
        }
        if self.benchmark_scenario.trim().is_empty() {
            anyhow::bail!("--benchmark-scenario must not be empty");
        }
        let benchmark_min_samples = self.benchmark_min_samples.unwrap_or(self.runs);
        let benchmark_max_samples = self.benchmark_max_samples.unwrap_or(self.runs);
        if benchmark_min_samples == 0 {
            anyhow::bail!("--benchmark-min-samples must be at least 1");
        }
        if benchmark_max_samples == 0 {
            anyhow::bail!("--benchmark-max-samples must be at least 1");
        }
        if benchmark_max_samples < benchmark_min_samples {
            anyhow::bail!(
                "--benchmark-max-samples must be greater than or equal to --benchmark-min-samples"
            );
        }
        let benchmark_pilot_min_samples = self.benchmark_pilot_min_samples.unwrap_or(1);
        let benchmark_pilot_max_samples = self
            .benchmark_pilot_max_samples
            .unwrap_or(benchmark_pilot_min_samples);
        if benchmark_pilot_min_samples == 0 {
            anyhow::bail!("--benchmark-pilot-min-samples must be at least 1");
        }
        if benchmark_pilot_max_samples == 0 {
            anyhow::bail!("--benchmark-pilot-max-samples must be at least 1");
        }
        if benchmark_pilot_max_samples < benchmark_pilot_min_samples {
            anyhow::bail!(
                "--benchmark-pilot-max-samples must be greater than or equal to --benchmark-pilot-min-samples"
            );
        }
        if let Some(stability_samples) = self.benchmark_stability_check_samples {
            if stability_samples == 0 {
                anyhow::bail!("--benchmark-stability-check-samples must be at least 1");
            }
        }
        if let Some(environment_samples) = self.benchmark_environment_check_samples {
            if environment_samples == 0 {
                anyhow::bail!("--benchmark-environment-check-samples must be at least 1");
            }
        }
        if let Some(overhead_samples) = self.benchmark_overhead_samples {
            if overhead_samples == 0 {
                anyhow::bail!("--benchmark-overhead-samples must be at least 1");
            }
        }
        if let Some(cooldown_samples) = self.benchmark_cooldown_samples {
            if cooldown_samples == 0 {
                anyhow::bail!("--benchmark-cooldown-samples must be at least 1");
            }
        }
        if let Some(target_relative_error) = self.benchmark_target_relative_error {
            if !target_relative_error.is_finite() || target_relative_error <= 0.0 {
                anyhow::bail!("--benchmark-target-relative-error must be a positive finite number");
            }
        }
        if let Some(target_absolute_error) = self.benchmark_target_absolute_error {
            if !target_absolute_error.is_finite() || target_absolute_error <= 0.0 {
                anyhow::bail!("--benchmark-target-absolute-error must be a positive finite number");
            }
        }
        if let Some(max_packet_loss_percent) = self.benchmark_max_packet_loss_percent {
            if !max_packet_loss_percent.is_finite()
                || !(0.0..=100.0).contains(&max_packet_loss_percent)
            {
                anyhow::bail!(
                    "--benchmark-max-packet-loss-percent must be a finite number between 0 and 100"
                );
            }
        }
        if let Some(max_jitter_ratio) = self.benchmark_max_jitter_ratio {
            if !max_jitter_ratio.is_finite() || max_jitter_ratio <= 0.0 {
                anyhow::bail!("--benchmark-max-jitter-ratio must be a positive finite number");
            }
        }
        if let Some(max_rtt_spread_ratio) = self.benchmark_max_rtt_spread_ratio {
            if !max_rtt_spread_ratio.is_finite() || max_rtt_spread_ratio <= 0.0 {
                anyhow::bail!("--benchmark-max-rtt-spread-ratio must be a positive finite number");
            }
        }
        let adaptive_benchmark_controls = self.benchmark_min_samples.is_some()
            || self.benchmark_max_samples.is_some()
            || self.benchmark_min_duration_ms.is_some()
            || self.benchmark_target_relative_error.is_some()
            || self.benchmark_target_absolute_error.is_some();
        let pilot_benchmark_controls = self.benchmark_pilot_min_samples.is_some()
            || self.benchmark_pilot_max_samples.is_some()
            || self.benchmark_pilot_min_duration_ms.is_some();
        let environment_benchmark_controls = self.benchmark_environment_check_samples.is_some()
            || self.benchmark_environment_check_interval_ms.is_some();
        let stability_benchmark_controls = self.benchmark_stability_check_samples.is_some()
            || self.benchmark_stability_check_interval_ms.is_some();
        let noise_threshold_controls = self.benchmark_max_packet_loss_percent.is_some()
            || self.benchmark_max_jitter_ratio.is_some()
            || self.benchmark_max_rtt_spread_ratio.is_some();
        if adaptive_benchmark_controls
            && self.benchmark_mode
            && self.benchmark_phase == "measured"
            && !self.http_stacks.is_empty()
        {
            anyhow::bail!(
                "adaptive benchmark stop controls are not yet supported together with --http-stacks"
            );
        }
        if pilot_benchmark_controls
            && self.benchmark_mode
            && self.benchmark_phase == "measured"
            && !self.http_stacks.is_empty()
        {
            anyhow::bail!(
                "pilot benchmark controls are not yet supported together with --http-stacks"
            );
        }
        if stability_benchmark_controls
            && self.benchmark_mode
            && self.benchmark_phase == "measured"
            && !self.http_stacks.is_empty()
        {
            anyhow::bail!(
                "stability-check controls are not yet supported together with --http-stacks"
            );
        }
        if environment_benchmark_controls
            && self.benchmark_mode
            && self.benchmark_phase == "measured"
            && !self.http_stacks.is_empty()
        {
            anyhow::bail!(
                "environment-check controls are not yet supported together with --http-stacks"
            );
        }
        if noise_threshold_controls
            && self.benchmark_mode
            && self.benchmark_phase == "measured"
            && !self.http_stacks.is_empty()
        {
            anyhow::bail!(
                "benchmark noise thresholds are not yet supported together with --http-stacks"
            );
        }

        if let Some(url) = &self.url_test_url {
            let parsed = url::Url::parse(url)
                .map_err(|e| anyhow::anyhow!("--url-test-url invalid URL: {e}"))?;
            match parsed.scheme() {
                "http" | "https" => {}
                other => anyhow::bail!("--url-test-url unsupported URL scheme '{other}'"),
            }
            if let Some(force) = &self.url_test_protocol_force {
                match force.as_str() {
                    "auto" | "h1" | "h2" | "h3" => {}
                    _ => {
                        anyhow::bail!("--url-test-protocol-force must be one of: auto, h1, h2, h3")
                    }
                }
            }
            if self.url_test_http3_repeat == 0 {
                anyhow::bail!("--url-test-http3-repeat must be at least 1");
            }
        }
        if let Some(url) = &self.tls_profile_url {
            let parsed = url::Url::parse(url)
                .map_err(|e| anyhow::anyhow!("--tls-profile-url invalid URL: {e}"))?;
            match parsed.scheme() {
                "https" => {}
                other => anyhow::bail!(
                    "--tls-profile-url unsupported URL scheme '{other}' (expected https)"
                ),
            }
            if parsed.host_str().is_none() {
                anyhow::bail!("--tls-profile-url must include a host");
            }
            if let Some(kind) = &self.tls_profile_target_kind {
                match kind.as_str() {
                    "managed-endpoint" | "managed_endpoint" | "external-url" | "external_url" | "external-host" | "external_host" => {}
                    _ => anyhow::bail!("--tls-profile-target-kind must be one of: managed-endpoint, external-url, external-host"),
                }
            }
            if let Some(ip) = &self.tls_profile_ip {
                ip.parse::<std::net::IpAddr>()
                    .map_err(|e| anyhow::anyhow!("--tls-profile-ip invalid IP: {e}"))?;
            }
            if let Some(project_id) = &self.tls_profile_project_id {
                project_id
                    .parse::<uuid::Uuid>()
                    .map_err(|e| anyhow::anyhow!("--tls-profile-project-id invalid UUID: {e}"))?;
            }
        }
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
        assert!(!cli.benchmark_mode);
        assert!(cli.benchmark_phase.is_none());
        assert!(cli.benchmark_scenario.is_none());
        assert!(cli.benchmark_launch_index.is_none());
        assert!(cli.benchmark_min_samples.is_none());
        assert!(cli.benchmark_max_samples.is_none());
        assert!(cli.benchmark_min_duration_ms.is_none());
        assert!(cli.benchmark_target_relative_error.is_none());
        assert!(cli.benchmark_target_absolute_error.is_none());
        assert!(cli.benchmark_pilot_min_samples.is_none());
        assert!(cli.benchmark_pilot_max_samples.is_none());
        assert!(cli.benchmark_pilot_min_duration_ms.is_none());
        assert!(cli.benchmark_environment_check_samples.is_none());
        assert!(cli.benchmark_environment_check_interval_ms.is_none());
        assert!(cli.benchmark_stability_check_samples.is_none());
        assert!(cli.benchmark_stability_check_interval_ms.is_none());
        assert!(cli.benchmark_max_packet_loss_percent.is_none());
        assert!(cli.benchmark_max_jitter_ratio.is_none());
        assert!(cli.benchmark_max_rtt_spread_ratio.is_none());
        assert!(cli.benchmark_overhead_samples.is_none());
        assert!(cli.benchmark_cooldown_samples.is_none());
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
        assert!(!cfg.benchmark_mode);
        assert_eq!(cfg.benchmark_phase, "measured");
        assert_eq!(cfg.benchmark_scenario, "default");
        assert_eq!(cfg.benchmark_launch_index, 0);
        assert!(cfg.benchmark_min_samples.is_none());
        assert!(cfg.benchmark_max_samples.is_none());
        assert!(cfg.benchmark_min_duration_ms.is_none());
        assert!(cfg.benchmark_target_relative_error.is_none());
        assert!(cfg.benchmark_target_absolute_error.is_none());
        assert!(cfg.benchmark_pilot_min_samples.is_none());
        assert!(cfg.benchmark_pilot_max_samples.is_none());
        assert!(cfg.benchmark_pilot_min_duration_ms.is_none());
        assert!(cfg.benchmark_environment_check_samples.is_none());
        assert!(cfg.benchmark_environment_check_interval_ms.is_none());
        assert!(cfg.benchmark_stability_check_samples.is_none());
        assert!(cfg.benchmark_stability_check_interval_ms.is_none());
        assert!(cfg.benchmark_max_packet_loss_percent.is_none());
        assert!(cfg.benchmark_max_jitter_ratio.is_none());
        assert!(cfg.benchmark_max_rtt_spread_ratio.is_none());
        assert!(cfg.benchmark_overhead_samples.is_none());
        assert!(cfg.benchmark_cooldown_samples.is_none());
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
    fn benchmark_mode_flag_is_resolved() {
        let cfg = Cli::parse_from(["networker-tester", "--benchmark-mode"]).resolve(None);
        assert!(cfg.benchmark_mode);
    }

    #[test]
    fn benchmark_lifecycle_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-phase",
            "warmup",
            "--benchmark-scenario",
            "cold",
            "--benchmark-launch-index",
            "7",
        ])
        .resolve(None);
        assert!(cfg.benchmark_mode);
        assert_eq!(cfg.benchmark_phase, "warmup");
        assert_eq!(cfg.benchmark_scenario, "cold");
        assert_eq!(cfg.benchmark_launch_index, 7);
    }

    #[test]
    fn benchmark_adaptive_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-min-samples",
            "10",
            "--benchmark-max-samples",
            "50",
            "--benchmark-min-duration-ms",
            "1500",
            "--benchmark-target-relative-error",
            "0.05",
            "--benchmark-target-absolute-error",
            "2.5",
        ])
        .resolve(None);
        assert_eq!(cfg.benchmark_min_samples, Some(10));
        assert_eq!(cfg.benchmark_max_samples, Some(50));
        assert_eq!(cfg.benchmark_min_duration_ms, Some(1500));
        assert_eq!(cfg.benchmark_target_relative_error, Some(0.05));
        assert_eq!(cfg.benchmark_target_absolute_error, Some(2.5));
    }

    #[test]
    fn benchmark_pilot_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-pilot-min-samples",
            "4",
            "--benchmark-pilot-max-samples",
            "9",
            "--benchmark-pilot-min-duration-ms",
            "200",
        ])
        .resolve(None);
        assert_eq!(cfg.benchmark_pilot_min_samples, Some(4));
        assert_eq!(cfg.benchmark_pilot_max_samples, Some(9));
        assert_eq!(cfg.benchmark_pilot_min_duration_ms, Some(200));
    }

    #[test]
    fn benchmark_environment_check_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-environment-check-samples",
            "6",
            "--benchmark-environment-check-interval-ms",
            "40",
        ])
        .resolve(None);
        assert_eq!(cfg.benchmark_environment_check_samples, Some(6));
        assert_eq!(cfg.benchmark_environment_check_interval_ms, Some(40));
    }

    #[test]
    fn benchmark_stability_check_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-stability-check-samples",
            "15",
            "--benchmark-stability-check-interval-ms",
            "75",
        ])
        .resolve(None);
        assert_eq!(cfg.benchmark_stability_check_samples, Some(15));
        assert_eq!(cfg.benchmark_stability_check_interval_ms, Some(75));
    }

    #[test]
    fn benchmark_noise_threshold_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-max-packet-loss-percent",
            "2.5",
            "--benchmark-max-jitter-ratio",
            "0.2",
            "--benchmark-max-rtt-spread-ratio",
            "1.7",
        ])
        .resolve(None);
        assert_eq!(cfg.benchmark_max_packet_loss_percent, Some(2.5));
        assert_eq!(cfg.benchmark_max_jitter_ratio, Some(0.2));
        assert_eq!(cfg.benchmark_max_rtt_spread_ratio, Some(1.7));
    }

    #[test]
    fn benchmark_overhead_and_cooldown_flags_are_resolved() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-overhead-samples",
            "2",
            "--benchmark-cooldown-samples",
            "3",
        ])
        .resolve(None);
        assert_eq!(cfg.benchmark_overhead_samples, Some(2));
        assert_eq!(cfg.benchmark_cooldown_samples, Some(3));
    }

    #[test]
    fn validate_rejects_benchmark_max_samples_below_min_samples() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-min-samples",
            "20",
            "--benchmark-max-samples",
            "10",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-max-samples"));
    }

    #[test]
    fn validate_rejects_non_positive_target_relative_error() {
        let cfg = Cli::parse_from(["networker-tester", "--benchmark-target-relative-error", "0"])
            .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-target-relative-error"));
    }

    #[test]
    fn validate_rejects_non_positive_target_absolute_error() {
        let cfg = Cli::parse_from(["networker-tester", "--benchmark-target-absolute-error", "0"])
            .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-target-absolute-error"));
    }

    #[test]
    fn validate_rejects_benchmark_pilot_max_samples_below_min_samples() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-pilot-min-samples",
            "8",
            "--benchmark-pilot-max-samples",
            "4",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-pilot-max-samples"));
    }

    #[test]
    fn validate_rejects_non_positive_stability_check_samples() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-stability-check-samples",
            "0",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-stability-check-samples"));
    }

    #[test]
    fn validate_rejects_non_positive_environment_check_samples() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-environment-check-samples",
            "0",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-environment-check-samples"));
    }

    #[test]
    fn validate_rejects_invalid_benchmark_noise_thresholds() {
        let packet_loss_cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-max-packet-loss-percent",
            "120",
        ])
        .resolve(None);
        let packet_loss_err = packet_loss_cfg.validate().unwrap_err().to_string();
        assert!(packet_loss_err.contains("--benchmark-max-packet-loss-percent"));

        let jitter_cfg = Cli::parse_from(["networker-tester", "--benchmark-max-jitter-ratio", "0"])
            .resolve(None);
        let jitter_err = jitter_cfg.validate().unwrap_err().to_string();
        assert!(jitter_err.contains("--benchmark-max-jitter-ratio"));

        let spread_cfg =
            Cli::parse_from(["networker-tester", "--benchmark-max-rtt-spread-ratio", "0"])
                .resolve(None);
        let spread_err = spread_cfg.validate().unwrap_err().to_string();
        assert!(spread_err.contains("--benchmark-max-rtt-spread-ratio"));
    }

    #[test]
    fn validate_rejects_non_positive_overhead_samples() {
        let cfg = Cli::parse_from(["networker-tester", "--benchmark-overhead-samples", "0"])
            .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-overhead-samples"));
    }

    #[test]
    fn validate_rejects_non_positive_cooldown_samples() {
        let cfg = Cli::parse_from(["networker-tester", "--benchmark-cooldown-samples", "0"])
            .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("--benchmark-cooldown-samples"));
    }

    #[test]
    fn validate_rejects_adaptive_controls_with_http_stacks_in_measured_benchmark_mode() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--runs",
            "10",
            "--benchmark-min-samples",
            "5",
            "--http-stacks",
            "nginx",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("adaptive benchmark stop controls"));
    }

    #[test]
    fn validate_rejects_pilot_controls_with_http_stacks_in_measured_benchmark_mode() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-pilot-min-samples",
            "5",
            "--http-stacks",
            "nginx",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("pilot benchmark controls"));
    }

    #[test]
    fn validate_rejects_stability_controls_with_http_stacks_in_measured_benchmark_mode() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-stability-check-samples",
            "8",
            "--http-stacks",
            "nginx",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("stability-check controls"));
    }

    #[test]
    fn validate_rejects_environment_check_controls_with_http_stacks_in_measured_benchmark_mode() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-environment-check-samples",
            "5",
            "--http-stacks",
            "nginx",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("environment-check controls"));
    }

    #[test]
    fn validate_rejects_noise_threshold_controls_with_http_stacks_in_measured_benchmark_mode() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--benchmark-mode",
            "--benchmark-max-rtt-spread-ratio",
            "1.8",
            "--http-stacks",
            "nginx",
        ])
        .resolve(None);
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("benchmark noise thresholds"));
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
        with_env_vars_cleared(&["NETWORKER_SQL_CONN", "NETWORKER_DB_URL"], || {
            let cli = Cli::parse_from(["networker-tester", "--save-to-sql"]);
            assert!(cli.resolve(None).validate().is_err());
        });
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
        with_env_vars_cleared(&[key], f)
    }

    fn with_env_vars_cleared<T>(keys: &[&str], f: impl FnOnce() -> T) -> T {
        use std::sync::{Mutex, OnceLock};

        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let saved: Vec<_> = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        for k in keys {
            std::env::remove_var(k);
        }
        let result = f();
        for (k, v) in &saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        result
    }

    // ── Database flag tests ────────────────────────────────────────────────────

    #[test]
    fn url_test_flags_parse_and_validate() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--url-test-url",
            "https://example.com",
            "--url-test-protocol-force",
            "h3",
            "--url-test-http3-repeat",
            "3",
            "--url-test-json",
        ])
        .resolve(None);
        assert_eq!(cfg.url_test_url.as_deref(), Some("https://example.com"));
        assert_eq!(cfg.url_test_protocol_force.as_deref(), Some("h3"));
        assert_eq!(cfg.url_test_http3_repeat, 3);
        assert!(cfg.url_test_json);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn url_test_invalid_protocol_force_fails_validation() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--url-test-url",
            "https://example.com",
            "--url-test-protocol-force",
            "h9",
        ])
        .resolve(None);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn tls_profile_flags_parse_and_validate() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--tls-profile-url",
            "https://example.com",
            "--tls-profile-ip",
            "93.184.216.34",
            "--tls-profile-sni",
            "example.com",
            "--tls-profile-target-kind",
            "external-url",
            "--tls-profile-json",
        ])
        .resolve(None);
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.tls_profile_url.as_deref(), Some("https://example.com"));
        assert_eq!(cfg.tls_profile_ip.as_deref(), Some("93.184.216.34"));
        assert_eq!(cfg.tls_profile_sni.as_deref(), Some("example.com"));
        assert_eq!(cfg.tls_profile_target_kind.as_deref(), Some("external-url"));
        assert!(cfg.tls_profile_json);
    }

    #[test]
    fn tls_profile_invalid_scheme_fails_validation() {
        let cfg = Cli::parse_from([
            "networker-tester",
            "--tls-profile-url",
            "http://example.com",
        ])
        .resolve(None);
        assert!(cfg.validate().is_err());
    }

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
        with_env_vars_cleared(&["NETWORKER_DB_URL", "NETWORKER_SQL_CONN"], || {
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
        });
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
        // Must clear both env vars: resolve() falls back from connection_string → db_url
        with_env_vars_cleared(&["NETWORKER_DB_URL", "NETWORKER_SQL_CONN"], || {
            let cfg = Cli::parse_from(["networker-tester"]).resolve(None);
            assert!(cfg.db_url.is_none(), "--db-url should default to None");
        });
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
