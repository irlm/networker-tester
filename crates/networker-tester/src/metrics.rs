/// Core data model – one row per concept, normalized.
///
/// TestRun  → 1:N  RequestAttempt
/// RequestAttempt → 0:1 DnsResult, TcpResult, TlsResult, HttpResult, UdpResult, ErrorRecord
///              → 0:1 ServerTimingResult (when X-Networker-* headers present)
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capture::{EndpointPacketCount, PacketCaptureSummary, PacketShare, PortPacketCount};

// ─────────────────────────────────────────────────────────────────────────────
// Host information (shared between client and server)
// ─────────────────────────────────────────────────────────────────────────────

/// Non-sensitive system metadata for a host (client or server).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub cpu_cores: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_memory_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    /// Server uptime in seconds (server only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// Cloud region or location (auto-detected from cloud metadata on the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// 1-minute load average sampled on the server when GET /info was served
    /// (server only — client load lives in the run-level [`LoadSample`]s).
    /// None for old endpoints that don't report it, and on Windows servers.
    /// Additive; `schema_version` stays 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_avg_1m: Option<f64>,
    /// Available (reclaimable) memory in MB sampled on the server when GET
    /// /info was served (server only; Linux endpoints only — same per-platform
    /// honesty as [`LoadSample`]). Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_available_mb: Option<u64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Source-network context (client side)
// ─────────────────────────────────────────────────────────────────────────────

/// Best-effort description of the SOURCE network the client ran from: default
/// route interface, interface kind (WiFi vs ethernet), MTU, egress IP, gateway
/// and a conservative VPN heuristic. Every field is optional — collection
/// failures leave fields `None` and never abort a run. Additive to the JSON
/// contract; `schema_version` stays 1.0.
///
/// Collection lives in [`crate::network_context`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkContext {
    /// Name of the interface owning the default route (e.g. `en0`, `eth0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_interface: Option<String>,
    /// Classification of the default interface: `ethernet` | `wifi` |
    /// `virtual` | `unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_kind: Option<String>,
    /// MTU of the default interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    /// Local source address of a UDP socket connect()ed to the target — the
    /// egress address actually used for this run (no packets are sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_ip: Option<String>,
    /// Default gateway address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_ip: Option<String>,
    /// Conservative VPN heuristic: `Some(true)` only when the default route
    /// goes through a tunnel-like interface (utun/tun/wg/tap/ppp/...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpn_detected: Option<bool>,
    /// Tunnel interface name when `vpn_detected` is `Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpn_interface: Option<String>,
    /// Whether an IPv6 default route is present on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_available: Option<bool>,
}

impl NetworkContext {
    /// True when collection produced no data at all (render guards use this).
    pub fn is_empty(&self) -> bool {
        self.default_interface.is_none()
            && self.interface_kind.is_none()
            && self.mtu.is_none()
            && self.local_ip.is_none()
            && self.gateway_ip.is_none()
            && self.vpn_detected.is_none()
            && self.vpn_interface.is_none()
            && self.ipv6_available.is_none()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Offline GeoIP enrichment (user-supplied MaxMind GeoLite2 databases)
// ─────────────────────────────────────────────────────────────────────────────

/// Geo / ISP / ASN enrichment for one IP address, resolved from local MaxMind
/// `.mmdb` databases (never a runtime API call). All fields are best-effort:
/// each is `None` when the corresponding database is absent or has no record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeoInfo {
    /// ISO 3166-1 alpha-2 country code, e.g. "US".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// City name (English), e.g. "Linköping".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// Autonomous system number, e.g. 13335.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asn: Option<u32>,
    /// Autonomous system organization, e.g. "Cloudflare, Inc.".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_org: Option<String>,
    /// Build date (`YYYY-MM-DD`) of the .mmdb the lookup came from, so
    /// consumers can judge staleness of the enrichment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_date: Option<String>,
}

impl GeoInfo {
    /// Compact human-readable label, e.g. "US · Linköping · AS1221 Telstra Pty Ltd".
    /// Returns "—" when every field is `None`.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(ref country) = self.country {
            parts.push(country.clone());
        }
        if let Some(ref city) = self.city {
            parts.push(city.clone());
        }
        match (self.asn, self.as_org.as_deref()) {
            (Some(asn), Some(org)) => parts.push(format!("AS{asn} {org}")),
            (Some(asn), None) => parts.push(format!("AS{asn}")),
            (None, Some(org)) => parts.push(org.to_string()),
            (None, None) => {}
        }
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join(" · ")
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Network baseline (RTT measurement before probes)
// ─────────────────────────────────────────────────────────────────────────────

/// Classification of the network path between client and target.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkType {
    /// Loopback (127.x.x.x / ::1)
    Loopback,
    /// Private/LAN (10.x, 172.16-31.x, 192.168.x, fe80::, etc.)
    LAN,
    /// Public internet
    Internet,
}

impl std::fmt::Display for NetworkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkType::Loopback => write!(f, "Loopback"),
            NetworkType::LAN => write!(f, "LAN"),
            NetworkType::Internet => write!(f, "Internet"),
        }
    }
}

/// RTT baseline measured before probes start (N TCP connect round-trips).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkBaseline {
    /// Number of RTT samples collected.
    pub samples: u32,
    /// Minimum RTT in milliseconds.
    pub rtt_min_ms: f64,
    /// Average RTT in milliseconds.
    pub rtt_avg_ms: f64,
    /// Maximum RTT in milliseconds.
    pub rtt_max_ms: f64,
    /// Median (p50) RTT in milliseconds.
    pub rtt_p50_ms: f64,
    /// 95th percentile RTT in milliseconds.
    pub rtt_p95_ms: f64,
    /// Network classification based on target IP.
    pub network_type: NetworkType,
}

/// Short pre-benchmark baseline measurement recorded as an explicit lifecycle phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkEnvironmentCheck {
    pub attempted_samples: u32,
    pub successful_samples: u32,
    pub failed_samples: u32,
    pub duration_ms: f64,
    pub rtt_min_ms: f64,
    pub rtt_avg_ms: f64,
    pub rtt_max_ms: f64,
    pub rtt_p50_ms: f64,
    pub rtt_p95_ms: f64,
    pub packet_loss_percent: f64,
    pub network_type: NetworkType,
    /// Tester CPU busy% over the environment-check window (additive, schema
    /// 1.0). `None` when the window was shorter than the
    /// [`MIN_CPU_WINDOW_MS`] trust guard or the platform has no collector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_busy_percent: Option<f64>,
    /// Tester hypervisor steal% over the same window (Linux only; `None`
    /// on macOS/Windows — no steal concept/API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_steal_percent: Option<f64>,
}

/// Short pre-benchmark noise measurement used to assess environment stability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkStabilityCheck {
    pub attempted_samples: u32,
    pub successful_samples: u32,
    pub failed_samples: u32,
    pub duration_ms: f64,
    pub rtt_min_ms: f64,
    pub rtt_avg_ms: f64,
    pub rtt_max_ms: f64,
    pub rtt_p50_ms: f64,
    pub rtt_p95_ms: f64,
    pub jitter_ms: f64,
    pub packet_loss_percent: f64,
    pub network_type: NetworkType,
}

/// Adaptive benchmark execution plan applied to the measured phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkExecutionPlan {
    /// Origin of the applied plan, e.g. explicit, pilot-derived, pilot-assisted.
    pub source: String,
    pub min_samples: u32,
    pub max_samples: u32,
    pub min_duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_relative_error: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_absolute_error: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub pilot_sample_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pilot_elapsed_ms: Option<f64>,
}

/// Publication thresholds used to classify benchmark noise and publication readiness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkNoiseThresholds {
    pub max_packet_loss_percent: f64,
    pub max_jitter_ratio: f64,
    pub max_rtt_spread_ratio: f64,
    /// Maximum environment-check tester CPU busy% allowed for
    /// publication-ready claims (deliberately lenient default — a contended
    /// tester distorts measurements the same way jitter/loss do). Additive:
    /// serde-defaulted so pre-existing JSON still deserializes.
    #[serde(default = "default_max_cpu_busy_percent")]
    pub max_cpu_busy_percent: f64,
    /// Maximum environment-check tester CPU steal% allowed for
    /// publication-ready claims (cloud testers; Linux only).
    #[serde(default = "default_max_cpu_steal_percent")]
    pub max_cpu_steal_percent: f64,
}

fn default_max_cpu_busy_percent() -> f64 {
    85.0
}

fn default_max_cpu_steal_percent() -> f64 {
    5.0
}

impl Default for BenchmarkNoiseThresholds {
    fn default() -> Self {
        Self {
            max_packet_loss_percent: 5.0,
            max_jitter_ratio: 0.25,
            max_rtt_spread_ratio: 2.0,
            max_cpu_busy_percent: default_max_cpu_busy_percent(),
            max_cpu_steal_percent: default_max_cpu_steal_percent(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level run
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    /// Version of the tester JSON output contract. Bumped when the emitted
    /// schema changes in a way consumers must be aware of. Additive fields do
    /// not require a bump; restructuring or field-removal does. This is the
    /// stable seam between the Rust probe core and downstream consumers
    /// (e.g. the C# agent/control-plane in the hybrid migration).
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub target_url: String,
    pub target_host: String,
    pub modes: Vec<String>,
    pub total_runs: u32,
    pub concurrency: u32,
    pub timeout_ms: u64,
    pub client_os: String,
    pub client_version: String,
    /// Server system metadata fetched from GET /info before probes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<HostInfo>,
    /// Client system metadata collected locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<HostInfo>,
    /// Source-network context (default route, interface kind, VPN heuristic)
    /// collected best-effort at run start. Additive; schema stays 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_network: Option<NetworkContext>,
    /// System load sampled on the tester at run start (measurement-gap #15).
    /// Best-effort per platform; None when the platform exposes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_load_before: Option<LoadSample>,
    /// System load sampled on the tester at run end (measurement-gap #15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_load_after: Option<LoadSample>,
    /// Tester CPU usage across the run: whole-run mean plus 1 s-sampled
    /// max/p95 busy and steal (additive; schema stays 1.0). `None` on
    /// platforms without a tick collector or when every window failed the
    /// min-window trust guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage: Option<CpuUsage>,
    /// One-shot SNTP cross-check of the client clock (measurement-gap #16).
    /// Independent of the per-attempt `clock_skew_ms` heuristic; best-effort,
    /// None when NTP is unreachable or disabled (`NETWORKER_NTP_DISABLE=1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_sync: Option<ClockSync>,
    /// Network baseline RTT measured before probes start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<NetworkBaseline>,
    /// Optional packet capture summary for runs where capture was enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_capture_summary: Option<PacketCaptureSummary>,
    /// Optional explicit environment-check phase for benchmark-mode runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_environment_check: Option<BenchmarkEnvironmentCheck>,
    /// Optional short pre-benchmark noise measurement for benchmark-mode runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_stability_check: Option<BenchmarkStabilityCheck>,
    /// Primary benchmark phase for this run when emitted in benchmark mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_phase: Option<String>,
    /// Benchmark scenario label (for example cold, warm, warmup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_scenario: Option<String>,
    /// Launch index supplied by an orchestrator when the run is part of a repeated benchmark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_launch_index: Option<u32>,
    /// Number of leading attempts that belong to an internal warmup phase.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub benchmark_warmup_attempt_count: u32,
    /// Number of attempts collected in the internal pilot phase before measured samples.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub benchmark_pilot_attempt_count: u32,
    /// Number of attempts collected in the internal overhead phase before measured samples.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub benchmark_overhead_attempt_count: u32,
    /// Number of attempts collected in the internal cooldown phase after measured samples.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub benchmark_cooldown_attempt_count: u32,
    /// Applied adaptive execution plan for benchmark-mode measured runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_execution_plan: Option<BenchmarkExecutionPlan>,
    /// Configured publication thresholds used to assess benchmark noise quality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_noise_thresholds: Option<BenchmarkNoiseThresholds>,
    /// Offline GeoIP enrichment of the client's egress IP. Only present when a
    /// local MaxMind database is configured AND the egress interface address is
    /// a public IP (RFC1918/CGNAT/loopback egress is never enriched — we do not
    /// call external "what's my IP" services).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_geo: Option<GeoInfo>,
    /// Offline GeoIP enrichment of the first resolved target IP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_geo: Option<GeoInfo>,
    pub attempts: Vec<RequestAttempt>,
}

impl TestRun {
    pub fn success_count(&self) -> usize {
        self.attempts.iter().filter(|a| a.success).count()
    }

    pub fn failure_count(&self) -> usize {
        self.attempts.iter().filter(|a| !a.success).count()
    }

    pub fn protocols_tested(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.attempts
            .iter()
            .filter_map(|a| {
                if seen.insert(a.protocol.to_string()) {
                    Some(a.protocol.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// The phase whose samples count as measured for this run
    /// ("measured" for non-benchmark runs).
    pub fn primary_phase(&self) -> &str {
        self.benchmark_phase.as_deref().unwrap_or("measured")
    }

    /// Phase label for each attempt, index-aligned with `self.attempts`.
    ///
    /// Prefers the structural `RequestAttempt.phase` (set at attempt creation
    /// by the benchmark runner since v0.28.81); falls back to positional
    /// reconstruction from the recorded phase counts for runs captured before
    /// the field existed. This is the single source of phase attribution —
    /// the benchmark JSON artifact and every human-facing stats surface
    /// (console summary, HTML, Excel) derive from it.
    pub fn resolved_attempt_phases(&self) -> Vec<String> {
        let primary_phase = self.primary_phase();
        let warmup_end = self.benchmark_warmup_attempt_count as usize;
        let overhead_end = warmup_end + self.benchmark_overhead_attempt_count as usize;
        let pilot_end = overhead_end + self.benchmark_pilot_attempt_count as usize;
        let cooldown_start = self
            .attempts
            .len()
            .saturating_sub(self.benchmark_cooldown_attempt_count as usize);
        let primary_is_special =
            matches!(primary_phase, "warmup" | "overhead" | "pilot" | "cooldown");

        self.attempts
            .iter()
            .enumerate()
            .map(|(idx, attempt)| {
                if let Some(phase) = attempt.phase.as_deref() {
                    phase.to_string()
                } else if primary_is_special {
                    primary_phase.to_string()
                } else if idx < warmup_end {
                    "warmup".to_string()
                } else if idx < overhead_end {
                    "overhead".to_string()
                } else if idx < pilot_end {
                    "pilot".to_string()
                } else if self.benchmark_cooldown_attempt_count > 0 && idx >= cooldown_start {
                    "cooldown".to_string()
                } else {
                    primary_phase.to_string()
                }
            })
            .collect()
    }

    /// Attempts belonging to this run's primary (measured) phase — the same
    /// set the benchmark JSON artifact computes its summaries over. For
    /// non-benchmark runs this is all attempts. Every stats surface (console
    /// summary, HTML report, Excel workbook) must compute over this set so
    /// human-facing numbers agree with the artifact.
    pub fn measured_attempts(&self) -> Vec<&RequestAttempt> {
        let primary_phase = self.primary_phase();
        self.resolved_attempt_phases()
            .iter()
            .zip(self.attempts.iter())
            .filter(|(phase, _)| phase.as_str() == primary_phase)
            .map(|(_, attempt)| attempt)
            .collect()
    }

    /// Number of attempts excluded from stats because they belong to a
    /// non-primary benchmark phase (warmup/overhead/pilot/cooldown).
    pub fn excluded_attempt_count(&self) -> usize {
        let primary_phase = self.primary_phase();
        self.resolved_attempt_phases()
            .iter()
            .filter(|phase| phase.as_str() != primary_phase)
            .count()
    }
}

impl HostInfo {
    /// Collect system info for the local machine (client side).
    pub fn collect_local() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            total_memory_mb: detect_total_memory_mb(),
            os_version: detect_os_version(),
            hostname: detect_hostname(),
            server_version: None,
            uptime_secs: None,
            region: None,
            // Client-side load lives in the run-level LoadSamples, not here.
            load_avg_1m: None,
            mem_available_mb: None,
        }
    }
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// Current version of the tester JSON output contract (see [`TestRun::schema_version`]).
pub const SCHEMA_VERSION: &str = "1.0";

fn default_schema_version() -> String {
    SCHEMA_VERSION.to_string()
}

fn detect_total_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb / 1024);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(bytes / (1024 * 1024))
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("wmic")
            .args(["computersystem", "get", "TotalPhysicalMemory", "/value"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(val) = line.strip_prefix("TotalPhysicalMemory=") {
                let bytes: u64 = val.trim().parse().ok()?;
                return Some(bytes / (1024 * 1024));
            }
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn detect_os_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let release = std::fs::read_to_string("/etc/os-release").ok()?;
        for line in release.lines() {
            if let Some(val) = line.strip_prefix("PRETTY_NAME=") {
                return Some(val.trim_matches('"').to_string());
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if ver.is_empty() {
            None
        } else {
            Some(format!("macOS {ver}"))
        }
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .ok()?;
        let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if ver.is_empty() {
            None
        } else {
            Some(ver)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Run-level system load sampling (measurement-gap #15)
// ─────────────────────────────────────────────────────────────────────────────

/// A point-in-time system load sample taken on the tester host. All fields are
/// best-effort per platform — a field is `None` when the platform does not
/// expose it, never fabricated:
///
/// * Linux: `load_avg_1m` from `/proc/loadavg`, `mem_available_mb` from
///   `/proc/meminfo` (`MemAvailable`). `cpu_busy_percent` comes from a
///   two-sample `/proc/stat` delta over the whole run window (see
///   [`CpuTicks`]) — it is set on the *after* sample only; the *before*
///   sample keeps `None` because a point-in-time busy% does not exist.
/// * macOS: `load_avg_1m` from `sysctl -n vm.loadavg`; `cpu_busy_percent`
///   from a `host_statistics(HOST_CPU_LOAD_INFO)` tick delta (after sample
///   only, like Linux); memory availability has no cheap equivalent so
///   `mem_available_mb` stays `None`.
/// * Windows: `cpu_busy_percent` from a `GetSystemTimes` tick delta (after
///   sample only); no load-average concept so `load_avg_1m` stays `None`.
/// * other: everything `None` (the whole sample is omitted).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LoadSample {
    /// 1-minute load average. Compare against `HostInfo::cpu_cores` — a value
    /// above the core count means the tester itself was contended and the
    /// measurements may be noisy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_avg_1m: Option<f64>,
    /// CPU busy percentage — the **whole-run mean** from a two-snapshot
    /// [`CpuTicks`] delta spanning the run (Linux `/proc/stat`, macOS
    /// `host_statistics`, Windows `GetSystemTimes`). Kept for compat; it is
    /// the same value as `TestRun.cpu_usage.mean_busy_percent`, which adds
    /// sampled max/p95/steal. Set on the end-of-run sample only; `None` on
    /// the start-of-run sample, on platforms without a collector, and on
    /// runs shorter than the [`MIN_CPU_WINDOW_MS`] trust guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_busy_percent: Option<f64>,
    /// Available (reclaimable) memory in MB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_available_mb: Option<u64>,
}

impl LoadSample {
    /// True when no field was collected — callers store `None` instead.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Best-effort local sample; returns `None` when nothing was collected.
    pub fn collect_local() -> Option<Self> {
        let sample = Self {
            load_avg_1m: detect_load_avg_1m(),
            cpu_busy_percent: None,
            mem_available_mb: detect_mem_available_mb(),
        };
        if sample.is_empty() {
            None
        } else {
            Some(sample)
        }
    }
}

/// Parse the leading 1-minute load average from a `/proc/loadavg`-style line
/// ("0.52 0.58 0.59 1/467 1234") or a macOS `sysctl -n vm.loadavg` line
/// ("{ 1.86 1.99 2.06 }").
fn parse_load_avg_1m(text: &str) -> Option<f64> {
    text.split_whitespace()
        .find(|tok| !tok.starts_with('{'))
        .and_then(|tok| tok.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn detect_load_avg_1m() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        parse_load_avg_1m(&std::fs::read_to_string("/proc/loadavg").ok()?)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()?;
        parse_load_avg_1m(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn detect_mem_available_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb / 1024);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS has no cheap MemAvailable equivalent; Windows/other likewise.
        // Honesty over fabrication — stays None.
        None
    }
}

/// Minimum wall-clock window (ms) a CPU tick delta must span before a busy /
/// steal percentage is derived from it. Below this, tick granularity (100 Hz
/// on typical Linux kernels) turns the ratio into fake precision — the sample
/// is dropped (`None`), never fabricated.
pub const MIN_CPU_WINDOW_MS: u64 = 500;
/// Minimum elapsed ticks a delta must contain (tick-granularity guard for the
/// platforms where one tick is 10 ms; on Windows the counters are 100 ns
/// units so this guard is trivially met once the wall-clock guard passes).
pub const MIN_CPU_WINDOW_TICKS: u64 = 20;

/// Cumulative CPU tick counters snapshotted at a point in time. Two snapshots
/// bracket a window; [`CpuTicks::busy_percent_since`] turns the deltas into a
/// busy percentage (`100 * (1 - idle_delta / total_delta)`, idle including
/// iowait on Linux). Sources:
///
/// * Linux: the aggregate `cpu` line of `/proc/stat`. `steal` is field 8 —
///   time the hypervisor ran somebody else while this vCPU wanted to run.
///   Steal is part of `total` and **not** part of `idle`, so it counts
///   toward busy% (a stolen tester is contended, not idle).
/// * macOS: `host_statistics(HOST_CPU_LOAD_INFO)` via libSystem (no extra
///   dependency; the counters are `u32` and may wrap on very long uptimes —
///   a wrapped delta yields `None`, never a fabricated value). No steal
///   concept/API → `steal` stays `None`.
/// * Windows: `GetSystemTimes` (idle/kernel/user `FILETIME`s, 100 ns units;
///   kernel time includes idle time, see `cpu_ticks_from_windows_times`).
///   No steal counter → `steal` stays `None`.
/// * other: no collector; [`CpuTicks::snapshot`] returns `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTicks {
    /// Ticks spent idle (Linux: idle + iowait; macOS: `CPU_STATE_IDLE`;
    /// Windows: `lpIdleTime`).
    pub idle: u64,
    /// Ticks across all accounted CPU states.
    pub total: u64,
    /// Ticks stolen by the hypervisor (Linux `/proc/stat` field 8 only;
    /// `None` on macOS/Windows and pre-2.6.11 kernels — never fabricated 0).
    pub steal: Option<u64>,
}

impl CpuTicks {
    /// Best-effort snapshot of the platform's cumulative CPU tick counters.
    pub fn snapshot() -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            parse_proc_stat_cpu(&std::fs::read_to_string("/proc/stat").ok()?)
        }
        #[cfg(target_os = "macos")]
        {
            snapshot_cpu_ticks_macos()
        }
        #[cfg(target_os = "windows")]
        {
            snapshot_cpu_ticks_windows()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            None
        }
    }

    /// Busy percentage over the window from `earlier` to `self`, or `None`
    /// when the counters went backwards (reboot/wrap), no time elapsed, or
    /// fewer than [`MIN_CPU_WINDOW_TICKS`] ticks elapsed (tick-granularity
    /// guard — a handful of 10 ms ticks cannot support a percentage).
    pub fn busy_percent_since(&self, earlier: &Self) -> Option<f64> {
        let total_delta = self.total.checked_sub(earlier.total)?;
        let idle_delta = self.idle.checked_sub(earlier.idle)?;
        if total_delta < MIN_CPU_WINDOW_TICKS || idle_delta > total_delta {
            return None;
        }
        Some((1.0 - idle_delta as f64 / total_delta as f64) * 100.0)
    }

    /// Steal percentage over the window from `earlier` to `self`. `None`
    /// when either snapshot has no steal counter (macOS/Windows/old kernels),
    /// the counters went backwards, or the tick-granularity guard fails.
    pub fn steal_percent_since(&self, earlier: &Self) -> Option<f64> {
        let total_delta = self.total.checked_sub(earlier.total)?;
        let steal_delta = self.steal?.checked_sub(earlier.steal?)?;
        if total_delta < MIN_CPU_WINDOW_TICKS || steal_delta > total_delta {
            return None;
        }
        Some(steal_delta as f64 / total_delta as f64 * 100.0)
    }
}

/// One guarded CPU measurement over a bracketed window — the unit the run
/// sampler collects once per second and the whole-run delta reduces to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuWindowSample {
    /// Busy percentage over the window (steal counts as busy, never idle).
    pub busy_percent: f64,
    /// Steal percentage over the window; `None` where the platform has no
    /// steal counter (macOS/Windows).
    pub steal_percent: Option<f64>,
}

/// Turn two tick snapshots bracketing `window_ms` of wall clock into a
/// guarded [`CpuWindowSample`]. Returns `None` (never fake precision) when
/// the window is shorter than [`MIN_CPU_WINDOW_MS`] or the tick deltas fail
/// [`CpuTicks::busy_percent_since`]'s own guards.
pub fn cpu_window_sample(
    before: &CpuTicks,
    after: &CpuTicks,
    window_ms: u64,
) -> Option<CpuWindowSample> {
    if window_ms < MIN_CPU_WINDOW_MS {
        return None;
    }
    Some(CpuWindowSample {
        busy_percent: after.busy_percent_since(before)?,
        steal_percent: after.steal_percent_since(before),
    })
}

/// Run-level tester CPU usage, upgraded from "one whole-run number" to a
/// trustworthy measurement: the whole-run mean plus periodically sampled
/// max/p95 so a short contention burst inside an otherwise-quiet run cannot
/// hide in the average. Additive on the run envelope (`TestRun.cpu_usage`,
/// schema stays 1.0). Every field is honest per platform: `None` means "not
/// measurable here", never 0.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CpuUsage {
    /// Busy% over the whole run window (two-snapshot delta — the same value
    /// mirrored into `client_load_after.cpu_busy_percent` for compat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_busy_percent: Option<f64>,
    /// Highest periodic busy% sample observed during the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_busy_percent: Option<f64>,
    /// 95th-percentile busy% across the periodic samples. Reported only when
    /// `sample_count >= MIN_SAMPLES_P95` (same philosophy as [`Stats::p95`] —
    /// at 1 s cadence a 20 s+ run earns a p95, shorter runs get `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_busy_percent: Option<f64>,
    /// Hypervisor steal% over the whole run window (Linux `/proc/stat` field
    /// 8). `None` on macOS/Windows — no steal concept/API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_steal_percent: Option<f64>,
    /// Highest periodic steal% sample observed during the run (Linux only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steal_percent: Option<f64>,
    /// Number of periodic samples that passed the min-window guards.
    pub sample_count: u32,
    /// Configured cadence of the periodic sampler in milliseconds.
    pub sample_interval_ms: u64,
}

impl CpuUsage {
    /// Aggregate the whole-run delta plus the periodic samples into the
    /// envelope struct. Returns `None` when nothing at all was measured (the
    /// field is then omitted from the JSON entirely).
    pub fn aggregate(
        whole_run: Option<CpuWindowSample>,
        samples: &[CpuWindowSample],
        sample_interval_ms: u64,
    ) -> Option<Self> {
        if whole_run.is_none() && samples.is_empty() {
            return None;
        }
        let mut busy: Vec<f64> = samples.iter().map(|s| s.busy_percent).collect();
        busy.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let max_busy_percent = busy.last().copied();
        // Same sample-size philosophy as Stats::p95: below MIN_SAMPLES_P95 an
        // interpolated p95 is indistinguishable from the max → suppressed.
        let p95_busy_percent =
            (busy.len() >= MIN_SAMPLES_P95).then(|| percentile_from_sorted(&busy, 95.0));
        let max_steal_percent = samples
            .iter()
            .filter_map(|s| s.steal_percent)
            .fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a| a.max(v)))
            });
        Some(Self {
            mean_busy_percent: whole_run.map(|w| w.busy_percent),
            max_busy_percent,
            p95_busy_percent,
            mean_steal_percent: whole_run.and_then(|w| w.steal_percent),
            max_steal_percent,
            sample_count: samples.len().min(u32::MAX as usize) as u32,
            sample_interval_ms,
        })
    }
}

/// Parse the aggregate `cpu` line of a `/proc/stat` dump into [`CpuTicks`].
/// Fields (Linux): user nice system idle iowait irq softirq steal
/// [guest guest_nice]. Idle includes iowait; steal (field 8) is kept in the
/// total but NOT folded into idle — steal is contention, not idleness.
/// guest/guest_nice are excluded from the total (the kernel already folds
/// them into user/nice).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // exercised by unit tests on every platform
fn parse_proc_stat_cpu(text: &str) -> Option<CpuTicks> {
    let line = text.lines().find(|l| {
        l.starts_with("cpu")
            && l.split_whitespace()
                .next()
                .is_some_and(|first| first == "cpu")
    })?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|tok| tok.parse::<u64>())
        .collect::<Result<_, _>>()
        .ok()?;
    // Need at least user/nice/system/idle to say anything meaningful.
    if fields.len() < 4 {
        return None;
    }
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let total = fields.iter().sum();
    // Steal only exists on 2.6.11+ kernels (8th field). Absent → None,
    // never a fabricated 0.
    let steal = fields.get(7).copied();
    Some(CpuTicks { idle, total, steal })
}

/// Build [`CpuTicks`] from Windows `GetSystemTimes` outputs (`FILETIME`s
/// flattened to u64 100 ns units). `kernel_incl_idle` is `lpKernelTime`,
/// which per the API contract **includes** `lpIdleTime` — so
/// `total = kernel + user` already counts idle exactly once and
/// `busy = 1 - idle/total` needs no further subtraction. Windows exposes no
/// hypervisor steal counter → `steal` is `None`. Pure so the
/// kernel-includes-idle math is unit-testable on every platform.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // exercised by unit tests on every platform
fn cpu_ticks_from_windows_times(idle: u64, kernel_incl_idle: u64, user: u64) -> Option<CpuTicks> {
    // Idle bigger than the kernel bucket that contains it → inconsistent read.
    if idle > kernel_incl_idle {
        return None;
    }
    let total = kernel_incl_idle.checked_add(user)?;
    Some(CpuTicks {
        idle,
        total,
        steal: None,
    })
}

#[cfg(target_os = "windows")]
fn snapshot_cpu_ticks_windows() -> Option<CpuTicks> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let zero = || FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (zero(), zero(), zero());
    // SAFETY: out-pointers to three stack FILETIMEs, exactly as documented.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    let flat = |ft: FILETIME| ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    cpu_ticks_from_windows_times(flat(idle), flat(kernel), flat(user))
}

#[cfg(target_os = "macos")]
fn snapshot_cpu_ticks_macos() -> Option<CpuTicks> {
    // host_statistics(HOST_CPU_LOAD_INFO) — plain libSystem symbols, declared
    // here to avoid pulling in a mach bindings crate for one call.
    const HOST_CPU_LOAD_INFO: libc::c_int = 3;
    const CPU_STATE_MAX: usize = 4; // user, system, idle, nice
    const CPU_STATE_IDLE: usize = 2;
    extern "C" {
        fn mach_host_self() -> libc::c_uint;
        fn host_statistics(
            host: libc::c_uint,
            flavor: libc::c_int,
            host_info_out: *mut libc::c_uint,
            host_info_out_cnt: *mut libc::c_uint,
        ) -> libc::c_int;
    }
    let mut ticks = [0u32; CPU_STATE_MAX];
    let mut count = CPU_STATE_MAX as libc::c_uint;
    let kr = unsafe {
        host_statistics(
            mach_host_self(),
            HOST_CPU_LOAD_INFO,
            ticks.as_mut_ptr(),
            &mut count,
        )
    };
    if kr != 0 || (count as usize) < CPU_STATE_MAX {
        return None;
    }
    Some(CpuTicks {
        idle: ticks[CPU_STATE_IDLE] as u64,
        total: ticks.iter().map(|&t| t as u64).sum(),
        // mach has no hypervisor steal concept — honest None.
        steal: None,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Clock-sync validation (measurement-gap #16)
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a one-shot SNTP (RFC 4330) query performed once per run as an
/// independent cross-check of the per-attempt `clock_skew_ms` heuristic.
/// Best-effort: all fields `None`-able, and the whole struct is omitted when
/// the query failed or was disabled.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ClockSync {
    /// NTP server queried (`NETWORKER_NTP_SERVER`, default `pool.ntp.org:123`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ntp_server: Option<String>,
    /// Estimated client clock offset vs the NTP server in ms
    /// (positive = local clock is behind the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<f64>,
    /// SNTP round-trip delay in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_trip_ms: Option<f64>,
}

fn detect_hostname() -> Option<String> {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return Some(h);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            let h = h.trim().to_string();
            if !h.is_empty() {
                return Some(h);
            }
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let out = std::process::Command::new("hostname").output().ok()?;
        let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !h.is_empty() {
            return Some(h);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-attempt record
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestAttempt {
    pub attempt_id: Uuid,
    pub run_id: Uuid,
    pub protocol: Protocol,
    pub sequence_num: u32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub success: bool,
    pub dns: Option<DnsResult>,
    pub tcp: Option<TcpResult>,
    pub tls: Option<TlsResult>,
    pub http: Option<HttpResult>,
    pub udp: Option<UdpResult>,
    pub error: Option<ErrorRecord>,
    /// Number of retries performed before this attempt succeeded (0 = first try succeeded).
    #[serde(default)]
    pub retry_count: u32,
    /// Server-side timing metadata parsed from response headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_timing: Option<ServerTimingResult>,
    /// UDP bulk transfer result (udpdownload / udpupload modes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_throughput: Option<UdpThroughputResult>,
    /// Page-load simulation result (pageload / pageload2 / pageload3 modes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_load: Option<PageLoadResult>,
    /// Real-browser page-load result (browser mode only, requires `--features browser`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserResult>,
    /// HTTP stack that served this request (e.g. "nginx", "iis").
    /// `None` means the default networker-endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_stack: Option<String>,
    /// Latency-under-load / bufferbloat result (`rpm` mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<RpmResult>,
    /// ICMP echo RTT result (`ping` mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping: Option<PingResult>,
    /// Hop-discovery / traceroute-style result (`path` mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathResult>,
    /// IPv4-vs-IPv6 comparison result (`dualstack` mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dualstack: Option<DualStackResult>,
    /// WebSocket upgrade + message-RTT result (`websocket` mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket: Option<WebSocketResult>,
    /// Path-MTU discovery result (`pmtud` mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pmtud: Option<PmtudResult>,
    /// Draft-conformant working-conditions responsiveness result
    /// (`responsiveness` mode only). Additive — absent in pre-Wave-R JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsiveness: Option<ResponsivenessResult>,
    /// STAMP (RFC 8762) probe result (`stamp` mode only). Additive — absent
    /// in pre-Wave-R JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<StampResult>,
    /// Multi-connection capacity result (`mthroughput` mode only). Additive —
    /// absent in pre-Wave-W JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mthroughput: Option<MthroughputResult>,
    /// Benchmark phase this attempt executed in ("warmup", "overhead",
    /// "pilot", "measured", "cooldown"), set at attempt creation by the
    /// benchmark runner. `None` means the attempt was not produced by a
    /// benchmark phase loop and is treated as measured. Additive — schema
    /// stays 1.0; older artifacts without the field fall back to positional
    /// phase reconstruction (see `TestRun::resolved_attempt_phases`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

impl RequestAttempt {
    pub fn total_duration_ms(&self) -> Option<f64> {
        let start = self.started_at;
        let end = self.finished_at?;
        Some((end - start).num_microseconds()? as f64 / 1000.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Protocol enum
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Http1,
    Http2,
    Http3,
    Udp,
    Download,
    Download1,
    Download2,
    Download3,
    Upload,
    Upload1,
    Upload2,
    Upload3,
    /// GET the endpoint download route; same transfer path as `Download`, different report label.
    WebDownload,
    /// POST to the endpoint upload route; same transfer path as `Upload`, different report label.
    WebUpload,
    /// UDP bulk download from the networker-endpoint UDP throughput server (port 9998).
    UdpDownload,
    /// UDP bulk upload to the networker-endpoint UDP throughput server (port 9998).
    UdpUpload,
    /// Multi-connection capacity probe: ramps N parallel HTTP/2 connections
    /// (shared load-gen machinery with `responsiveness`) each streaming the
    /// endpoint's `/download` (then `/upload`) route until AGGREGATE goodput
    /// stabilizes (same moving-average criterion) or the connection cap is
    /// hit, then measures a fixed steady window. This is the Ookla-style
    /// methodology: link CAPACITY, complementing the single-connection
    /// `download`/`upload` figure (ndt7-style per-flow fair share) — the two
    /// answer different questions and diverge exactly on high-BDP/lossy
    /// paths. Per-connection goodput spread and post-transfer kernel TCP
    /// attribution (rwnd/sndbuf/path-limited verdicts, retransmits) explain
    /// the delta. See [`MthroughputResult`].
    Mthroughput,
    /// Latency-under-load / bufferbloat probe: samples unloaded UDP echo RTT,
    /// then loads the link with sustained HTTP downloads from the
    /// networker-endpoint while probing UDP echo RTT at a steady cadence.
    /// Reports RPM (60000 / loaded avg RTT — round-trips per minute, higher
    /// is better) and the bufferbloat factor (loaded/unloaded). NOTE: this is
    /// a UDP-echo-under-single-flow-load diagnostic, NOT the
    /// draft-ietf-ippm-responsiveness methodology — its RPM figure is not
    /// comparable with Apple `networkQuality`/Cloudflare/Ookla RPM (see
    /// [`RpmResult`] docs).
    Rpm,
    /// Working-conditions responsiveness probe conformant with
    /// draft-ietf-ippm-responsiveness (draft-08 parameters): ramps parallel
    /// HTTP/2 load connections against the networker-endpoint's `/download`
    /// (then `/upload`) route until goodput stabilizes (moving-average
    /// stability criterion), while measuring HTTP probe latency on NEW
    /// connections ("foreign" probes: TCP + TLS + GET phases) and ON the
    /// load-generating connections themselves ("self" probes — multiplexed
    /// H2 GETs that share the loaded flow's queue, defeating flow-isolating
    /// AQM blindness). Reports RPM per direction via the draft's trimmed-mean
    /// formula plus capacity at saturation. Unlike `rpm`, this figure IS
    /// comparable with other draft implementations. See
    /// [`ResponsivenessResult`].
    Responsiveness,
    /// STAMP (RFC 8762) probe: sends unauthenticated-mode Session-Sender test
    /// packets to the networker-endpoint's Session-Reflector (UDP port 9997)
    /// at a fixed cadence. The reflector's receive/transmit timestamps and
    /// sequence number yield processing-corrected RTT (T4−T1 − (T3−T2), no
    /// clock sync needed), per-direction delay variation, and DIRECTIONAL
    /// loss (sender→reflector vs reflector→sender). See [`StampResult`].
    Stamp,
    /// ICMP echo probe — measures raw network-layer RTT (min/avg/p95), jitter,
    /// and packet loss to the target host without any TCP/UDP dependency.
    /// Uses unprivileged ICMP datagram sockets (Linux `ping_group_range`,
    /// macOS built-in); reports a clear Config error when the OS denies them.
    Ping,
    /// Hop-discovery probe (traceroute-style): UDP probes with incrementing
    /// TTL map the routers between the tester and the target. On Linux the
    /// per-hop addresses come from `IP_RECVERR` (no raw sockets needed); on
    /// platforms without unprivileged ICMP-error access the probe degrades
    /// honestly to a hop-count estimate + destination reachability and
    /// reports `hops: []` — it never fabricates hop addresses.
    Path,
    /// IPv4-vs-IPv6 comparison: resolves A and AAAA separately, runs an
    /// HTTP GET pinned to each family, and compares per-phase timing
    /// (DNS/TCP/TLS/TTFB/total) with a happy-eyeballs (RFC 8305) verdict.
    /// One working family is a success; the other is reported absent/failed.
    DualStack,
    /// WebSocket probe: full DNS + TCP + TLS timing ladder, then the HTTP 101
    /// upgrade round-trip (`upgrade_ms`), then N echo messages against the
    /// networker-endpoint's `/ws` route — per-message RTT min/avg/p95,
    /// mean inter-probe delay variation (IPDV), and loss. Uses ws:// or
    /// wss:// derived from the target scheme.
    WebSocket,
    /// Path-MTU discovery probe: UDP datagrams with the DF bit set at
    /// binary-searched sizes toward the target. On Linux, ICMP
    /// fragmentation-needed errors (with the next-hop MTU) are read from the
    /// socket error queue unprivileged (`IP_RECVERR`); on macOS `IP_DONTFRAG`
    /// relies on EMSGSIZE surfacing on the connected socket. Pairs with the
    /// endpoint's UDP echo (port 9999) when available so unfragmented delivery
    /// is positively confirmed; without any feedback the probe reports an
    /// honest `path_mtu: None` — it never fabricates. Windows is reported as a
    /// clean unsupported Config error.
    Pmtud,
    /// Standalone DNS resolution probe — resolves the target host and records timing without TCP.
    Dns,
    /// Standalone TLS probe — DNS + TCP + TLS handshake only, no HTTP request.
    /// Collects the full certificate chain, cipher suite, and negotiated ALPN.
    Tls,
    /// TLS resumption probe — two fresh TLS handshakes to the same origin with a real
    /// HTTP request on each connection; success depends on the second handshake resuming.
    TlsResume,
    /// Native-TLS probe — DNS + TCP + platform TLS (SChannel / SecureTransport / OpenSSL)
    /// + HTTP/1.1. Reports which backend was used in `TlsResult.tls_backend`.
    Native,
    /// Curl probe — spawns the `curl` binary and captures per-phase timing from
    /// `--write-out`. Maps to the same result structs as an http1 probe.
    Curl,
    /// HTTP/1.1 page-load: fetches /page manifest then downloads all assets
    /// using up to 6 parallel connections (browser-like).
    PageLoad,
    /// HTTP/2 page-load: same assets but multiplexed over one TLS connection.
    /// Requires an HTTPS target.
    PageLoad2,
    /// HTTP/3 page-load: same assets multiplexed over one QUIC connection.
    /// Requires `--features http3` and an HTTPS target.
    PageLoad3,
    /// Real headless-browser probe via CDP (chromiumoxide).
    /// Requires `--features browser`. Self-skips with an error attempt if Chrome is not found.
    Browser,
    /// Real headless-browser probe forced to HTTP/1.1 (`--disable-http2`).
    Browser1,
    /// Real headless-browser probe forced to HTTP/2 (`--disable-quic`).
    Browser2,
    /// Real headless-browser probe forced to HTTP/3 QUIC (`--enable-quic --origin-to-force-quic-on=…`).
    Browser3,
    /// LagHound SDK probe: an HTTP/1.1 request to a customer-embedded LagHound
    /// endpoint (`<base>/laghound/echo` by default) that splits total request
    /// latency into a NETWORK leg and a SERVER-processing leg using the
    /// `Server-Timing: app;dur=<ms>` header (docs/sdk/contract-v1.md §4).
    /// Reuses the HTTP/1.1 phase-timing path (DNS/TCP/TLS/TTFB/total) and adds
    /// the `X-LagHound-Token` auth header.
    SdkProbe,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Tcp => write!(f, "tcp"),
            Protocol::Http1 => write!(f, "http1"),
            Protocol::Http2 => write!(f, "http2"),
            Protocol::Http3 => write!(f, "http3"),
            Protocol::Udp => write!(f, "udp"),
            Protocol::Download => write!(f, "download"),
            Protocol::Download1 => write!(f, "download1"),
            Protocol::Download2 => write!(f, "download2"),
            Protocol::Download3 => write!(f, "download3"),
            Protocol::Upload => write!(f, "upload"),
            Protocol::Upload1 => write!(f, "upload1"),
            Protocol::Upload2 => write!(f, "upload2"),
            Protocol::Upload3 => write!(f, "upload3"),
            Protocol::WebDownload => write!(f, "webdownload"),
            Protocol::WebUpload => write!(f, "webupload"),
            Protocol::UdpDownload => write!(f, "udpdownload"),
            Protocol::UdpUpload => write!(f, "udpupload"),
            Protocol::Mthroughput => write!(f, "mthroughput"),
            Protocol::Rpm => write!(f, "rpm"),
            Protocol::Responsiveness => write!(f, "responsiveness"),
            Protocol::Stamp => write!(f, "stamp"),
            Protocol::Ping => write!(f, "ping"),
            Protocol::Path => write!(f, "path"),
            Protocol::DualStack => write!(f, "dualstack"),
            Protocol::WebSocket => write!(f, "websocket"),
            Protocol::Pmtud => write!(f, "pmtud"),
            Protocol::Dns => write!(f, "dns"),
            Protocol::Tls => write!(f, "tls"),
            Protocol::TlsResume => write!(f, "tlsresume"),
            Protocol::Native => write!(f, "native"),
            Protocol::Curl => write!(f, "curl"),
            Protocol::PageLoad => write!(f, "pageload"),
            Protocol::PageLoad2 => write!(f, "pageload2"),
            Protocol::PageLoad3 => write!(f, "pageload3"),
            Protocol::Browser => write!(f, "browser"),
            Protocol::Browser1 => write!(f, "browser1"),
            Protocol::Browser2 => write!(f, "browser2"),
            Protocol::Browser3 => write!(f, "browser3"),
            Protocol::SdkProbe => write!(f, "sdkprobe"),
        }
    }
}

/// Metadata for a probe mode, used by the dashboard UI.
#[derive(Debug, Clone, Serialize)]
pub struct ModeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub detail: String,
    pub group: String,
}

impl Protocol {
    /// Return metadata for all available probe modes, grouped by category.
    /// This is the single source of truth for the UI mode pickers.
    pub fn all_modes() -> Vec<ModeInfo> {
        vec![
            // Network
            m(
                "tcp",
                "TCP",
                "Connect",
                "TCP 3-way handshake timing to measure raw connection latency",
                "Network",
            ),
            m(
                "dns",
                "DNS",
                "Resolve",
                "DNS resolution timing for the target hostname",
                "Network",
            ),
            m(
                "tls",
                "TLS",
                "Handshake",
                "TLS handshake via rustls — reports version, cipher, ALPN, cert chain",
                "Network",
            ),
            m(
                "tlsresume",
                "TLS Resume",
                "Warm handshake",
                "Two fresh TLS handshakes with a real HTTP request; the second should resume",
                "Network",
            ),
            m(
                "native",
                "Native TLS",
                "OS TLS stack",
                "Uses SChannel (Win), SecureTransport (macOS), or OpenSSL (Linux)",
                "Network",
            ),
            m(
                "udp",
                "UDP",
                "Round-trip",
                "UDP echo probe — measures RTT, jitter, and packet loss",
                "Network",
            ),
            m(
                "rpm",
                "RPM",
                "Latency under load",
                "Bufferbloat probe — UDP echo RTT idle vs during a sustained download; reports RPM (round-trips per minute) and bufferbloat factor",
                "Network",
            ),
            m(
                "responsiveness",
                "Responsiveness",
                "RPM under load",
                "Working-conditions responsiveness per draft-ietf-ippm-responsiveness — ramps parallel HTTP/2 load connections to saturation while probing latency on new and on the loaded connections; reports RPM per direction and capacity",
                "Network",
            ),
            m(
                "stamp",
                "STAMP",
                "RFC 8762 probe",
                "STAMP (RFC 8762) UDP probe against the endpoint's Session-Reflector (port 9997) — processing-corrected RTT, per-direction delay variation, and directional loss",
                "Network",
            ),
            m(
                "ping",
                "Ping",
                "ICMP echo",
                "ICMP echo RTT probe — measures network-layer RTT, jitter, and packet loss without TCP/UDP (unprivileged ICMP sockets)",
                "Network",
            ),
            m(
                "path",
                "Path",
                "Hop discovery",
                "Traceroute-style probe — UDP probes with rising TTL map the hops toward the target; degrades to an honest hop-count estimate where ICMP errors are not readable unprivileged",
                "Network",
            ),
            m(
                "dualstack",
                "Dual Stack",
                "IPv4 vs IPv6",
                "Resolves A and AAAA separately, runs an HTTP GET pinned to each family, and compares per-phase timing with a happy-eyeballs verdict",
                "Network",
            ),
            m(
                "websocket",
                "WebSocket",
                "Msg round-trip",
                "WebSocket probe — DNS/TCP/TLS ladder, HTTP 101 upgrade time, then echo-message RTT, jitter, and loss over the open socket",
                "Network",
            ),
            m(
                "pmtud",
                "Path MTU",
                "DF-bit discovery",
                "Path-MTU discovery — binary-searches DF-flagged UDP datagram sizes toward the target; reads ICMP fragmentation-needed where the platform allows and never fabricates an MTU",
                "Network",
            ),
            // HTTP
            m(
                "http1",
                "HTTP/1.1",
                "Single request",
                "Full HTTP/1.1 request: DNS + TCP + TLS + request/response",
                "HTTP",
            ),
            m(
                "http2",
                "HTTP/2",
                "Multiplexed",
                "HTTP/2 over TLS with ALPN h2 negotiation",
                "HTTP",
            ),
            m(
                "http3",
                "HTTP/3",
                "QUIC",
                "HTTP/3 over QUIC (UDP) — 0-RTT capable",
                "HTTP",
            ),
            m(
                "curl",
                "Curl",
                "Via curl CLI",
                "Spawns curl binary, captures per-phase timing from --write-out",
                "HTTP",
            ),
            m(
                "sdkprobe",
                "SDK Probe",
                "Server split",
                "Probes a customer-embedded LagHound endpoint — splits total time into DNS, TCP, TLS, network transfer, and server processing via Server-Timing",
                "HTTP",
            ),
            // Page Load (Native)
            m(
                "pageload",
                "H1",
                "6 parallel connections",
                "Fetches page manifest + assets using 6 parallel HTTP/1.1 connections (browser-like)",
                "Page Load (Native)",
            ),
            m(
                "pageload2",
                "H2",
                "Multiplexed",
                "Same assets multiplexed over a single TLS/HTTP2 connection",
                "Page Load (Native)",
            ),
            m(
                "pageload3",
                "H3",
                "QUIC",
                "Same assets multiplexed over a single QUIC connection",
                "Page Load (Native)",
            ),
            // Page Load (Browser)
            m(
                "browser1",
                "H1",
                "Chrome HTTP/1.1",
                "Chrome headless with HTTP/2 disabled — forces HTTP/1.1",
                "Page Load (Browser)",
            ),
            m(
                "browser2",
                "H2",
                "Chrome HTTP/2",
                "Chrome headless with QUIC disabled — forces HTTP/2",
                "Page Load (Browser)",
            ),
            m(
                "browser3",
                "H3",
                "Chrome QUIC",
                "Chrome headless with QUIC forced via origin flag + SPKI cert pinning",
                "Page Load (Browser)",
            ),
            // Throughput
            m(
                "download",
                "Download",
                "Server\u{2192}client",
                "Large payload download via HTTP — measures sustained throughput",
                "Throughput",
            ),
            m(
                "upload",
                "Upload",
                "Client\u{2192}server",
                "Large payload upload via HTTP POST — measures sustained throughput",
                "Throughput",
            ),
            m(
                "download1",
                "Download H1",
                "H1 download",
                "Throughput download forced over HTTP/1.1",
                "Throughput",
            ),
            m(
                "download2",
                "Download H2",
                "H2 download",
                "Throughput download over HTTP/2 multiplexed stream",
                "Throughput",
            ),
            m(
                "download3",
                "Download H3",
                "H3 download",
                "Throughput download over QUIC/HTTP3",
                "Throughput",
            ),
            m(
                "upload1",
                "Upload H1",
                "H1 upload",
                "Throughput upload forced over HTTP/1.1",
                "Throughput",
            ),
            m(
                "upload2",
                "Upload H2",
                "H2 upload",
                "Throughput upload over HTTP/2",
                "Throughput",
            ),
            m(
                "upload3",
                "Upload H3",
                "H3 upload",
                "Throughput upload over QUIC/HTTP3",
                "Throughput",
            ),
            m(
                "webdownload",
                "Web Download",
                "HTTP GET",
                "Download via /download endpoint route",
                "Throughput",
            ),
            m(
                "webupload",
                "Web Upload",
                "HTTP POST",
                "Upload via /upload endpoint route",
                "Throughput",
            ),
            m(
                "udpdownload",
                "UDP Download",
                "UDP bulk DL",
                "Bulk download via UDP throughput server (port 9998)",
                "Throughput",
            ),
            m(
                "udpupload",
                "UDP Upload",
                "UDP bulk UL",
                "Bulk upload via UDP throughput server (port 9998)",
                "Throughput",
            ),
            m(
                "mthroughput",
                "Multi-Conn",
                "Link capacity",
                "Multi-connection capacity probe — ramps parallel HTTP/2 connections against /download then /upload until aggregate goodput stabilizes (Ookla-style link capacity vs the single-connection fair share of download/upload); reports per-connection spread and TCP-attribution verdicts",
                "Throughput",
            ),
        ]
    }
}

fn m(id: &str, name: &str, desc: &str, detail: &str, group: &str) -> ModeInfo {
    ModeInfo {
        id: id.into(),
        name: name.into(),
        description: desc.into(),
        detail: detail.into(),
        group: group.into(),
    }
}

impl std::str::FromStr for Protocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tcp" => Ok(Protocol::Tcp),
            "http1" => Ok(Protocol::Http1),
            "http2" => Ok(Protocol::Http2),
            "http3" => Ok(Protocol::Http3),
            "udp" => Ok(Protocol::Udp),
            "download" => Ok(Protocol::Download),
            "download1" => Ok(Protocol::Download1),
            "download2" => Ok(Protocol::Download2),
            "download3" => Ok(Protocol::Download3),
            "upload" => Ok(Protocol::Upload),
            "upload1" => Ok(Protocol::Upload1),
            "upload2" => Ok(Protocol::Upload2),
            "upload3" => Ok(Protocol::Upload3),
            "webdownload" => Ok(Protocol::WebDownload),
            "webupload" => Ok(Protocol::WebUpload),
            "udpdownload" => Ok(Protocol::UdpDownload),
            "udpupload" => Ok(Protocol::UdpUpload),
            "mthroughput" => Ok(Protocol::Mthroughput),
            "rpm" => Ok(Protocol::Rpm),
            "responsiveness" => Ok(Protocol::Responsiveness),
            "stamp" => Ok(Protocol::Stamp),
            "ping" => Ok(Protocol::Ping),
            "path" => Ok(Protocol::Path),
            "dualstack" => Ok(Protocol::DualStack),
            "websocket" => Ok(Protocol::WebSocket),
            "pmtud" => Ok(Protocol::Pmtud),
            "dns" => Ok(Protocol::Dns),
            "tls" => Ok(Protocol::Tls),
            "tlsresume" | "tls-resume" => Ok(Protocol::TlsResume),
            "native" => Ok(Protocol::Native),
            "curl" => Ok(Protocol::Curl),
            "pageload" => Ok(Protocol::PageLoad),
            "pageload2" => Ok(Protocol::PageLoad2),
            "pageload3" => Ok(Protocol::PageLoad3),
            "browser" => Ok(Protocol::Browser),
            "browser1" => Ok(Protocol::Browser1),
            "browser2" => Ok(Protocol::Browser2),
            "browser3" => Ok(Protocol::Browser3),
            "sdkprobe" => Ok(Protocol::SdkProbe),
            other => Err(format!("Unknown protocol: {other}")),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-result types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsResult {
    pub query_name: String,
    pub resolved_ips: Vec<String>,
    pub duration_ms: f64,
    pub started_at: DateTime<Utc>,
    pub success: bool,
    /// Identity of the resolver used, e.g. `"system (192.168.1.1:53)"` or
    /// `"google-fallback (8.8.8.8:53)"`. `None` when the probe cannot know
    /// (e.g. curl's own resolution). Additive, serde-defaulted — absent in
    /// pre-0.28.19 JSON. (Trust audit V1.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
    /// Duration of the A-record lookup alone (ms). Populated by the standalone
    /// `dns` probe mode only (measurement-gap #6); other modes resolve via a
    /// single dual-stack lookup and leave this None. Additive, serde-defaulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_ms: Option<f64>,
    /// Duration of the AAAA-record lookup alone (ms). `dns` probe mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aaaa_ms: Option<f64>,
    /// Number of A records in the answer. `dns` probe mode only; None when the
    /// A lookup was skipped (`--ipv6-only`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_record_count: Option<u32>,
    /// Number of AAAA records in the answer. `dns` probe mode only; None when
    /// the AAAA lookup was skipped (`--ipv4-only`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aaaa_record_count: Option<u32>,
    /// CNAME chain from the answer section, in resolution order (targets only:
    /// `query_name → cname_chain[0] → cname_chain[1] → …`). Empty when the
    /// name resolves directly. `dns` probe mode only. Additive, serde-defaulted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cname_chain: Vec<String>,
    /// Minimum TTL (seconds) of the A answer — the cacheability window a
    /// resolver/client honors before re-querying. `dns` probe mode only; None
    /// when the A lookup was skipped or returned no records (M2 D4). Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_ttl_secs: Option<u32>,
    /// Minimum TTL (seconds) of the AAAA answer. `dns` probe mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aaaa_ttl_secs: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpResult {
    pub local_addr: Option<String>,
    pub remote_addr: String,
    pub connect_duration_ms: f64,
    pub attempt_count: u32,
    pub started_at: DateTime<Utc>,
    pub success: bool,
    /// MSS as reported by TCP_MAXSEG setsockopt (best-effort, Unix only).
    pub mss_bytes: Option<u32>,
    /// Smoothed RTT in ms from TCP_INFO (Linux) or TCP_CONNECTION_INFO (macOS).
    pub rtt_estimate_ms: Option<f64>,
    // ── Extended kernel stats (TCP_INFO / TCP_CONNECTION_INFO) ─────────────────
    /// Segments currently queued for retransmit (tcpi_retransmits).
    #[serde(default)]
    pub retransmits: Option<u32>,
    /// Lifetime retransmission count (tcpi_total_retrans).
    #[serde(default)]
    pub total_retrans: Option<u32>,
    /// Congestion window in segments (tcpi_snd_cwnd).
    #[serde(default)]
    pub snd_cwnd: Option<u32>,
    /// Slow-start threshold; None when set to the kernel sentinel (infinite).
    #[serde(default)]
    pub snd_ssthresh: Option<u32>,
    /// RTT variance in ms (tcpi_rttvar).
    #[serde(default)]
    pub rtt_variance_ms: Option<f64>,
    /// Receiver advertised window in bytes (tcpi_rcv_space).
    #[serde(default)]
    pub rcv_space: Option<u32>,
    /// Segments sent since connection start (Linux ≥ 4.2).
    #[serde(default)]
    pub segs_out: Option<u32>,
    /// Segments received since connection start (Linux ≥ 4.2).
    #[serde(default)]
    pub segs_in: Option<u32>,
    /// Congestion control algorithm name, e.g. "cubic", "bbr" (TCP_CONGESTION).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub congestion_algorithm: Option<String>,
    /// Estimated TCP delivery rate in bytes/sec (Linux ≥ 4.9: tcpi_delivery_rate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_rate_bps: Option<u64>,
    /// Minimum RTT ever observed by the kernel in ms (Linux ≥ 4.9: tcpi_min_rtt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_rtt_ms: Option<f64>,
}

/// TCP kernel statistics (TCP_INFO on Linux, TCP_CONNECTION_INFO on macOS)
/// sampled on a `dup(2)` of the probe socket **after** the HTTP transfer
/// completed — the point where cwnd, retransmission counts, and delivery rate
/// are meaningful (measurement gap #5).
///
/// Contrast with the same-named fields on [`TcpResult`], which are sampled at
/// connect time (fresh connection: initial cwnd, zero retransmissions).
///
/// Every field is optional and additive: absent on Windows and on kernels
/// that do not report the field. `schema_version` stays 1.0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SocketStats {
    /// Maximum Segment Size in bytes (TCP_MAXSEG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mss_bytes: Option<u32>,
    /// Smoothed RTT in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_estimate_ms: Option<f64>,
    /// Segments currently queued for retransmit (tcpi_retransmits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retransmits: Option<u32>,
    /// Lifetime retransmission count over the connection (tcpi_total_retrans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_retrans: Option<u32>,
    /// Congestion window in segments (tcpi_snd_cwnd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snd_cwnd: Option<u32>,
    /// Slow-start threshold; None when set to the kernel sentinel (infinite).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snd_ssthresh: Option<u32>,
    /// RTT variance in ms (tcpi_rttvar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_variance_ms: Option<f64>,
    /// Receiver advertised window in bytes (tcpi_rcv_space). Linux only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rcv_space: Option<u32>,
    /// Segments sent since connection start (Linux ≥ 4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segs_out: Option<u32>,
    /// Segments received since connection start (Linux ≥ 4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segs_in: Option<u32>,
    /// Congestion control algorithm name, e.g. "cubic", "bbr" (TCP_CONGESTION).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub congestion_algorithm: Option<String>,
    /// Estimated TCP delivery rate in bytes/sec (Linux ≥ 4.9: tcpi_delivery_rate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_rate_bps: Option<u64>,
    /// Minimum RTT ever observed by the kernel in ms (Linux ≥ 4.9: tcpi_min_rtt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_rtt_ms: Option<f64>,
    // ── B.2 full Linux tcp_info (all additive, Linux-only, None elsewhere) ──
    /// µs the connection was busy sending data (Linux ≥ 4.10: tcpi_busy_time).
    /// Denominator of the throughput-attribution triad below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_time_us: Option<u64>,
    /// µs of busy time limited by the peer's receive window (Linux ≥ 4.10:
    /// tcpi_rwnd_limited) — throughput bottleneck was the receiver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rwnd_limited_us: Option<u64>,
    /// µs of busy time limited by the local send buffer (Linux ≥ 4.10:
    /// tcpi_sndbuf_limited) — throughput bottleneck was local, not the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sndbuf_limited_us: Option<u64>,
    /// Bytes acked by the peer (Linux ≥ 4.1: tcpi_bytes_acked, RFC 4898).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_acked: Option<u64>,
    /// Bytes sent incl. retransmissions (Linux ≥ 4.19: tcpi_bytes_sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_sent: Option<u64>,
    /// Bytes retransmitted (Linux ≥ 4.19: tcpi_bytes_retrans) — RFC 6349
    /// retransmitted-bytes-ratio numerator over `bytes_sent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_retrans: Option<u64>,
    /// Data packets delivered to the peer (Linux ≥ 4.18: tcpi_delivered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<u32>,
    /// Delivered packets that carried an ECN CE mark (Linux ≥ 4.18:
    /// tcpi_delivered_ce) — real ECN/L4S congestion signal (RFC 3168/9330).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_ce: Option<u32>,
    /// ECN was negotiated at session init (tcpi_options TCPI_OPT_ECN).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecn_negotiated: Option<bool>,
    /// TCP Fast Open data rode the SYN (tcpi_options TCPI_OPT_SYN_DATA).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tfo_used: Option<bool>,
    /// `delivery_rate_bps` was application-limited (Linux ≥ 4.9 bitfield) —
    /// when true the delivery rate is NOT a path-capacity signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_limited: Option<bool>,
    /// Kernel pacing rate in bytes/sec (Linux ≥ 3.15: tcpi_pacing_rate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pacing_rate_bps: Option<u64>,
    /// Bytes queued but unsent at sample time (Linux ≥ 4.6: tcpi_notsent_bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notsent_bytes: Option<u32>,
    /// Reordering events observed (Linux ≥ 4.19: tcpi_reord_seen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reord_seen: Option<u32>,
    /// DSACK-reported duplicates (Linux ≥ 4.19: tcpi_dsack_dups) — with
    /// `reord_seen`, separates spurious retransmission from genuine loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsack_dups: Option<u32>,
    /// Receiver-side RTT estimate in ms (tcpi_rcv_rtt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rcv_rtt_ms: Option<f64>,
}

impl SocketStats {
    /// True when the kernel reported nothing (unsupported platform or failed
    /// getsockopt) — callers use this to store `None` instead of an all-empty
    /// object.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// QUIC transport statistics sampled from `quinn::Connection::stats()`
/// **after** the response body completed, before the connection is closed —
/// the QUIC analogue of the post-transfer TCP kernel snapshot
/// ([`SocketStats`], sampled on a dup of the probe socket). Deep-measurement
/// audit M1 §B.1 / M3 §G1: h3 probes previously reported zero transport facts
/// while TCP modes carried full kernel stats.
///
/// QUIC is userspace, so unlike `TCP_INFO` this is identical on
/// Linux/macOS/Windows and needs no privileges. Sources (quinn-proto 0.11
/// `ConnectionStats`): `PathStats` (rtt, cwnd, loss, congestion, DPLPMTUD)
/// and `UdpStats` (datagram/byte counts per direction).
///
/// NOT exposed by quinn 0.11 and therefore honestly absent (not faked):
/// ECN counters, PTO/loss-timer episode counts, and per-ACK delivery rate.
///
/// Every field is optional and additive (serde-defaulted, skipped when
/// `None`); `schema_version` stays 1.0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QuicStats {
    /// Current smoothed path RTT estimate in ms (`PathStats::rtt`) — the QUIC
    /// analogue of `tcpi_rtt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    /// Congestion window in **bytes** (`PathStats::cwnd`). QUIC congestion
    /// control is byte-based (RFC 9002) — deliberately NOT converted to
    /// segments, so this is not directly comparable to
    /// [`SocketStats::snd_cwnd`] (segments) without multiplying by MSS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwnd_bytes: Option<u64>,
    /// Largest UDP payload the path currently supports in bytes
    /// (`PathStats::current_mtu`) — the connection's live DPLPMTUD (RFC 8899)
    /// verdict; cross-checks the `pmtud` probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mtu: Option<u16>,
    /// Packets declared lost on this path (`PathStats::lost_packets`) — with
    /// `congestion_events`, the QUIC analogue of `total_retrans`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_packets: Option<u64>,
    /// Bytes declared lost on this path (`PathStats::lost_bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_bytes: Option<u64>,
    /// Packets sent on this path (`PathStats::sent_packets`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_packets: Option<u64>,
    /// Congestion events (loss/ECN-triggered cwnd reductions,
    /// `PathStats::congestion_events`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub congestion_events: Option<u64>,
    /// DPLPMTUD probe packets sent (`PathStats::sent_plpmtud_probes`; also
    /// counted by `sent_packets`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_plpmtud_probes: Option<u64>,
    /// DPLPMTUD probe packets lost (`PathStats::lost_plpmtud_probes`; ignored
    /// by `lost_packets`/`lost_bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_plpmtud_probes: Option<u64>,
    /// Times a path MTU black hole was detected
    /// (`PathStats::black_holes_detected`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub black_holes_detected: Option<u64>,
    /// UDP datagrams transmitted on the connection (`udp_tx.datagrams`).
    /// May differ from `sent_packets`: QUIC can coalesce packets into one
    /// datagram and uses GSO batching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_tx_datagrams: Option<u64>,
    /// UDP payload bytes transmitted on the connection (`udp_tx.bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_tx_bytes: Option<u64>,
    /// UDP datagrams received on the connection (`udp_rx.datagrams`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_rx_datagrams: Option<u64>,
    /// UDP payload bytes received on the connection (`udp_rx.bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_rx_bytes: Option<u64>,
    /// Congestion control algorithm of OUR client configuration — quinn's
    /// `TransportConfig` default (Cubic) since the probes never override it.
    /// Honestly labeled `"cubic (client-config)"`: unlike
    /// [`SocketStats::congestion_algorithm`] (queried live from the kernel
    /// via `TCP_CONGESTION`), quinn exposes no runtime controller query, so
    /// this records configuration, not an observed kernel fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub congestion_algorithm: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Server-side timing
// ─────────────────────────────────────────────────────────────────────────────

/// Metadata extracted from X-Networker-* and Server-Timing response headers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerTimingResult {
    /// Echoed X-Networker-Request-Id from the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Server wall-clock time from X-Networker-Server-Timestamp header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_timestamp: Option<DateTime<Utc>>,
    /// Rough one-way clock skew estimate: (server_ts − client_send_at) − ttfb_ms/2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_skew_ms: Option<f64>,
    /// Body drain time on server side (Server-Timing: recv;dur=X, upload only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recv_body_ms: Option<f64>,
    /// Server processing time (Server-Timing: proc;dur=X, download only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing_ms: Option<f64>,
    /// Total server time (Server-Timing: total;dur=X).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_server_ms: Option<f64>,
    /// LagHound SDK server processing time (Server-Timing: app;dur=X). This is
    /// the authoritative "server did work" number for the sdkprobe network-vs-
    /// server split (docs/sdk/contract-v1.md §4). Populated for any response
    /// that carries `app;dur`; None for the classic networker-endpoint which
    /// emits `proc`/`recv`/`total` instead. Additive, serde-defaulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_ms: Option<f64>,
    /// Server-side portion of total request latency for the sdkprobe split (ms).
    /// `app;dur` when present, else falls back to `total;dur`. This is the
    /// "how much of the latency was the customer's app" half of the report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_ms: Option<f64>,
    /// Network-transfer portion of total request latency for the sdkprobe split
    /// (ms): `max(0, ttfb_ms − server_ms)` — request upstream + response first
    /// byte, everything that was NOT server processing. Clamped to 0 (and the
    /// attempt flagged via `split_anomaly`) when the reported `server_ms`
    /// exceeds the measured wall (clock/measure anomaly, contract §4.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_ms: Option<f64>,
    /// True when the network/server split was clamped because reported
    /// `server_ms > ttfb_ms` (a clock or measurement anomaly). Lets reports
    /// flag the datapoint rather than silently present a 0ms network leg.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub split_anomaly: bool,
    /// Server binary version from X-Networker-Server-Version header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    /// Server-side voluntary context switches for this request
    /// (from Server-Timing: csw-v;dur=N, where N is the count).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srv_csw_voluntary: Option<u64>,
    /// Server-side involuntary context switches for this request
    /// (from Server-Timing: csw-i;dur=N, where N is the count).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srv_csw_involuntary: Option<u64>,
}

/// A single certificate in the peer's certificate chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertEntry {
    pub subject: String,
    pub issuer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<DateTime<Utc>>,
    /// Subject Alternative Names (DNS names and IP addresses).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sans: Vec<String>,
    /// Public key algorithm, e.g. "RSA", "ECDSA P-256", "Ed25519". Parsed from
    /// the certificate's SubjectPublicKeyInfo. Additive (measurement-gap #7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_algorithm: Option<String>,
    /// Public key size in bits (modulus size for RSA, curve size for ECDSA).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_size_bits: Option<u32>,
    /// Signature algorithm the certificate was signed with, e.g.
    /// "SHA256 with RSA", "ECDSA with SHA256". Falls back to the raw OID string
    /// for algorithms outside the common set. Additive (measurement-gap #7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_algorithm: Option<String>,
}

/// Trust-path diagnosis of the certificate chain the server actually presented,
/// derived purely from the captured [`CertEntry`] chain (deep-measurement M2
/// D5/D9). It does not validate against a trust store — it reports structural
/// facts a CLI reveals that a browser hides (browsers cache intermediates, so
/// a server that forgets to send one still "works" for them but breaks
/// non-browser clients). DN comparison is exact-string over the same extractor,
/// so the checks are heuristic by design.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChainDiagnosis {
    /// Number of certificates the server sent (leaf-first).
    pub chain_length: u32,
    /// Whether each cert's issuer DN equals the next cert's subject DN (the
    /// chain links cleanly). `None` when fewer than two certs were sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links_consistent: Option<bool>,
    /// The leaf is not self-signed AND its issuer DN is not the subject of any
    /// cert the server sent — a client without the intermediate cached will
    /// fail to build a path. The classic "works in my browser" misconfig.
    pub missing_intermediate_suspected: bool,
    /// The leaf's subject DN equals its issuer DN (self-signed).
    pub self_signed_leaf: bool,
    /// Some subject DN appears more than once with differing issuers — a
    /// cross-sign shape (e.g. a root cross-signed by an older root).
    pub cross_signed_subjects: bool,
}

/// Derive [`ChainDiagnosis`] from a leaf-first cert chain. `None` for an empty
/// chain (nothing to diagnose). Pure/deterministic.
pub fn diagnose_chain(chain: &[CertEntry]) -> Option<ChainDiagnosis> {
    let leaf = chain.first()?;
    let len = chain.len();

    let self_signed_leaf = leaf.subject == leaf.issuer;

    let links_consistent = if len >= 2 {
        Some(chain.windows(2).all(|w| w[0].issuer == w[1].subject))
    } else {
        None
    };

    // Missing intermediate: leaf isn't self-signed, and its issuer isn't the
    // subject of any cert the server bothered to send.
    let issuer_present = chain.iter().any(|c| c.subject == leaf.issuer);
    let missing_intermediate_suspected = !self_signed_leaf && !issuer_present;

    // Cross-sign: a subject DN repeated with two different issuers.
    let mut cross_signed_subjects = false;
    for (i, a) in chain.iter().enumerate() {
        for b in &chain[i + 1..] {
            if a.subject == b.subject && a.issuer != b.issuer {
                cross_signed_subjects = true;
            }
        }
    }

    Some(ChainDiagnosis {
        chain_length: len as u32,
        links_consistent,
        missing_intermediate_suspected,
        self_signed_leaf,
        cross_signed_subjects,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsResult {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub alpn_negotiated: Option<String>,
    pub cert_subject: Option<String>,
    pub cert_issuer: Option<String>,
    pub cert_expiry: Option<DateTime<Utc>>,
    pub handshake_duration_ms: f64,
    pub started_at: DateTime<Utc>,
    pub success: bool,
    /// Full certificate chain returned by the server (leaf cert first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cert_chain: Vec<CertEntry>,
    /// Structural trust-path diagnosis of `cert_chain` (M2 D5/D9). Populated by
    /// the cert-focused probes (tls / tlsresume / native); `None` on incidental
    /// transfer handshakes and when no chain was captured. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_diagnosis: Option<ChainDiagnosis>,
    /// TLS backend that performed the handshake.
    /// "rustls" for the default backend; "native/schannel", "native/secure-transport",
    /// or "native/openssl" for the `native` probe mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_backend: Option<String>,
    /// True when the handshake reused prior session state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed: Option<bool>,
    /// rustls handshake classification: full, full-hrr, or resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_kind: Option<String>,
    /// Number of TLS 1.3 tickets observed on this connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls13_tickets_received: Option<u32>,
    /// For tlsresume probes: the first/cold handshake duration before the resumed attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_handshake_duration_ms: Option<f64>,
    /// For tlsresume probes: handshake kind from the first/cold connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_handshake_kind: Option<String>,
    /// HTTP status from the first/cold request when a real request was used to seed resumption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_http_status_code: Option<u16>,
    /// HTTP status from this connection when a real request was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status_code: Option<u16>,
    /// Whether the server presented a stapled OCSP response for the leaf
    /// certificate during the handshake. Observed via the rustls certificate
    /// verifier, so it is None when verification did not run for this
    /// connection (resumed handshakes) or the backend can't observe it
    /// (native/curl paths). For `tlsresume`, reflects the cold connection.
    /// Additive (measurement-gap #7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocsp_stapled: Option<bool>,
    /// Length of the stapled OCSP response in bytes; only set when
    /// `ocsp_stapled` is `Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocsp_response_bytes: Option<u32>,
    /// HTTP/3 (QUIC) only: whether the follow-up connection provably resumed
    /// the TLS 1.3 session. quinn does not expose rustls' handshake kind for
    /// QUIC, so this is verified via early-data acceptance: `Some(true)` iff
    /// the server accepted 0-RTT (which requires PSK resumption). `Some(false)`
    /// when no session ticket was available or the server rejected early data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quic_resumed: Option<bool>,
    /// HTTP/3 (QUIC) only: the follow-up connection had 0-RTT keys available
    /// (a TLS 1.3 session ticket with an early-data allowance) and sent the
    /// request in 0-RTT early data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_rtt_attempted: Option<bool>,
    /// HTTP/3 (QUIC) only: the server accepted the 0-RTT early data
    /// (quinn `ZeroRttAccepted` resolved true). Only present when
    /// `zero_rtt_attempted` is `Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_rtt_accepted: Option<bool>,
    /// HTTP/3 (QUIC) only: handshake-completion time of the follow-up
    /// (resumption/0-RTT) connection, comparable against
    /// `handshake_duration_ms` (the cold/full handshake of this attempt's
    /// primary connection) to quantify the resumption latency win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quic_resumed_handshake_ms: Option<f64>,
}

/// Security-relevant response headers derived from the already-captured
/// `HttpResult::response_headers` at result-build time (measurement-gap #14).
/// Pure parsing — no additional network work. `None` fields mean the header
/// was absent (or, for parsed values, unparseable).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SecurityHeaders {
    /// Raw `Strict-Transport-Security` header value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hsts: Option<String>,
    /// `max-age` directive parsed out of the HSTS value, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hsts_max_age_secs: Option<u64>,
    /// Whether a `Content-Security-Policy` header was present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp_present: Option<bool>,
    /// Whether `X-Content-Type-Options: nosniff` was present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_content_type_options_nosniff: Option<bool>,
    /// Raw `X-Frame-Options` header value (e.g. "DENY", "SAMEORIGIN").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_frame_options: Option<String>,
    /// Raw `Referrer-Policy` header value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer_policy: Option<String>,
    /// Raw `Server` header value (software disclosure signal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_header: Option<String>,
}

impl SecurityHeaders {
    /// Derive the audit from captured response headers. Returns `None` when
    /// the header list is empty (nothing was captured — e.g. curl probes),
    /// so absence of data is never presented as "no security headers".
    pub fn from_response_headers(headers: &[(String, String)]) -> Option<Self> {
        if headers.is_empty() {
            return None;
        }
        let find = |name: &str| -> Option<String> {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_string())
        };
        let hsts = find("strict-transport-security");
        let hsts_max_age_secs = hsts.as_deref().and_then(parse_hsts_max_age);
        let x_content_type_options_nosniff =
            Some(find("x-content-type-options").is_some_and(|v| {
                v.split(',')
                    .any(|t| t.trim().eq_ignore_ascii_case("nosniff"))
            }));
        Some(Self {
            hsts,
            hsts_max_age_secs,
            csp_present: Some(find("content-security-policy").is_some()),
            x_content_type_options_nosniff,
            x_frame_options: find("x-frame-options"),
            referrer_policy: find("referrer-policy"),
            server_header: find("server"),
        })
    }
}

/// Parse the `max-age` directive (seconds) out of a raw
/// `Strict-Transport-Security` value. Case-insensitive, tolerates whitespace
/// and quoted values; returns `None` for malformed or missing max-age.
fn parse_hsts_max_age(value: &str) -> Option<u64> {
    for directive in value.split(';') {
        if let Some((key, val)) = directive.split_once('=') {
            if key.trim().eq_ignore_ascii_case("max-age") {
                return val.trim().trim_matches('"').parse().ok();
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResult {
    pub negotiated_version: String,
    pub status_code: u16,
    pub headers_size_bytes: usize,
    /// Response body bytes actually received, as delivered by the HTTP layer.
    /// hyper performs no transparent decompression, so when the server
    /// compresses (see `content_encoding` / `--accept-encoding`) this is the
    /// compressed on-the-wire body size, not the decoded size.
    pub body_size_bytes: usize,
    /// Probe TTFB (ms): request write start → response headers received, on
    /// an already-established connection. Excludes DNS/TCP/TLS and the HTTP
    /// connection handshake, which are reported as their own phases. For
    /// upload probes this window deliberately spans the request-body write
    /// plus the server draining it (see `runner/throughput.rs`). NOT
    /// comparable with [`BrowserResult::ttfb_ms`], which uses the browser
    /// navigation-relative definition.
    pub ttfb_ms: f64,
    pub total_duration_ms: f64,
    pub redirect_count: u32,
    pub started_at: DateTime<Utc>,
    pub response_headers: Vec<(String, String)>,
    /// Bytes requested (download) or sent (upload); 0 for normal probes.
    #[serde(default)]
    pub payload_bytes: usize,
    /// Measured throughput in MB/s; None for normal latency probes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throughput_mbps: Option<f64>,
    /// True end-to-end goodput = payload_bytes / (dns_ms + tcp_ms + tls_ms + total_http_ms).
    /// Only set for throughput probes (download/upload/webdownload/webupload). None otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goodput_mbps: Option<f64>,
    /// Process CPU time (user + system) consumed during this probe (ms).
    /// Enables H1 vs H2 vs H3 CPU overhead comparison; highest for HTTP/3 (QUIC userspace).
    /// None on Windows or if measurement fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<f64>,
    /// Client-side voluntary context switches during this probe (getrusage ru_nvcsw delta).
    /// Unix only; None on Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csw_voluntary: Option<u64>,
    /// Client-side involuntary context switches during this probe (getrusage ru_nivcsw delta).
    /// Unix only; None on Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csw_involuntary: Option<u64>,
    /// Time spent in the HTTP connection handshake (hyper HTTP/1.1 setup or
    /// HTTP/2 preface + SETTINGS exchange) before the request was written (ms).
    /// Excluded from throughput transfer windows so throughput measures
    /// transfer, not connection setup. None for probes that don't measure it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_handshake_ms: Option<f64>,
    /// TCP kernel stats sampled after the transfer completed (see
    /// [`SocketStats`]). Present for http1/http2 and the TCP-based throughput
    /// modes on Linux/macOS; None on Windows, for HTTP/3 (QUIC/UDP), and for
    /// probes that do not own the socket (curl, browser). Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_stats: Option<SocketStats>,
    /// Response `Content-Encoding` header value (e.g. "gzip", "br", "zstd").
    /// By default probes send **no** `Accept-Encoding` header, so
    /// well-behaved origins answer identity and this stays `None`. Opt in
    /// with `--accept-encoding` (env `NETWORKER_ACCEPT_ENCODING=true`), which
    /// sends `Accept-Encoding: gzip, br, zstd` on http1/http2/curl probe
    /// requests; the probe never decompresses, so `body_size_bytes` remains
    /// the wire size. Throughput modes always negotiate identity (payload
    /// sizes are the measurement contract). None when the header is absent.
    /// Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    /// Declared response `Content-Length` header value in bytes. None when
    /// the header is absent (e.g. chunked transfer encoding). Compare with
    /// `body_size_bytes` (bytes actually received) to spot truncation or
    /// on-the-wire compression. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_length_header: Option<u64>,
    /// Security-header audit derived from `response_headers` at result-build
    /// time (see [`SecurityHeaders`]). `None` when no headers were captured.
    /// Additive (measurement-gap #14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_headers: Option<SecurityHeaders>,
    /// QUIC transport stats for the PRIMARY connection, sampled from
    /// `quinn::Connection::stats()` after the response body completed (see
    /// [`QuicStats`]). Present for http3/download3/upload3 and pageload3's
    /// cold connection; None for TCP-based protocols (which carry
    /// `socket_stats` instead) and for probes that don't own the QUIC
    /// connection (browser3). Additive (deep-measurement M1 B.1 / M3 G1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quic_stats: Option<QuicStats>,
    /// QUIC transport stats for the resumption FOLLOW-UP connection (the
    /// second connection the plain `http3` probe opens to measure TLS 1.3
    /// session resumption / 0-RTT — see [`TlsResult::quic_resumed`]).
    /// Sampled after that connection's early-data exchange completes; None
    /// when no follow-up connection ran (download3/upload3/pageload3) or it
    /// failed before completing. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quic_resumption_stats: Option<QuicStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpResult {
    pub remote_addr: String,
    pub probe_count: u32,
    pub success_count: u32,
    pub loss_percent: f64,
    pub rtt_min_ms: f64,
    pub rtt_avg_ms: f64,
    pub rtt_p95_ms: f64,
    /// Mean inter-probe delay variation (IPDV): mean |Δ| of RTTs paired
    /// consecutive-by-sequence over received probes (RFC 3393 §4.2 selection,
    /// RTT-based). NOT RFC 3550 interarrival jitter (that is a 1/16-gain EWMA
    /// over one-way transit deltas) — the serde field name is kept for
    /// contract compatibility.
    pub jitter_ms: f64,
    /// 95th percentile of the |IPDV| samples the mean above is computed from.
    /// `None` below [`MIN_SAMPLES_P95`] IPDV pairs (the project-wide
    /// small-n honesty gate) — absent at the default 10-probe train.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipdv_p95_ms: Option<f64>,
    /// 99th percentile of the |IPDV| samples; `None` below
    /// [`MIN_SAMPLES_P99`] pairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipdv_p99_ms: Option<f64>,
    pub started_at: DateTime<Utc>,
    /// Per-probe RTT values (ms), None if probe was lost.
    pub probe_rtts_ms: Vec<Option<f64>>,
    /// Echo datagrams the kernel dropped on OUR socket because its receive
    /// buffer was full (Linux ≥ 4.14 SO_MEMINFO `sk_drops`; measurement gap
    /// B.6). These arrived over the path and are still counted inside
    /// `loss_percent` — reported separately, never subtracted, so path loss
    /// and local overflow stay distinguishable. `None` = unobservable
    /// (macOS/Windows/old kernels/pre-fix results), NOT zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_drops: Option<u64>,
    /// Effective `SO_RCVBUF` of the probe socket in bytes at transfer end
    /// (context for `local_drops`; Linux reports the kernel-doubled
    /// bookkeeping value, as `ss -m` shows). Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub so_rcvbuf_bytes: Option<u32>,
    /// Loss-pattern characterization (RFC 3357) derived from `probe_rtts_ms`.
    /// `None` below [`MIN_LOSS_PATTERN_PROBES`] — too few probes to say anything
    /// about pattern. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_pattern: Option<LossPattern>,
}

/// Minimum probe train length before a loss *pattern* is characterized. Below
/// this the burst-vs-random distinction is statistically meaningless (a handful
/// of losses can't distinguish a bursty path from an unlucky independent one),
/// so [`compute_loss_pattern`] returns `None` rather than guess.
pub const MIN_LOSS_PATTERN_PROBES: usize = 20;

/// Loss-pattern characterization for a probe train, per RFC 3357 (Loss
/// Distance / Loss Period metrics). Derived purely from the seq-indexed
/// per-probe timeline (`None` = lost), so it costs no extra measurement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LossPattern {
    /// Total lost probes.
    pub lost_count: u32,
    /// Number of loss *periods* (maximal runs of consecutive losses) — RFC 3357
    /// §5. One long outage is one period; scattered single losses are many.
    pub loss_burst_count: u32,
    /// Longest run of consecutive losses (max loss-period length).
    pub loss_max_burst: u32,
    /// Mean *loss distance* (RFC 3357 §4): mean gap, in sequence numbers,
    /// between consecutive loss-period start indices. `None` with fewer than two
    /// loss periods (no distance to measure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_mean_distance: Option<f64>,
    /// Heuristic label over the RFC-3357 raw metrics above:
    /// `"no-loss"` · `"random-like"` (isolated single losses) ·
    /// `"single-burst"` (one contiguous outage) · `"bursty"` (a run of ≥3
    /// consecutive losses — the congestion/buffer-event signature). The raw
    /// counts are authoritative; this is a convenience classification.
    pub classification: String,
}

/// Characterize the loss pattern of a seq-indexed probe timeline (`None` =
/// lost). Returns `None` below [`MIN_LOSS_PATTERN_PROBES`]. Pure/deterministic.
pub fn compute_loss_pattern(rtts: &[Option<f64>]) -> Option<LossPattern> {
    let total = rtts.len();
    if total < MIN_LOSS_PATTERN_PROBES {
        return None;
    }

    // Walk the timeline collecting maximal loss runs and their start indices.
    let mut burst_lengths: Vec<usize> = Vec::new();
    let mut loss_starts: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < total {
        if rtts[i].is_none() {
            let start = i;
            let mut len = 0;
            while i < total && rtts[i].is_none() {
                len += 1;
                i += 1;
            }
            burst_lengths.push(len);
            loss_starts.push(start);
        } else {
            i += 1;
        }
    }

    let lost_count: usize = burst_lengths.iter().sum();
    let burst_count = burst_lengths.len();
    let max_burst = burst_lengths.iter().copied().max().unwrap_or(0);

    // RFC 3357 §4 loss distance: gaps between consecutive loss-period starts.
    let loss_mean_distance = if loss_starts.len() >= 2 {
        let gaps: Vec<usize> = loss_starts.windows(2).map(|w| w[1] - w[0]).collect();
        Some(gaps.iter().sum::<usize>() as f64 / gaps.len() as f64)
    } else {
        None
    };

    let classification = if lost_count == 0 {
        "no-loss"
    } else if max_burst >= 3 {
        "bursty"
    } else if burst_count == 1 && max_burst >= 2 {
        "single-burst"
    } else {
        "random-like"
    };

    Some(LossPattern {
        lost_count: lost_count as u32,
        loss_burst_count: burst_count as u32,
        loss_max_burst: max_burst as u32,
        loss_mean_distance,
        classification: classification.to_string(),
    })
}

/// UDP bulk throughput transfer result (udpdownload / udpupload modes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpThroughputResult {
    pub remote_addr: String,
    /// Requested transfer size in bytes.
    pub payload_bytes: usize,
    /// Number of datagrams sent by the sender (server for download, client for upload).
    pub datagrams_sent: u32,
    /// Number of datagrams received by the receiver. `None` when the client
    /// cannot know it: for uploads the server's CMD_REPORT carries a byte
    /// count, not a datagram count, so this value is never fabricated.
    /// (Trust audit V3.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datagrams_received: Option<u32>,
    /// Bytes acknowledged by the server (from CMD_REPORT); upload mode only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_acked: Option<usize>,
    /// Datagram loss percentage (based on unique seq_num gaps).
    ///
    /// **Includes local drops.** For downloads, datagrams the kernel dropped
    /// because OUR socket's receive buffer overflowed count as missing seqs
    /// here — indistinguishable from path loss without `local_drops`. When
    /// `local_drops` is `None` (non-Linux, kernels < 4.14, pre-fix results)
    /// the split is unobservable and this figure may over-attribute loss to
    /// the path (measurement gap B.6). The split is surfaced, never silently
    /// subtracted.
    pub loss_percent: f64,
    /// Total transfer window in ms. Download: first data packet to CMD_DONE
    /// received. Upload: send start to CMD_DONE sent — the CMD_REPORT
    /// round-trip is excluded (trust audit V4).
    pub transfer_ms: f64,
    /// Measured throughput in MB/s; None if transfer_ms = 0.
    pub throughput_mbps: Option<f64>,
    pub started_at: DateTime<Utc>,
    /// Datagrams the kernel dropped on OUR socket (receive-buffer overflow;
    /// Linux ≥ 4.14 SO_MEMINFO `sk_drops`). Meaningful for downloads (client
    /// receives the stream); ~0 for uploads (drops happen server-side).
    /// Counted inside `loss_percent` — reported separately for attribution.
    /// `None` = unobservable, NOT zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_drops: Option<u64>,
    /// Effective `SO_RCVBUF` of the probe socket in bytes at transfer end —
    /// the buffer whose overflow `local_drops` counts (Linux reports the
    /// kernel-doubled bookkeeping value). Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub so_rcvbuf_bytes: Option<u32>,
}

/// Latency-under-load / bufferbloat result (`rpm` mode).
///
/// Two phases against a networker-endpoint target:
/// 1. **Unloaded** — a burst of UDP echo probes (port 9999) on an idle link.
/// 2. **Loaded** — sustained HTTP downloads load the link while UDP echo
///    probes fire at a steady cadence.
///
/// Headline numbers: `rpm` = 60000 / loaded avg RTT (round-trips per minute,
/// higher is better) and `bufferbloat_factor` = loaded avg / unloaded avg
/// (1.0 ≈ no bufferbloat).
///
/// **Methodology honesty.** This probe is NOT conformant with
/// draft-ietf-ippm-responsiveness (the Apple/Cloudflare/Ookla "RPM"
/// methodology): the load is a single sequential HTTP download flow (no
/// multi-connection ramp, no saturation detection, no upload direction), and
/// latency is probed with UDP echoes on a separate socket (no HTTP
/// foreign/self probes — flow-isolating AQMs like fq_codel can prioritize the
/// sparse echo stream past the loaded flow's queue). The `rpm` figure here is
/// therefore typically much HIGHER than a spec RPM for the same link and must
/// not be compared across tools; treat it as a UDP-echo-under-load
/// diagnostic. A spec-conformant responsiveness mode is tracked separately
/// (Wave R).
///
/// **Loaded-tail accounting.** Loaded-phase echoes are waited for up to the
/// full per-probe `--udp-timeout` (Tmax, default 5 s) past the load window,
/// so slow echoes from deep queues count as high RTTs instead of silently
/// becoming loss. `loaded_loss_percent` counts only probes unanswered after
/// their full Tmax; `loaded_probes_censored` reports probes whose wait was
/// truncated below Tmax (should be 0 in normal operation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpmResult {
    pub remote_addr: String,
    // ── Phase 1: unloaded UDP echo RTT ─────────────────────────────────────
    pub unloaded_probe_count: u32,
    pub unloaded_success_count: u32,
    pub unloaded_loss_percent: f64,
    pub unloaded_rtt_min_ms: f64,
    pub unloaded_rtt_avg_ms: f64,
    pub unloaded_rtt_p95_ms: f64,
    pub unloaded_jitter_ms: f64,
    // ── Phase 2: UDP echo RTT while the link is loaded ─────────────────────
    pub loaded_probe_count: u32,
    pub loaded_success_count: u32,
    /// Probes unanswered after their full per-probe Tmax (`--udp-timeout`).
    pub loaded_loss_percent: f64,
    pub loaded_rtt_min_ms: f64,
    pub loaded_rtt_avg_ms: f64,
    pub loaded_rtt_p95_ms: f64,
    pub loaded_jitter_ms: f64,
    /// Loaded-phase probes whose echo wait was truncated before their full
    /// Tmax elapsed (e.g. a socket error ended the drain early) — these are
    /// counted in `loaded_loss_percent` but may actually be very-late echoes,
    /// so a nonzero value means the loaded RTT average is optimistically
    /// biased. Additive; absent in pre-v0.28.82 results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_probes_censored: Option<u32>,
    /// Loaded-phase echo datagrams the kernel dropped on OUR socket
    /// (receive-buffer overflow; Linux ≥ 4.14 SO_MEMINFO `sk_drops`). The
    /// loaded phase is exactly when the probe process is busiest, so a
    /// scheduling hiccup can overflow the echo socket and masquerade as
    /// under-load loss — these drops are counted inside
    /// `loaded_loss_percent` and reported separately here (B.6). `None` =
    /// unobservable, NOT zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_local_drops: Option<u64>,
    // ── Derived headline metrics ───────────────────────────────────────────
    /// Round-trips per minute under load: 60000 / loaded_rtt_avg_ms.
    /// `None` when every loaded probe was lost (no avg to derive from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<f64>,
    /// loaded_rtt_avg_ms / unloaded_rtt_avg_ms. `None` when either phase has
    /// no successful probes (a 0.0 sentinel avg is not a measurement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bufferbloat_factor: Option<f64>,
    // ── Load generator (sustained HTTP /download) ──────────────────────────
    /// Wall-clock duration of the loaded phase (ms).
    pub load_duration_ms: f64,
    /// Bytes delivered by downloads that completed inside the load window.
    /// A download still in flight when the window closed is not counted.
    pub load_bytes_transferred: u64,
    /// Number of downloads that completed inside the load window.
    pub load_downloads_completed: u32,
    /// Mean throughput across completed downloads (MB/s); `None` when no
    /// download completed inside the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_throughput_mbps: Option<f64>,
    pub started_at: DateTime<Utc>,
}

/// One direction (download or upload) of the `responsiveness` probe.
///
/// Parameters follow draft-ietf-ippm-responsiveness-08 §4: interval duration
/// ID = 1 s, initial connections INP = 1, added per interval INC = 1, cap
/// MNP = 16, moving-average distance MAD = 4, standard-deviation tolerance
/// SDT = 5 %, trimmed-mean percentage TMP = 95 %. Saturation is declared when
/// the standard deviation of the last MAD moving-average goodput values is
/// below SDT of the current moving average; responsiveness stability is then
/// checked the same way over per-interval responsiveness values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsivenessDirection {
    /// Goodput stability (saturation) was declared before the direction's
    /// time cap. `false` means the link was still ramping when the cap hit —
    /// the RPM is still reported but was NOT measured at verified saturation.
    pub saturation_reached: bool,
    /// Per-interval responsiveness values also stabilized (the draft's second
    /// stability criterion) before the cap.
    pub responsiveness_stable: bool,
    /// Number of parallel load connections open when the direction ended.
    pub saturated_connections: u32,
    /// Number of measurement intervals (ID each) the direction ran.
    pub intervals: u32,
    /// Wall-clock duration of the direction (ms).
    pub load_duration_ms: f64,
    /// Bytes moved by the load connections over the whole direction.
    pub bytes_transferred: u64,
    /// Final moving-average goodput (MB/s, decimal 1e6) — the capacity at
    /// (or nearest to) saturation. `None` when no bytes moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_mbps: Option<f64>,
    /// The draft's Responsiveness value (round-trips per minute):
    /// `(60000/((TM(tcp_f)+TM(tls_f)+TM(http_f))/3) + 60000/TM(http_l)) / 2`
    /// with TM = single-sided 95 % trimmed mean over the final MAD intervals
    /// (TLS term omitted for cleartext targets, per the draft's TCP-only
    /// variant). `None` when either probe family produced no samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<f64>,
    /// Foreign-probe component: `60000 / mean(TM(tcp_f), TM(tls_f), TM(http_f))`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_rpm: Option<f64>,
    /// Self-probe component: `60000 / TM(http_l)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_rpm: Option<f64>,
    /// Trimmed-mean TCP connect time of foreign probes (ms, final MAD intervals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_tcp_tm_ms: Option<f64>,
    /// Trimmed-mean TLS handshake time of foreign probes (ms); `None` for
    /// cleartext targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_tls_tm_ms: Option<f64>,
    /// Trimmed-mean HTTP GET (1-byte object) time of foreign probes (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_http_tm_ms: Option<f64>,
    /// Trimmed-mean HTTP GET time of self probes — multiplexed on a
    /// load-generating connection (ms). This is the number flow-isolating
    /// AQMs cannot hide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_http_tm_ms: Option<f64>,
    pub foreign_probes_sent: u32,
    pub foreign_probes_ok: u32,
    pub self_probes_sent: u32,
    pub self_probes_ok: u32,
}

/// Working-conditions responsiveness result (`responsiveness` mode) —
/// draft-ietf-ippm-responsiveness-08 methodology. Both directions are
/// measured sequentially (download first). Unlike [`RpmResult`], the RPM
/// figures here follow the draft's probe types and aggregation formula and
/// ARE comparable with other conformant implementations (Apple
/// `networkQuality`, Cloudflare, Ookla RPM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsivenessResult {
    /// Endpoint base URL the load and probes ran against.
    pub remote_addr: String,
    /// Headline: download-direction RPM (higher is better).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_download: Option<f64>,
    /// Upload-direction RPM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_upload: Option<f64>,
    /// Download capacity at saturation (MB/s, decimal 1e6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_down_mbps: Option<f64>,
    /// Upload capacity at saturation (MB/s, decimal 1e6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_up_mbps: Option<f64>,
    /// Download-direction detail.
    pub download: ResponsivenessDirection,
    /// Upload-direction detail. `None` when the upload stage could not run
    /// (see `upload_error`) — never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<ResponsivenessDirection>,
    /// Why the upload stage is absent, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_error: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// One connection of a multi-connection throughput direction — the compact
/// per-flow diagnostic the aggregate capacity figure is explained by.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MthroughputConn {
    /// Connection index (0-based, spawn order).
    pub conn: u32,
    /// This connection's goodput over the steady measure window (MB/s,
    /// decimal 1e6). A wide spread across connections indicates per-flow
    /// shaping/policing rather than a shared bottleneck.
    pub mbps: f64,
    /// Post-transfer kernel TCP attribution verdict, from the Linux ≥ 4.10
    /// busy/rwnd/sndbuf chronograph triad sampled on a `dup(2)` of this
    /// connection's socket: `"rwnd-limited NN%"` (peer receive window),
    /// `"sndbuf-limited NN%"` (local send buffer), `"path-limited"` (neither
    /// ≥ 5% — congestion/path was the constraint), or `"unobserved"` (no
    /// triad: non-Linux platform or old kernel). The triad is SEND-side: it
    /// attests the data direction for the upload stage; on the download
    /// stage the sender is the endpoint, whose kernel is not readable from
    /// here, so download verdicts describe only the request/ACK flow.
    pub verdict: String,
    /// Lifetime retransmitted segments on this connection (Linux
    /// tcpi_total_retrans; macOS tcpi_txretransmitpackets). `None` when the
    /// kernel exposes no counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrans: Option<u32>,
}

/// One direction (download or upload) of the `mthroughput` probe.
///
/// Ramp: start 1 HTTP/2 connection, add 1 per 1 s interval (cap 8) until the
/// aggregate goodput's moving average stabilizes (stddev of the last 4
/// moving averages < 5% of the current average — the same criterion the
/// `responsiveness` mode uses), then hold the connection count fixed for a
/// 4-interval steady measure window that produces `capacity_mbps` and the
/// per-connection figures. Stages are time-boxed (15 s cap per direction),
/// not payload-sized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MthroughputDirection {
    /// Aggregate goodput stabilized before the time cap. `false` means the
    /// cap hit mid-ramp — `capacity_mbps` is then the goodput over the final
    /// intervals, NOT a verified saturation figure.
    pub saturation_reached: bool,
    /// Parallel connections open during the measure window.
    pub connections: u32,
    /// Total measurement intervals the direction ran (ramp + measure).
    pub intervals: u32,
    /// Wall-clock duration of the ramp phase, start → saturation declared
    /// (or the whole direction when saturation never was) (ms).
    pub ramp_duration_ms: f64,
    /// Wall-clock duration of the steady measure window (ms).
    pub measure_duration_ms: f64,
    /// Wall-clock duration of the whole direction (ms).
    pub load_duration_ms: f64,
    /// Bytes moved by all connections over the whole direction.
    pub bytes_transferred: u64,
    /// Aggregate capacity over the steady measure window (MB/s, decimal 1e6).
    /// `None` when no bytes moved in the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_mbps: Option<f64>,
    /// Slowest connection's goodput over the measure window (MB/s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_conn_min_mbps: Option<f64>,
    /// Fastest connection's goodput over the measure window (MB/s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_conn_max_mbps: Option<f64>,
    /// Mean per-connection goodput over the measure window (MB/s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_conn_mean_mbps: Option<f64>,
    /// Fair-share spread `(max − min) / mean × 100` (%). Near 0 = the
    /// bottleneck is shared fairly across flows; large = per-flow
    /// shaping/policing or asymmetric path treatment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fair_share_spread_pct: Option<f64>,
    /// Connections whose verdict was receiver-window-limited (≥ 5% of busy
    /// time in tcpi_rwnd_limited).
    pub rwnd_limited_conns: u32,
    /// Connections whose verdict was local-send-buffer-limited.
    pub sndbuf_limited_conns: u32,
    /// Connections limited by neither (path/congestion was the constraint).
    pub path_limited_conns: u32,
    /// Connections with no triad data (non-Linux platform / old kernel) —
    /// honest count, never folded into the other buckets.
    pub unobserved_conns: u32,
    /// Per-connection detail, one entry per connection (`connections` long).
    pub per_conn: Vec<MthroughputConn>,
}

/// Multi-connection capacity result (`mthroughput` mode). Both directions
/// are measured sequentially (download first). Complements the
/// single-connection `download`/`upload` modes: those report per-flow fair
/// share (ndt7-style), this reports link capacity (Ookla-style) — on
/// high-BDP or lossy paths the two legitimately diverge, and the
/// per-connection TCP attribution here explains why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MthroughputResult {
    /// Endpoint base URL the load ran against.
    pub remote_addr: String,
    /// Headline: download capacity at saturation (MB/s, decimal 1e6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_down_mbps: Option<f64>,
    /// Upload capacity at saturation (MB/s, decimal 1e6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_up_mbps: Option<f64>,
    /// Download connections open at saturation.
    pub conns_down: u32,
    /// Upload connections open at saturation; `None` when the upload stage
    /// could not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conns_up: Option<u32>,
    /// Download fair-share spread (%, see [`MthroughputDirection`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fair_share_spread_down_pct: Option<f64>,
    /// Upload fair-share spread (%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fair_share_spread_up_pct: Option<f64>,
    /// Download-direction detail.
    pub download: MthroughputDirection,
    /// Upload-direction detail. `None` when the upload stage could not run
    /// (see `upload_error`) — never fabricated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<MthroughputDirection>,
    /// Why the upload stage is absent, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_error: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// STAMP (RFC 8762) probe result (`stamp` mode).
///
/// The Session-Reflector stamps each reflected packet with its receive (T2)
/// and transmit (T3) timestamps plus its own sequence number, so the sender
/// can compute — with NO clock synchronization:
/// - RTT corrected for reflector processing time: `(T4−T1) − (T3−T2)`;
/// - per-direction delay VARIATION (clock offsets cancel within a direction);
/// - DIRECTIONAL loss: the highest reflector sequence observed says how many
///   probes reached the reflector, splitting sender→reflector loss from
///   reflector→sender loss (RFC 8762 §4.2).
///
/// Timestamps use the NTP 64-bit format (RFC 8762 default; PTP not used).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StampResult {
    /// Reflector address the probes were sent to.
    pub remote_addr: String,
    pub probes_sent: u32,
    pub replies_received: u32,
    /// Overall round-trip loss percent (either direction).
    pub loss_percent: f64,
    /// Sender→reflector loss percent, derived from the reflector's sequence
    /// numbers. `None` when no reply arrived (nothing to derive from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_sent_percent: Option<f64>,
    /// Reflector→sender loss percent. `None` when no reply arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_return_percent: Option<f64>,
    // ── Processing-corrected RTT ((T4−T1) − (T3−T2)) ───────────────────────
    pub rtt_min_ms: f64,
    pub rtt_avg_ms: f64,
    pub rtt_p95_ms: f64,
    /// Mean |IPDV| (RFC 3393, consecutive-by-sequence) of the corrected RTT.
    pub jitter_ms: f64,
    // ── Per-direction delay variation (no clock sync needed) ───────────────
    /// Mean |IPDV| of the forward (sender→reflector) one-way delay (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_ipdv_mean_ms: Option<f64>,
    /// p95 |IPDV| of the forward direction; `None` below the p95 sample gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_ipdv_p95_ms: Option<f64>,
    /// Mean |IPDV| of the return (reflector→sender) one-way delay (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_ipdv_mean_ms: Option<f64>,
    /// p95 |IPDV| of the return direction; `None` below the p95 sample gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_ipdv_p95_ms: Option<f64>,
    /// Mean reflector processing time T3−T2 (µs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflector_processing_avg_us: Option<f64>,
    /// Highest reflector sequence number observed (0-based). The reflector
    /// increments per received packet, so `max+1` probes reached it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflector_seq_max: Option<u32>,
    // ── Raw one-way readings + optional clock-synced estimate ──────────────
    /// Mean raw forward reading T2−T1 (ms) — INCLUDES the unknown clock
    /// offset between sender and reflector; may be negative. Useful only for
    /// variation and for the offset-corrected estimate below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_owd_raw_avg_ms: Option<f64>,
    /// Mean raw return reading T4−T3 (ms) — includes the clock offset with
    /// the opposite sign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_owd_raw_avg_ms: Option<f64>,
    /// INDICATIVE absolute forward one-way delay (ms): raw reading corrected
    /// by the run's SNTP clock offset. Only filled when the run performed a
    /// clock-sync query; uncertainty is at least `owd_uncertainty_ms` and
    /// assumes the reflector's own clock is NTP-true. An ESTIMATE, not a
    /// measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owd_forward_est_ms: Option<f64>,
    /// Indicative absolute return one-way delay (ms) — same caveats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owd_return_est_ms: Option<f64>,
    /// ± uncertainty of the OWD estimates (ms): half the SNTP round-trip
    /// delay (the offset's intrinsic error bound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owd_uncertainty_ms: Option<f64>,
    /// Per-probe processing-corrected RTTs (ms); `None` = lost.
    pub probe_rtts_ms: Vec<Option<f64>>,
    /// Send cadence (ms) — probes are periodic (RFC 3432), not send-on-echo.
    pub interval_ms: u64,
    pub started_at: DateTime<Utc>,
}

/// ICMP echo result (`ping` mode).
///
/// Same aggregate shape as [`UdpResult`] (min/avg/p95, mean |IPDV| as
/// `jitter_ms` via [`aggregate_udp_rtts`], loss percent, per-probe RTTs) but
/// measured at the network layer with ICMP echo — no TCP/UDP service on the
/// target required.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    /// IP actually pinged (first resolved address of the target host).
    pub remote_addr: String,
    pub probe_count: u32,
    pub success_count: u32,
    pub loss_percent: f64,
    pub rtt_min_ms: f64,
    pub rtt_avg_ms: f64,
    pub rtt_p95_ms: f64,
    pub jitter_ms: f64,
    /// Per-probe RTT values (ms), None if the echo was lost.
    pub probe_rtts_ms: Vec<Option<f64>>,
    /// IP TTL / hop limit observed on echo replies, when the platform exposes
    /// it to unprivileged sockets. `None` is "not observable", never a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_ttl: Option<u32>,
    pub started_at: DateTime<Utc>,
}

/// One hop discovered by the `path` probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathHop {
    /// TTL value that surfaced this hop (1-based).
    pub index: u32,
    /// Router address that answered, `None` when the hop did not respond
    /// within the per-hop timeout (a traceroute `*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    /// Probe-send → ICMP-error-arrival RTT (ms); `None` for silent hops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
}

/// Hop-discovery result (`path` mode).
///
/// `method` records HOW the hops were (or were not) obtained, because the
/// unprivileged capability differs per platform:
/// - `"udp-ttl/ip-recverr"` (Linux): full per-hop addresses + RTTs from the
///   UDP socket's error queue — no raw sockets, no privileges.
/// - `"udp-ttl-estimate"` (macOS/Windows): ICMP time-exceeded errors are not
///   readable unprivileged, so `hops` is empty and only the destination-
///   reached TTL scan result is reported. Hops are NEVER fabricated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathResult {
    /// Destination address the probes were aimed at.
    pub remote_addr: String,
    /// Discovered hops in TTL order. Empty when the platform cannot observe
    /// hop addresses unprivileged (see `method`).
    pub hops: Vec<PathHop>,
    /// Number of hops to the destination. From the responding hop list when
    /// the destination was reached, or the TTL-scan estimate in degraded
    /// mode; `None` when the destination never answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop_count: Option<u32>,
    /// True when a destination-generated response (ICMP port-unreachable /
    /// connection-refused) was observed.
    pub destination_reached: bool,
    /// RTT to the destination itself (ms), when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_rtt_ms: Option<f64>,
    /// How the path was measured — see the struct docs.
    pub method: String,
    /// Highest TTL probed.
    pub max_ttl: u32,
    pub started_at: DateTime<Utc>,
}

/// One address-family leg of the `dualstack` probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualStackLeg {
    /// False when the family had no DNS records (or the target is a literal
    /// of the other family) — nothing was probed, nothing failed.
    pub attempted: bool,
    /// True when the HTTP GET over this family completed.
    pub success: bool,
    /// Address the leg connected to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<f64>,
    /// Why the leg failed / was not attempted, for the report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// IPv4-vs-IPv6 comparison result (`dualstack` mode).
///
/// The happy-eyeballs verdict follows RFC 8305's connection race: IPv6 is
/// started first and IPv4 only after a grace period (250 ms), so IPv6 "wins"
/// unless its TCP connect is more than the grace slower than IPv4's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualStackResult {
    pub ipv4: DualStackLeg,
    pub ipv6: DualStackLeg,
    /// `"ipv4"` / `"ipv6"` — family with the lower total_ms among successful
    /// legs; `None` unless both legs succeeded (nothing to compare).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faster_family: Option<String>,
    /// slower total_ms − faster total_ms (≥ 0); only when both legs succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_ms: Option<f64>,
    /// Which family a happy-eyeballs (RFC 8305) client would use, with the
    /// reason — e.g. `"ipv6 (connect within 250ms grace of ipv4)"`.
    pub happy_eyeballs_verdict: String,
    /// Grace period used for the verdict (ms) — RFC 8305's recommended 250.
    pub happy_eyeballs_grace_ms: f64,
    pub started_at: DateTime<Utc>,
}

/// WebSocket probe result (`websocket` mode).
///
/// The connection phases (DNS/TCP/TLS) live in the attempt's `dns`/`tcp`/`tls`
/// sub-results exactly like the `tls` probe; this struct carries what happens
/// after the socket is up: the HTTP 101 upgrade round-trip and the echo
/// message RTT distribution. Message RTTs share [`UdpResult`]'s aggregate
/// semantics (min/avg/p95 via [`aggregate_udp_rtts`], mean |IPDV| as
/// `jitter_ms`, per-message RTTs with `None` for lost echoes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketResult {
    /// ws:// or wss:// URL the probe connected to.
    pub url: String,
    /// HTTP 101 upgrade round-trip: client handshake request sent → server
    /// 101 response processed (ms). Excludes DNS/TCP/TLS (reported separately).
    pub upgrade_ms: f64,
    /// Status code of the upgrade response (101 on success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade_status: Option<u16>,
    /// Echo messages sent after the upgrade.
    pub message_count: u32,
    /// Echoes received (matched by embedded sequence id).
    pub echo_count: u32,
    /// 100 × (message_count − echo_count) / message_count.
    pub loss_percent: f64,
    pub msg_rtt_min_ms: f64,
    pub msg_rtt_avg_ms: f64,
    pub msg_rtt_p95_ms: f64,
    /// Mean inter-probe delay variation (IPDV): mean |Δ| of RTTs paired
    /// consecutive-by-sequence over received echoes (RFC 3393-style; NOT
    /// RFC 3550 interarrival jitter).
    pub jitter_ms: f64,
    /// Per-message RTT values (ms), `None` if the echo never arrived.
    pub msg_rtts_ms: Vec<Option<f64>>,
    /// Bytes per echo message payload.
    pub payload_size: usize,
    pub started_at: DateTime<Utc>,
}

/// Path-MTU discovery result (`pmtud` mode).
///
/// `method` records HOW the verdict was reached, because unprivileged
/// capability differs per platform (same honesty contract as [`PathResult`]):
/// - `"df-udp-echo/ip-recverr"` (Linux, echo replies observed): DF-flagged
///   probes binary-searched with positive delivery confirmation from the
///   endpoint's UDP echo; ICMP frag-needed (with next-hop MTU) read from the
///   error queue tightens the bound.
/// - `"df-icmp/ip-recverr"` (Linux, no echo service): verdict from ICMP
///   fragmentation-needed errors alone — the classic PMTUD signal; works
///   against targets that never answer.
/// - `"df-dontfrag/udp-echo"` (macOS, echo replies observed): `IP_DONTFRAG`
///   probes confirmed by echo; ICMP surfaces only as EMSGSIZE on the
///   connected socket (no next-hop MTU value available).
/// - `"df-dontfrag/emsgsize"` (macOS, no echo): EMSGSIZE-only evidence.
/// - `"df-no-feedback"`: no echo, no ICMP, no send errors — `path_mtu` is
///   `None` because nothing was measured (ICMP black hole or silent path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PmtudResult {
    /// Destination address the DF probes were aimed at.
    pub remote_addr: String,
    /// Discovered path MTU in bytes at the IP layer (largest unfragmented
    /// payload + IP/UDP headers). `None` when no feedback allowed a verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_mtu: Option<u32>,
    /// Largest UDP payload that traversed unfragmented (path_mtu − headers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_unfragmented_payload: Option<u32>,
    /// DF-flagged datagrams sent during the search.
    pub probes_sent: u32,
    /// How the MTU was (or was not) determined — see the struct docs.
    pub method: String,
    /// Next-hop MTU reported by an ICMP fragmentation-needed message, when
    /// one was observed (Linux error queue only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icmp_mtu: Option<u32>,
    /// MTU of the default-route interface, for contrast with the path value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_mtu: Option<u32>,
    /// IP + UDP header bytes assumed when converting payload ⇄ MTU
    /// (28 for IPv4, 48 for IPv6).
    pub header_bytes: u32,
    /// True when the search confirmed its ceiling size without ever finding a
    /// "too big" bound — the true path MTU may be larger than `path_mtu`.
    pub lower_bound_only: bool,
    pub started_at: DateTime<Utc>,
}

/// Page-load simulation result (pageload / pageload2 / pageload3 modes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageLoadResult {
    /// Number of assets listed in the manifest.
    pub asset_count: usize,
    /// Number of assets successfully retrieved — a 2xx response with the body
    /// fully drained. Asset requests never follow redirects, so a 3xx asset
    /// response yields no asset body and counts as *failed*, as do 4xx/5xx and
    /// transport errors. (Before v0.28.82 any status < 500 — including 404 —
    /// counted as "fetched"; the number may drop on re-measurement of targets
    /// that error on assets because the old number was wrong.)
    pub assets_fetched: usize,
    /// Sum of all fetched (2xx) asset response body bytes.
    pub total_bytes: usize,
    /// Wall-clock time from probe start to last asset byte received (ms).
    pub total_ms: f64,
    /// TTFB of the `/page` manifest request (ms): request write → first
    /// response byte, measured on an already-established connection
    /// (probe-style TTFB — excludes DNS/TCP/TLS setup, which are reported in
    /// their own phases). NOT the browser TTFB definition; see
    /// [`BrowserResult::ttfb_ms`].
    pub ttfb_ms: f64,
    /// Number of distinct TCP connections opened (6 for H1.1, 1 for H2).
    pub connections_opened: u32,
    /// Per-asset total durations in ms, index-aligned with asset ids
    /// (`asset_timings_ms[i]` is asset `i`; length == `asset_count`). Assets
    /// that were not fetched successfully carry the `0.0` sentinel — use
    /// `assets_failed`/`assets_fetched` to tell "failed" apart from "instant".
    /// Caveat: pageload2/pageload3 results recorded before v0.28.82 pushed
    /// only *successful* fetches, so on any failure the old vector was shorter
    /// than `asset_count` and not index-aligned.
    pub asset_timings_ms: Vec<f64>,
    pub started_at: DateTime<Utc>,
    /// Sum of all TLS handshake durations during this page load (ms).
    /// H1.1: sum across all connections_opened (one per connection).
    /// H2/H3: single handshake duration. Zero when target is plain http://.
    #[serde(default)]
    pub tls_setup_ms: f64,
    /// Fraction of total_ms spent in TLS handshakes (0.0–1.0).
    /// Zero when target is plain http://.
    #[serde(default)]
    pub tls_overhead_ratio: f64,
    /// Individual TLS handshake duration for each connection opened (ms).
    /// Length == connections_opened. Plain HTTP = all zeros.
    #[serde(default)]
    pub per_connection_tls_ms: Vec<f64>,
    /// Total process CPU time (user + system) consumed during this probe (ms).
    /// Highest for HTTP/3 due to QUIC userspace encryption. None if unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<f64>,
    /// True when a pre-established connection was reused (--connection-reuse).
    /// Warm probes skip DNS/TCP/TLS setup; compare with cold probes to see savings.
    #[serde(default)]
    pub connection_reused: bool,
    /// Post-transfer kernel TCP stats sampled per opened connection (dup-fd
    /// `TCP_INFO`, same mechanism as `HttpResult::socket_stats`). Length ==
    /// `connections_opened` when captured; empty on non-Unix platforms, for
    /// QUIC (pageload3 — UDP, no TCP socket) and for warm probes that reuse a
    /// pre-established connection. Additive; `schema_version` stays 1.0.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_connection_socket_stats: Vec<SocketStats>,
    /// Number of assets that did NOT come back as a fully drained 2xx
    /// response (non-2xx status — including unfollowed 3xx — or a transport
    /// error/timeout). Always `Some(asset_count - assets_fetched)` on new
    /// runs; `None` on pre-v0.28.82 data. Additive; `schema_version` stays
    /// 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets_failed: Option<u32>,
}

/// Real-browser page-load result (browser mode, requires `--features browser`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserResult {
    /// Total page load time: navigation start → load event (ms).
    pub load_ms: f64,
    /// DOMContentLoaded event timing (ms from navigation start).
    pub dom_content_loaded_ms: f64,
    /// Browser TTFB (ms): `responseStart − navigationStart` from
    /// `window.performance.timing`. This is the *browser* definition — it
    /// includes DNS, TCP connect, TLS, and any redirects before the main
    /// document's first byte. NOT comparable with [`HttpResult::ttfb_ms`] or
    /// [`PageLoadResult::ttfb_ms`], which measure request-send → first byte
    /// on an already-established connection.
    pub ttfb_ms: f64,
    /// Total resources loaded (main doc + all sub-resources).
    pub resource_count: u32,
    /// Sum of *declared* `Content-Length` response headers across all
    /// resources — NOT wire bytes. It excludes response headers, counts 0 for
    /// responses without a Content-Length (e.g. chunked), and reflects the
    /// declared (possibly compressed) body size. Field name kept for wire
    /// compat; true wire-byte accounting (CDP
    /// `Network.loadingFinished.encodedDataLength`) is planned for a later
    /// wave.
    pub transferred_bytes: usize,
    /// HTTP protocol negotiated for the main document ("h2", "h3", "http/1.1" …).
    pub protocol: String,
    /// Per-protocol resource counts sorted by count desc: [("h2", 18), ("h3", 2)].
    pub resource_protocols: Vec<(String, u32)>,
    pub started_at: DateTime<Utc>,
    /// Largest Contentful Paint (ms from navigation start), from a buffered
    /// `PerformanceObserver` injected before navigation. The reported value is
    /// the *last* LCP candidate entry observed by collection time. `None` when
    /// the page produced no LCP entry or the observer failed — never
    /// 0-as-missing. Additive; `schema_version` stays 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcp_ms: Option<f64>,
    /// Cumulative Layout Shift (unitless), computed with the web.dev
    /// session-window rule: layout-shift entries (excluding
    /// `hadRecentInput`) are grouped into sessions — a new session starts
    /// when the gap since the previous entry exceeds 1 s or the session span
    /// exceeds 5 s — and CLS is the *maximum* session value, not the naive
    /// sum. `Some(0.0)` is a real measurement (observer registered, zero
    /// shifts); `None` means the observer failed / entry type unsupported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cls: Option<f64>,
    /// First Contentful Paint (ms from navigation start), from the buffered
    /// `paint` observer. `None` when no FCP entry was produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fcp_ms: Option<f64>,
    /// Total Blocking Time (ms): Σ max(0, duration − 50 ms) over `longtask`
    /// entries whose start falls at/after FCP (when FCP is known; otherwise
    /// over the whole window), within the lab collection window = navigation
    /// start → load event + settle delay (a TTI proxy — headless lab pages
    /// have no user input, so the classic FCP→TTI window is approximated by
    /// the load window). `Some(0.0)` = observer worked, no blocking tasks;
    /// `None` = longtask observation unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tbt_ms: Option<f64>,
    /// Real wire bytes across all requests: per request,
    /// max(`Network.loadingFinished.encodedDataLength`, Σ `dataReceived`
    /// chunk `encodedDataLength`) — headers + bodies as transferred
    /// (compressed size when the server compresses). The honest replacement
    /// for the declared-length sum in `transferred_bytes` (kept as-is for
    /// wire compat). Caveat (measured 2026-07): Chrome's CDP byte
    /// attribution under-reports on very fast (loopback) transfers — some
    /// requests get body-only or partial counts, so on loopback this is a
    /// *lower bound*; attribution is reliable on real-RTT paths. (The
    /// JS-side `ResourceTiming.transferSize` is NOT an alternative: the
    /// spec pegs it to `encodedBodySize + 300` as a fingerprinting
    /// mitigation.) `None` on pre-Wave-W data or when the event streams
    /// carried no byte accounting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_bytes_total: Option<u64>,
    /// Per-request CDP waterfall (capped at [`BROWSER_WATERFALL_CAP`]
    /// entries; see `waterfall_truncated`). Empty on pre-Wave-W data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waterfall: Vec<BrowserRequest>,
    /// True when the page issued more requests than the waterfall cap and
    /// the vector was truncated (aggregates such as `wire_bytes_total` still
    /// cover ALL requests).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waterfall_truncated: bool,
}

/// Maximum number of per-request entries kept in [`BrowserResult::waterfall`].
/// Third-party pages can fire thousands of requests; aggregates still count
/// everything, only the per-request detail vector is capped.
pub const BROWSER_WATERFALL_CAP: usize = 200;

/// One network request captured from the CDP event stream during a browser
/// probe (Network.requestWillBeSent / responseReceived / dataReceived /
/// loadingFinished, correlated by requestId). Additive; `schema_version`
/// stays 1.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserRequest {
    /// Request URL, truncated to 160 chars (with `…`) for report sanity.
    pub url: String,
    /// HTTP method ("GET", "POST", …).
    pub method: String,
    /// HTTP status code. `None` when no response was received (failed /
    /// still in flight at collection time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Coarse MIME type from the response ("text/html", "image/png", …) —
    /// parameters after `;` stripped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Protocol this resource was fetched over ("h2", "h3", "http/1.1", …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Wire bytes for this request:
    /// max(`Network.loadingFinished.encodedDataLength`, Σ `dataReceived`
    /// chunk `encodedDataLength`) — includes headers and reflects
    /// on-the-wire (possibly compressed) size. Disk-cache hits report
    /// `Some(0)` honestly (nothing crossed the wire). `None` when no event
    /// carried byte accounting for this request. See
    /// [`BrowserResult::wire_bytes_total`] for the loopback
    /// under-attribution caveat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_bytes: Option<u64>,
    /// Request start (ms relative to the main document request's
    /// `requestWillBeSent` timestamp — the navigation origin of this capture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<f64>,
    /// Loading finished (same time base as `start_ms`). `None` when the
    /// request did not finish before collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<f64>,
    /// Served from disk cache (no network fetch for the body).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_disk_cache: bool,
    /// Served by a service worker.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_service_worker: bool,
    /// Per-phase ResourceTiming breakdown (absent for cache hits / data URLs
    /// and whenever Chrome reports no timing for the fetch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<BrowserRequestTiming>,
}

/// DevTools-waterfall phase breakdown for one request, mapped from CDP
/// `ResourceTiming` (all ms; a phase is `None` when Chrome reports −1, i.e.
/// the phase did not occur — e.g. no DNS on a reused connection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserRequestTiming {
    /// DNS resolve (dnsStart → dnsEnd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<f64>,
    /// TCP/QUIC connect (connectStart → connectEnd; includes SSL time on
    /// Chrome's clock — see `ssl_ms` for the TLS share).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<f64>,
    /// TLS handshake (sslStart → sslEnd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_ms: Option<f64>,
    /// Request send (sendStart → sendEnd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_ms: Option<f64>,
    /// Server wait / TTFB (sendEnd → receiveHeadersEnd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<f64>,
    /// Content download (receiveHeadersEnd → loadingFinished).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_ms: Option<f64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// URL page-load diagnostic contracts (PR-01 foundation only)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UrlDiagnosticStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Partial,
}

impl std::fmt::Display for UrlDiagnosticStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Partial => "partial",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UrlPageLoadStrategy {
    Browser,
    BrowserProbe,
    FetchProbe,
    Hybrid,
}

impl UrlPageLoadStrategy {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::BrowserProbe => "browser_probe",
            Self::FetchProbe => "fetch_probe",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UrlProbeAttemptType {
    Browser,
    Fetch,
    Probe,
}

impl UrlProbeAttemptType {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Fetch => "fetch",
            Self::Probe => "probe",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlPacketCaptureSummary {
    pub mode: String,
    pub interface: String,
    pub capture_path: String,
    pub total_packets: u64,
    pub capture_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub tcp_packets: u64,
    pub udp_packets: u64,
    pub quic_packets: u64,
    pub http_packets: u64,
    pub dns_packets: u64,
    pub retransmissions: u64,
    pub duplicate_acks: u64,
    pub resets: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transport_shares: Vec<PacketShare>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_endpoints: Vec<EndpointPacketCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_ports: Vec<PortPacketCount>,
    pub observed_quic: bool,
    pub observed_tcp_only: bool,
    pub observed_mixed_transport: bool,
    pub capture_may_be_ambiguous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlTestRun {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub requested_url: String,
    pub final_url: Option<String>,
    pub status: UrlDiagnosticStatus,
    pub page_load_strategy: UrlPageLoadStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_protocol_primary_load: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advertised_alt_svc: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validated_http_versions: Vec<String>,
    /// Security-header audit derived from the response headers captured by the
    /// protocol validation probes — the same derivation as
    /// `HttpResult::security_headers` (see [`SecurityHeaders`], measurement
    /// gap #14). `None` when no probe captured headers. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_headers: Option<SecurityHeaders>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cipher_suite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dom_content_loaded_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_event_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_idle_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_end_ms: Option<f64>,
    pub total_requests: u32,
    pub total_transfer_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_concurrent_connections: Option<u32>,
    pub redirect_count: u32,
    pub failure_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub har_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcap_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcap_summary: Option<UrlPacketCaptureSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture_errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origin_summaries: Vec<UrlOriginSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_summary: Option<UrlConnectionSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<UrlTestResource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocol_runs: Vec<UrlTestProtocolRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlOriginSummary {
    pub origin: String,
    pub request_count: u32,
    pub failure_count: u32,
    pub total_transfer_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dominant_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlConnectionSummary {
    pub total_connection_ids: u32,
    pub reused_connection_count: u32,
    pub reused_resource_count: u32,
    pub resources_with_connection_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_origin_request_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlTestResource {
    pub url_test_run_id: Uuid,
    pub resource_url: String,
    pub origin: String,
    pub resource_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoded_body_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_body_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reused_connection: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cache: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirected: Option<bool>,
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlTestProtocolRun {
    pub url_test_run_id: Uuid,
    pub protocol_mode: String,
    pub run_number: u32,
    pub attempt_type: UrlProbeAttemptType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_occurred: Option<bool>,
    pub succeeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub category: ErrorCategory,
    pub message: String,
    pub detail: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ErrorCategory {
    Dns,
    Tcp,
    Tls,
    Http,
    Udp,
    Timeout,
    Config,
    Other,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ErrorCategory::Dns => "dns",
            ErrorCategory::Tcp => "tcp",
            ErrorCategory::Tls => "tls",
            ErrorCategory::Http => "http",
            ErrorCategory::Udp => "udp",
            ErrorCategory::Timeout => "timeout",
            ErrorCategory::Config => "config",
            ErrorCategory::Other => "other",
        };
        write!(f, "{s}")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UDP RTT aggregation helper
// ─────────────────────────────────────────────────────────────────────────────

pub struct RttStats {
    pub min: f64,
    pub avg: f64,
    pub p95: f64,
    /// Mean |IPDV| — see [`aggregate_udp_rtts`] for the exact estimator.
    pub jitter: f64,
    /// 95th percentile of the |IPDV| samples; `None` below
    /// [`MIN_SAMPLES_P95`] pairs (small-n honesty gate).
    pub ipdv_p95: Option<f64>,
    /// 99th percentile of the |IPDV| samples; `None` below
    /// [`MIN_SAMPLES_P99`] pairs.
    pub ipdv_p99: Option<f64>,
    pub loss_percent: f64,
}

/// Compute aggregate stats from a slice of `Option<f64>` RTT samples.
/// None values count as lost probes.
///
/// `jitter` is the mean inter-probe delay variation (IPDV): mean |Δ| of RTTs
/// paired **consecutive-by-sequence over received probes** (the input slice
/// is indexed by sequence number; lost probes are skipped when pairing). This
/// is the RFC 3393 §4.2 selection function applied to round-trip delay — NOT
/// RFC 3550 interarrival jitter (a 1/16-gain EWMA over one-way transit
/// deltas), and not arrival-order pairing (a reordered late echo is paired by
/// its sequence position, where it was credited).
pub fn aggregate_udp_rtts(samples: &[Option<f64>]) -> RttStats {
    let total = samples.len() as f64;
    let mut rtts: Vec<f64> = samples.iter().filter_map(|v| *v).collect();
    let received = rtts.len() as f64;
    let loss = if total > 0.0 {
        (total - received) / total * 100.0
    } else {
        100.0
    };

    if rtts.is_empty() {
        return RttStats {
            min: 0.0,
            avg: 0.0,
            p95: 0.0,
            jitter: 0.0,
            ipdv_p95: None,
            ipdv_p99: None,
            loss_percent: loss,
        };
    }

    // Mean |IPDV| over sequence-consecutive received pairs. This must be
    // computed BEFORE sorting — successive diffs of a sorted array telescope
    // to (max − min) / (n − 1), a range statistic, not delay variation.
    // (Trust audit V2.)
    let mut ipdv_samples: Vec<f64> = rtts.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let jitter = if ipdv_samples.is_empty() {
        0.0
    } else {
        ipdv_samples.iter().sum::<f64>() / ipdv_samples.len() as f64
    };
    // Tail percentiles of the IPDV distribution (a mean hides bimodality —
    // RFC 5481 §4.2), gated by the project-wide small-n rules.
    ipdv_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let ipdv_p95 = (ipdv_samples.len() >= MIN_SAMPLES_P95)
        .then(|| percentile_from_sorted(&ipdv_samples, 95.0));
    let ipdv_p99 = (ipdv_samples.len() >= MIN_SAMPLES_P99)
        .then(|| percentile_from_sorted(&ipdv_samples, 99.0));

    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = rtts[0];
    let avg = rtts.iter().sum::<f64>() / received;
    let p95_idx = ((rtts.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    let p95 = rtts[p95_idx.min(rtts.len() - 1)];

    RttStats {
        min,
        avg,
        p95,
        jitter,
        ipdv_p95,
        ipdv_p99,
        loss_percent: loss,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics aggregation
// ─────────────────────────────────────────────────────────────────────────────

/// Minimum sample count required before a p95 estimate is reported.
/// Below this, the interpolated p95 is essentially the max and would
/// mislead readers into thinking it is a tail estimate.
pub const MIN_SAMPLES_P95: usize = 20;
/// Minimum sample count required before a p99 estimate is reported.
pub const MIN_SAMPLES_P99: usize = 100;

/// Descriptive statistics for a series of floating-point measurements.
///
/// `p95`/`p99` are `None` when the sample count is below
/// [`MIN_SAMPLES_P95`]/[`MIN_SAMPLES_P99`] — at small n (the default is
/// `--runs 3`) an interpolated tail percentile is not a meaningful estimate.
#[derive(Debug, Clone)]
pub struct Stats {
    pub count: usize,
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    /// 95th percentile; `None` when `count < MIN_SAMPLES_P95`.
    pub p95: Option<f64>,
    /// 99th percentile; `None` when `count < MIN_SAMPLES_P99`.
    pub p99: Option<f64>,
    pub max: f64,
    pub stddev: f64,
}

/// Compute summary statistics from a slice of `f64`.
/// Returns `None` if `values` is empty.
pub fn compute_stats(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = sorted.len();
    let min = sorted[0];
    let max = sorted[count - 1];
    let mean = sorted.iter().sum::<f64>() / count as f64;
    let p50 = percentile_from_sorted(&sorted, 50.0);
    // Sample-size guard: suppress tail percentiles that would be
    // indistinguishable from the max at small n (V13).
    let p95 = (count >= MIN_SAMPLES_P95).then(|| percentile_from_sorted(&sorted, 95.0));
    let p99 = (count >= MIN_SAMPLES_P99).then(|| percentile_from_sorted(&sorted, 99.0));
    let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count as f64;
    let stddev = variance.sqrt();
    Some(Stats {
        count,
        min,
        mean,
        p50,
        p95,
        p99,
        max,
        stddev,
    })
}

fn percentile_from_sorted(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = p / 100.0 * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

/// Human-readable label for the primary metric used when computing statistics
/// for a given protocol.
pub fn primary_metric_label(proto: &Protocol) -> &'static str {
    match proto {
        Protocol::Http1 | Protocol::Http2 | Protocol::Http3 | Protocol::Native | Protocol::Curl => {
            "Total ms"
        }
        Protocol::Tcp => "Connect ms",
        Protocol::Udp => "RTT avg ms",
        Protocol::Download
        | Protocol::Download1
        | Protocol::Download2
        | Protocol::Download3
        | Protocol::Upload
        | Protocol::Upload1
        | Protocol::Upload2
        | Protocol::Upload3
        | Protocol::WebDownload
        | Protocol::WebUpload
        | Protocol::UdpDownload
        | Protocol::UdpUpload => "Throughput MB/s",
        // Round-trips per minute under load — higher is better, like the
        // throughput modes.
        Protocol::Rpm => "RPM",
        // Draft-conformant responsiveness — the download-direction RPM is the
        // headline (higher is better), labeled to distinguish it from the
        // UDP-echo-derived `rpm` figure.
        Protocol::Responsiveness => "RPM (draft)",
        // Multi-connection link capacity (download direction) — labeled apart
        // from the single-connection "Throughput MB/s" because the two are
        // different measurements (link capacity vs per-flow fair share).
        Protocol::Mthroughput => "Capacity MB/s",
        // Processing-corrected round-trip time is the probe's distinguishing
        // measurement; directional loss/jitter are reported separately.
        Protocol::Stamp => "RTT avg ms",
        Protocol::Ping => "RTT avg ms",
        // Path length is the probe's distinguishing measurement; destination
        // RTT is already covered by ping/tcp and is reported separately in
        // the summary. `None` in degraded mode when the destination never
        // answered — an unknown path length is not a 0-hop path.
        Protocol::Path => "Hops",
        // Total request time of the family a happy-eyeballs client would not
        // pick is diagnostic detail; the winning family's total is the number
        // a user experiences.
        Protocol::DualStack => "Total ms",
        // Steady-state message round-trip is the probe's distinguishing
        // number; the one-time upgrade cost is reported separately in the
        // summary section.
        Protocol::WebSocket => "Msg RTT avg ms",
        // Bytes, not milliseconds — the discovered path MTU itself.
        Protocol::Pmtud => "Path MTU",
        Protocol::Dns => "Resolve ms",
        Protocol::Tls | Protocol::TlsResume => "Handshake ms",
        Protocol::PageLoad | Protocol::PageLoad2 | Protocol::PageLoad3 => "Total ms",
        Protocol::Browser | Protocol::Browser1 | Protocol::Browser2 | Protocol::Browser3 => {
            "Load ms"
        }
        // Primary metric is total request latency; the distinguishing output
        // (network-vs-server split) is surfaced separately in the summary.
        Protocol::SdkProbe => "Total ms",
    }
}

/// Extracts payload bytes from an attempt (throughput protocols only).
/// Returns None for non-throughput protocols and for payload == 0.
pub fn attempt_payload_bytes(a: &RequestAttempt) -> Option<usize> {
    a.http
        .as_ref()
        .map(|h| h.payload_bytes)
        .filter(|&b| b > 0)
        .or_else(|| {
            a.udp_throughput
                .as_ref()
                .map(|ut| ut.payload_bytes)
                .filter(|&b| b > 0)
        })
}

/// Extract the primary metric value from an attempt for statistics purposes.
/// Returns `None` if the relevant sub-result is absent.
pub fn primary_metric_value(a: &RequestAttempt) -> Option<f64> {
    match a.protocol {
        Protocol::Http1 | Protocol::Http2 | Protocol::Http3 | Protocol::Native | Protocol::Curl => {
            a.http.as_ref().map(|h| h.total_duration_ms)
        }
        Protocol::Tcp => a.tcp.as_ref().map(|t| t.connect_duration_ms),
        // A fully-lost UDP attempt has no RTT samples: its rtt_avg_ms is a
        // 0.0 sentinel, not a measurement. Excluding it keeps lost attempts
        // out of the RTT distribution — loss is reported via loss_percent.
        // (Trust audit V11.)
        Protocol::Udp => a
            .udp
            .as_ref()
            .filter(|u| u.success_count > 0)
            .map(|u| u.rtt_avg_ms),
        Protocol::Download
        | Protocol::Download1
        | Protocol::Download2
        | Protocol::Download3
        | Protocol::Upload
        | Protocol::Upload1
        | Protocol::Upload2
        | Protocol::Upload3
        | Protocol::WebDownload
        | Protocol::WebUpload => a.http.as_ref().and_then(|h| h.throughput_mbps),
        Protocol::UdpDownload | Protocol::UdpUpload => {
            a.udp_throughput.as_ref().and_then(|ut| ut.throughput_mbps)
        }
        // `rpm` is None when every loaded probe was lost — same "no samples is
        // not a 0.0 measurement" rule as the UDP arm above (trust audit V11).
        Protocol::Rpm => a.rpm.as_ref().and_then(|r| r.rpm),
        // Download-direction draft RPM; None when either probe family
        // produced no samples (no fabricated responsiveness).
        Protocol::Responsiveness => a.responsiveness.as_ref().and_then(|r| r.rpm_download),
        // Download-direction aggregate capacity; None when the measure window
        // moved no bytes (no fabricated capacity).
        Protocol::Mthroughput => a.mthroughput.as_ref().and_then(|m| m.capacity_down_mbps),
        // A fully-lost stamp attempt carries a 0.0 sentinel avg, not a
        // measurement — excluded exactly like the UDP arm (trust audit V11).
        Protocol::Stamp => a
            .stamp
            .as_ref()
            .filter(|s| s.replies_received > 0)
            .map(|s| s.rtt_avg_ms),
        // Fully-lost ping attempts carry a 0.0 sentinel avg, not a
        // measurement — excluded exactly like the UDP arm (trust audit V11).
        Protocol::Ping => a
            .ping
            .as_ref()
            .filter(|p| p.success_count > 0)
            .map(|p| p.rtt_avg_ms),
        // Hop count; None when the destination never answered (unknown path
        // length must not enter the distribution as a number).
        Protocol::Path => a.path.as_ref().and_then(|p| p.hop_count).map(|h| h as f64),
        // Winning family's total; falls back to whichever single family
        // succeeded. None when neither leg completed.
        Protocol::DualStack => a.dualstack.as_ref().and_then(|d| {
            match (d.faster_family.as_deref(), d.ipv4.total_ms, d.ipv6.total_ms) {
                (Some("ipv4"), v4, _) => v4,
                (Some("ipv6"), _, v6) => v6,
                (_, Some(v4), None) => Some(v4),
                (_, None, Some(v6)) => Some(v6),
                (_, Some(v4), Some(v6)) => Some(v4.min(v6)),
                _ => None,
            }
        }),
        // An attempt whose every echo was lost carries a 0.0 sentinel avg,
        // not a measurement — excluded exactly like the UDP arm above
        // (trust audit V11).
        Protocol::WebSocket => a
            .websocket
            .as_ref()
            .filter(|w| w.echo_count > 0)
            .map(|w| w.msg_rtt_avg_ms),
        // Discovered path MTU in bytes; None when no feedback allowed a
        // verdict (an unknown MTU must not enter the distribution as 0).
        Protocol::Pmtud => a.pmtud.as_ref().and_then(|p| p.path_mtu).map(|m| m as f64),
        Protocol::Dns => a.dns.as_ref().map(|d| d.duration_ms),
        Protocol::Tls | Protocol::TlsResume => a.tls.as_ref().map(|t| t.handshake_duration_ms),
        Protocol::PageLoad | Protocol::PageLoad2 | Protocol::PageLoad3 => {
            a.page_load.as_ref().map(|p| p.total_ms)
        }
        Protocol::Browser | Protocol::Browser1 | Protocol::Browser2 | Protocol::Browser3 => {
            a.browser.as_ref().map(|b| b.load_ms)
        }
        Protocol::SdkProbe => a.http.as_ref().map(|h| h.total_duration_ms),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // compute_loss_pattern — RFC 3357 loss-pattern classification
    // ─────────────────────────────────────────────────────────────────────────

    /// Build a timeline: `Some(1.0)` for received, `None` for the given
    /// zero-based loss indices, over `n` probes.
    fn timeline(n: usize, lost: &[usize]) -> Vec<Option<f64>> {
        (0..n)
            .map(|i| if lost.contains(&i) { None } else { Some(1.0) })
            .collect()
    }

    #[test]
    fn loss_pattern_none_below_min_probes() {
        // Fewer than MIN_LOSS_PATTERN_PROBES → refuse to characterize.
        assert!(compute_loss_pattern(&timeline(MIN_LOSS_PATTERN_PROBES - 1, &[3])).is_none());
    }

    #[test]
    fn loss_pattern_no_loss() {
        let p = compute_loss_pattern(&timeline(25, &[])).expect("enough probes");
        assert_eq!(p.classification, "no-loss");
        assert_eq!(p.lost_count, 0);
        assert_eq!(p.loss_burst_count, 0);
        assert_eq!(p.loss_max_burst, 0);
        assert!(p.loss_mean_distance.is_none());
    }

    #[test]
    fn loss_pattern_random_like_scattered_singles() {
        // Isolated single losses spread out → random-like.
        let p = compute_loss_pattern(&timeline(30, &[3, 12, 21])).expect("some");
        assert_eq!(p.classification, "random-like");
        assert_eq!(p.lost_count, 3);
        assert_eq!(p.loss_burst_count, 3);
        assert_eq!(p.loss_max_burst, 1);
        // Loss distances 12-3=9, 21-12=9 → mean 9.
        assert_eq!(p.loss_mean_distance, Some(9.0));
    }

    #[test]
    fn loss_pattern_single_burst() {
        // One contiguous outage of length 2 → single-burst.
        let p = compute_loss_pattern(&timeline(25, &[10, 11])).expect("some");
        assert_eq!(p.classification, "single-burst");
        assert_eq!(p.lost_count, 2);
        assert_eq!(p.loss_burst_count, 1);
        assert_eq!(p.loss_max_burst, 2);
        assert!(p.loss_mean_distance.is_none()); // one period → no distance
    }

    #[test]
    fn loss_pattern_bursty_run_of_three() {
        // A run of ≥3 consecutive losses is the congestion/buffer signature.
        let p = compute_loss_pattern(&timeline(25, &[8, 9, 10, 18])).expect("some");
        assert_eq!(p.classification, "bursty");
        assert_eq!(p.lost_count, 4);
        assert_eq!(p.loss_burst_count, 2); // {8,9,10} and {18}
        assert_eq!(p.loss_max_burst, 3);
        assert_eq!(p.loss_mean_distance, Some(10.0)); // 18-8
    }

    #[test]
    fn loss_pattern_field_is_additive_and_optional() {
        // Serializes away when None; a legacy UdpResult JSON without the key
        // still deserializes (schema stays 1.0).
        let json = r#"{"remote_addr":"1.2.3.4:9","probe_count":1,"success_count":1,
            "loss_percent":0.0,"rtt_min_ms":1.0,"rtt_avg_ms":1.0,"rtt_p95_ms":1.0,
            "jitter_ms":0.0,"started_at":"2026-01-01T00:00:00Z","probe_rtts_ms":[1.0]}"#;
        let r: UdpResult = serde_json::from_str(json).expect("legacy deserialize");
        assert!(r.loss_pattern.is_none());
        assert!(!serde_json::to_string(&r).unwrap().contains("loss_pattern"));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // diagnose_chain — certificate trust-path structure (M2 D5/D9)
    // ─────────────────────────────────────────────────────────────────────────

    fn cert(subject: &str, issuer: &str) -> CertEntry {
        CertEntry {
            subject: subject.into(),
            issuer: issuer.into(),
            expiry: None,
            sans: vec![],
            key_algorithm: None,
            key_size_bits: None,
            signature_algorithm: None,
        }
    }

    #[test]
    fn diagnose_chain_empty_is_none() {
        assert!(diagnose_chain(&[]).is_none());
    }

    #[test]
    fn diagnose_chain_complete_leaf_intermediate_root() {
        let chain = [
            cert("CN=example.com", "CN=Intermediate CA"),
            cert("CN=Intermediate CA", "CN=Root CA"),
            cert("CN=Root CA", "CN=Root CA"),
        ];
        let d = diagnose_chain(&chain).unwrap();
        assert_eq!(d.chain_length, 3);
        assert_eq!(d.links_consistent, Some(true));
        assert!(!d.missing_intermediate_suspected);
        assert!(!d.self_signed_leaf);
        assert!(!d.cross_signed_subjects);
    }

    #[test]
    fn diagnose_chain_missing_intermediate() {
        let chain = [cert("CN=example.com", "CN=Intermediate CA")];
        let d = diagnose_chain(&chain).unwrap();
        assert_eq!(d.chain_length, 1);
        assert_eq!(d.links_consistent, None);
        assert!(d.missing_intermediate_suspected);
        assert!(!d.self_signed_leaf);
    }

    #[test]
    fn diagnose_chain_self_signed_leaf() {
        let chain = [cert("CN=localhost", "CN=localhost")];
        let d = diagnose_chain(&chain).unwrap();
        assert!(d.self_signed_leaf);
        assert!(!d.missing_intermediate_suspected);
    }

    #[test]
    fn diagnose_chain_broken_link_is_inconsistent() {
        let chain = [
            cert("CN=example.com", "CN=Intermediate CA"),
            cert("CN=Some Other CA", "CN=Root CA"),
        ];
        let d = diagnose_chain(&chain).unwrap();
        assert_eq!(d.links_consistent, Some(false));
        assert!(d.missing_intermediate_suspected);
    }

    #[test]
    fn diagnose_chain_cross_signed_subjects() {
        let chain = [
            cert("CN=example.com", "CN=Intermediate CA"),
            cert("CN=Intermediate CA", "CN=Root CA"),
            cert("CN=Intermediate CA", "CN=Legacy Root"),
        ];
        let d = diagnose_chain(&chain).unwrap();
        assert!(d.cross_signed_subjects);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SocketStats — post-transfer TCP kernel stats (gap #5), additive contract
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn socket_stats_default_is_empty_and_serializes_to_empty_object() {
        let s = SocketStats::default();
        assert!(s.is_empty());
        // Every field is skip_serializing_if — an all-None struct is `{}`.
        assert_eq!(serde_json::to_string(&s).unwrap(), "{}");
    }

    #[test]
    fn socket_stats_serializes_only_present_fields() {
        let s = SocketStats {
            total_retrans: Some(7),
            congestion_algorithm: Some("bbr".into()),
            ..Default::default()
        };
        assert!(!s.is_empty());
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["total_retrans"], 7);
        assert_eq!(json["congestion_algorithm"], "bbr");
        assert!(json.get("snd_cwnd").is_none(), "None fields must be absent");
    }

    #[test]
    fn http_result_new_fields_are_additive() {
        // Pre-gap-#5/#9 JSON (no socket_stats / content_* fields) must still
        // deserialize — the fields are serde-defaulted to None.
        let old_json = serde_json::json!({
            "negotiated_version": "HTTP/1.1",
            "status_code": 200,
            "headers_size_bytes": 100,
            "body_size_bytes": 5,
            "ttfb_ms": 1.0,
            "total_duration_ms": 2.0,
            "redirect_count": 0,
            "started_at": chrono::Utc::now(),
            "response_headers": [],
        });
        let h: HttpResult = serde_json::from_value(old_json).unwrap();
        assert!(h.socket_stats.is_none());
        assert!(h.content_encoding.is_none());
        assert!(h.content_length_header.is_none());

        // And when None, the fields must be omitted on the wire (schema 1.0
        // output for non-Unix platforms is byte-identical to before).
        let out = serde_json::to_value(&h).unwrap();
        assert!(out.get("socket_stats").is_none());
        assert!(out.get("content_encoding").is_none());
        assert!(out.get("content_length_header").is_none());
    }

    #[test]
    fn test_rtt_stats_no_loss() {
        // Deliberately UNSORTED (sequence order). Percentile/min/avg must not
        // depend on sample order; the IPDV mean must (dedicated tests below).
        let samples: Vec<Option<f64>> = vec![
            Some(7.0),
            Some(2.0),
            Some(10.0),
            Some(4.0),
            Some(1.0),
            Some(6.0),
            Some(3.0),
            Some(9.0),
            Some(5.0),
            Some(8.0),
        ];
        let s = aggregate_udp_rtts(&samples);
        assert_eq!(s.loss_percent, 0.0);
        assert!((s.min - 1.0).abs() < 1e-9);
        assert!((s.avg - 5.5).abs() < 1e-9);
        // 95th percentile of 10 values → index ceil(9.5)-1 = 9 → 10.0
        assert!((s.p95 - 10.0).abs() < 1e-9);
        // Sequence-consecutive |IPDV|: 5,8,6,3,5,3,6,4,3 → mean = 43/9
        assert!(
            (s.jitter - 43.0 / 9.0).abs() < 1e-9,
            "jitter must be the mean |IPDV| in sequence order, got {}",
            s.jitter
        );
    }

    #[test]
    fn test_rtt_stats_with_loss() {
        // Unsorted sequence order; None entries are lost probes and are
        // skipped when pairing sequence-consecutive samples for the IPDV mean
        // (RFC 3393 §4.2 selection).
        let samples: Vec<Option<f64>> = vec![Some(15.0), None, Some(5.0), None, Some(10.0)];
        let s = aggregate_udp_rtts(&samples);
        assert!((s.loss_percent - 40.0).abs() < 1e-9);
        assert!((s.min - 5.0).abs() < 1e-9);
        assert!((s.avg - 10.0).abs() < 1e-9);
        // Sequence-consecutive |IPDV| over received: |5−15|=10, |10−5|=5 → mean 7.5
        assert!((s.jitter - 7.5).abs() < 1e-9, "got jitter {}", s.jitter);
    }

    /// Regression test for trust-audit V2: alternating 1/10/1/10 ms RTTs have
    /// a true mean |IPDV| of 9 ms. The old implementation sorted the samples
    /// first and reported (max−min)/(n−1) = 3 ms instead.
    #[test]
    fn test_rtt_jitter_uses_arrival_order_not_sorted() {
        let samples: Vec<Option<f64>> = vec![Some(1.0), Some(10.0), Some(1.0), Some(10.0)];
        let s = aggregate_udp_rtts(&samples);
        assert!(
            (s.jitter - 9.0).abs() < 1e-9,
            "expected sequence-order mean |IPDV| 9.0, got {} (sorted-input bug?)",
            s.jitter
        );
    }

    #[test]
    fn test_rtt_jitter_single_sample_is_zero() {
        let s = aggregate_udp_rtts(&[Some(5.0)]);
        assert_eq!(s.jitter, 0.0);
    }

    /// IPDV tail percentiles obey the project-wide small-n honesty gates:
    /// suppressed below MIN_SAMPLES_P95/P99 *pairs* (so the default 10-probe
    /// train reports none), present once enough pairs exist.
    #[test]
    fn test_rtt_ipdv_percentiles_gated_by_sample_size() {
        // 10 samples → 9 IPDV pairs < MIN_SAMPLES_P95 → both suppressed.
        let small: Vec<Option<f64>> = (0..10).map(|i| Some(i as f64)).collect();
        let s = aggregate_udp_rtts(&small);
        assert!(s.ipdv_p95.is_none(), "9 pairs must suppress ipdv_p95");
        assert!(s.ipdv_p99.is_none());

        // 30 samples alternating 1/10 ms → 29 pairs of |Δ| = 9 ms:
        // ≥ MIN_SAMPLES_P95 → p95 present (= 9.0); < MIN_SAMPLES_P99 → p99
        // still suppressed.
        let medium: Vec<Option<f64>> = (0..30)
            .map(|i| Some(if i % 2 == 0 { 1.0 } else { 10.0 }))
            .collect();
        let s = aggregate_udp_rtts(&medium);
        let p95 = s.ipdv_p95.expect("29 pairs must yield ipdv_p95");
        assert!((p95 - 9.0).abs() < 1e-9, "got {p95}");
        assert!(s.ipdv_p99.is_none(), "29 pairs must suppress ipdv_p99");

        // 101 samples → 100 pairs = MIN_SAMPLES_P99 → both present.
        let large: Vec<Option<f64>> = (0..101)
            .map(|i| Some(if i % 2 == 0 { 1.0 } else { 10.0 }))
            .collect();
        let s = aggregate_udp_rtts(&large);
        assert!(s.ipdv_p95.is_some());
        assert!(s.ipdv_p99.is_some());
    }

    /// A bimodal delay pattern the mean hides (RFC 5481 §4.2 rationale):
    /// mostly-steady RTTs with occasional big spikes must surface in the
    /// IPDV p95 while the mean stays small.
    #[test]
    fn test_rtt_ipdv_p95_exposes_bimodality() {
        // 40 probes: steady 5 ms with a 45 ms spike every 10th probe.
        let samples: Vec<Option<f64>> = (0..40)
            .map(|i| Some(if i % 10 == 9 { 45.0 } else { 5.0 }))
            .collect();
        let s = aggregate_udp_rtts(&samples);
        let p95 = s.ipdv_p95.expect("39 pairs must yield ipdv_p95");
        assert!(
            p95 > s.jitter * 2.0,
            "spiky IPDV tail (p95 {p95:.1}) must exceed the mean ({:.1}) by far",
            s.jitter
        );
    }

    #[test]
    fn test_rtt_stats_all_lost() {
        let samples: Vec<Option<f64>> = vec![None, None, None];
        let s = aggregate_udp_rtts(&samples);
        assert_eq!(s.loss_percent, 100.0);
    }

    #[test]
    fn test_protocol_roundtrip_all_variants() {
        use std::str::FromStr;
        // Every Protocol variant must survive Display→FromStr round-trip.
        let all = [
            "tcp",
            "http1",
            "http2",
            "http3",
            "udp",
            "download",
            "download1",
            "download2",
            "download3",
            "upload",
            "upload1",
            "upload2",
            "upload3",
            "webdownload",
            "webupload",
            "udpdownload",
            "udpupload",
            "rpm",
            "dns",
            "tls",
            "tlsresume",
            "native",
            "curl",
            "pageload",
            "pageload2",
            "pageload3",
            "browser",
            "browser1",
            "browser2",
            "browser3",
        ];
        for p in &all {
            let parsed = Protocol::from_str(p)
                .unwrap_or_else(|_| panic!("Protocol::from_str({p:?}) must succeed"));
            assert_eq!(
                parsed.to_string(),
                *p,
                "Display→FromStr round-trip failed for {p}"
            );
        }
        // Verify we tested every variant (count must match enum variant count).
        assert_eq!(
            all.len(),
            30,
            "Update this test when adding Protocol variants"
        );
    }

    #[test]
    fn protocol_from_str_rejects_unknown() {
        use std::str::FromStr;
        assert!(Protocol::from_str("unknown").is_err());
        assert!(Protocol::from_str("").is_err());
        assert!(Protocol::from_str("http4").is_err());
    }

    #[test]
    fn protocol_from_str_is_case_insensitive() {
        use std::str::FromStr;
        assert_eq!(Protocol::from_str("HTTP1").unwrap(), Protocol::Http1);
        assert_eq!(Protocol::from_str("DNS").unwrap(), Protocol::Dns);
        assert_eq!(Protocol::from_str("PageLoad").unwrap(), Protocol::PageLoad);
    }

    #[test]
    fn error_category_display_round_trip() {
        // Verify all ErrorCategory variants produce expected strings.
        assert_eq!(ErrorCategory::Dns.to_string(), "dns");
        assert_eq!(ErrorCategory::Tcp.to_string(), "tcp");
        assert_eq!(ErrorCategory::Tls.to_string(), "tls");
        assert_eq!(ErrorCategory::Http.to_string(), "http");
        assert_eq!(ErrorCategory::Udp.to_string(), "udp");
        assert_eq!(ErrorCategory::Timeout.to_string(), "timeout");
        assert_eq!(ErrorCategory::Config.to_string(), "config");
        assert_eq!(ErrorCategory::Other.to_string(), "other");
    }

    #[test]
    fn error_category_serde_round_trip() {
        for cat in [
            ErrorCategory::Dns,
            ErrorCategory::Tcp,
            ErrorCategory::Tls,
            ErrorCategory::Http,
            ErrorCategory::Udp,
            ErrorCategory::Timeout,
            ErrorCategory::Config,
            ErrorCategory::Other,
        ] {
            let json = serde_json::to_string(&cat).unwrap();
            let de: ErrorCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(de, cat, "serde round-trip failed for {cat:?}");
        }
    }

    #[test]
    fn test_test_run_counts() {
        let run_id = Uuid::new_v4();
        let mk = |success: bool| RequestAttempt {
            phase: None,
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Http1,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: None,
            success,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
            rpm: None,
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
            mthroughput: None,
        };
        let run = TestRun {
            schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
            run_id,
            started_at: Utc::now(),
            finished_at: None,
            target_url: "http://x".into(),
            target_host: "x".into(),
            modes: vec![],
            total_runs: 3,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            client_network: None,
            client_load_before: None,
            client_load_after: None,
            cpu_usage: None,
            clock_sync: None,
            baseline: None,
            packet_capture_summary: None,
            benchmark_environment_check: None,
            benchmark_stability_check: None,
            benchmark_phase: None,
            benchmark_scenario: None,
            benchmark_launch_index: None,
            benchmark_warmup_attempt_count: 0,
            benchmark_pilot_attempt_count: 0,
            benchmark_overhead_attempt_count: 0,
            benchmark_cooldown_attempt_count: 0,
            benchmark_execution_plan: None,
            benchmark_noise_thresholds: None,
            client_geo: None,
            target_geo: None,
            attempts: vec![mk(true), mk(false), mk(true)],
        };
        assert_eq!(run.success_count(), 2);
        assert_eq!(run.failure_count(), 1);
    }

    #[test]
    fn test_json_serialization() {
        let r = DnsResult {
            query_name: "example.com".into(),
            resolved_ips: vec!["93.184.216.34".into()],
            duration_ms: 12.5,
            started_at: Utc::now(),
            success: true,
            resolver: Some("system (192.168.1.1:53)".into()),
            a_ms: None,
            aaaa_ms: None,
            a_record_count: None,
            aaaa_record_count: None,
            cname_chain: Vec::new(),
            a_ttl_secs: None,
            aaaa_ttl_secs: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        let de: DnsResult = serde_json::from_str(&json).unwrap();
        assert_eq!(de.query_name, r.query_name);
        assert!((de.duration_ms - r.duration_ms).abs() < 1e-9);
        assert_eq!(de.resolver.as_deref(), Some("system (192.168.1.1:53)"));
    }

    /// Pre-0.28.19 JSON has no `resolver` field — must still deserialize.
    #[test]
    fn test_dns_result_deserializes_without_resolver_field() {
        let json = r#"{
            "query_name": "example.com",
            "resolved_ips": ["93.184.216.34"],
            "duration_ms": 3.5,
            "started_at": "2026-01-01T00:00:00Z",
            "success": true
        }"#;
        let de: DnsResult = serde_json::from_str(json).unwrap();
        assert_eq!(de.resolver, None);
    }

    /// Measurement-gap #6 additive fields: JSON produced before the per-record-
    /// type DNS depth (a_ms/aaaa_ms/counts/cname_chain) must still deserialize,
    /// defaulting every new field.
    #[test]
    fn test_dns_result_deserializes_without_per_type_fields() {
        let json = r#"{
            "query_name": "example.com",
            "resolved_ips": ["93.184.216.34"],
            "duration_ms": 3.5,
            "started_at": "2026-01-01T00:00:00Z",
            "success": true,
            "resolver": "system (192.168.1.1:53)"
        }"#;
        let de: DnsResult = serde_json::from_str(json).unwrap();
        assert_eq!(de.a_ms, None);
        assert_eq!(de.aaaa_ms, None);
        assert_eq!(de.a_record_count, None);
        assert_eq!(de.aaaa_record_count, None);
        assert!(de.cname_chain.is_empty());
        // And the new fields are omitted on serialize when unset — the frozen
        // 1.0 shape is unchanged for producers that don't populate them.
        let v = serde_json::to_value(&de).unwrap();
        assert!(v.get("a_ms").is_none());
        assert!(v.get("cname_chain").is_none());
    }

    /// Measurement-gap #7 additive fields: old TlsResult/CertEntry JSON
    /// (without key/signature/OCSP detail) must still deserialize.
    #[test]
    fn test_tls_result_deserializes_without_cert_depth_fields() {
        let json = r#"{
            "protocol_version": "TLSv1.3",
            "cipher_suite": "TLS13_AES_128_GCM_SHA256",
            "alpn_negotiated": null,
            "cert_subject": null,
            "cert_issuer": null,
            "cert_expiry": null,
            "handshake_duration_ms": 12.0,
            "started_at": "2026-01-01T00:00:00Z",
            "success": true,
            "cert_chain": [
                {"subject": "CN=x", "issuer": "CN=y"}
            ]
        }"#;
        let de: TlsResult = serde_json::from_str(json).unwrap();
        assert_eq!(de.ocsp_stapled, None);
        assert_eq!(de.ocsp_response_bytes, None);
        let leaf = &de.cert_chain[0];
        assert_eq!(leaf.key_algorithm, None);
        assert_eq!(leaf.key_size_bits, None);
        assert_eq!(leaf.signature_algorithm, None);
        // Unset new fields stay omitted on serialize.
        let v = serde_json::to_value(&de).unwrap();
        assert!(v.get("ocsp_stapled").is_none());
        assert!(v.pointer("/cert_chain/0/key_algorithm").is_none());
    }

    #[test]
    fn url_test_run_serialization_round_trip() {
        let run_id = Uuid::new_v4();
        let run = UrlTestRun {
            id: run_id,
            started_at: Utc::now(),
            completed_at: None,
            requested_url: "https://example.com".into(),
            final_url: Some("https://www.example.com/home".into()),
            status: UrlDiagnosticStatus::Partial,
            page_load_strategy: UrlPageLoadStrategy::Browser,
            browser_engine: Some("chromium".into()),
            browser_version: Some("123.0".into()),
            user_agent: Some("NetworkerTester/0.13".into()),
            primary_origin: Some("https://www.example.com".into()),
            observed_protocol_primary_load: Some("h3".into()),
            advertised_alt_svc: Some("h3=\":443\"".into()),
            validated_http_versions: vec!["h1".into(), "h2".into(), "h3".into()],
            security_headers: None,
            tls_version: Some("TLS 1.3".into()),
            cipher_suite: Some("TLS_AES_128_GCM_SHA256".into()),
            alpn: Some("h3".into()),
            dns_ms: Some(12.0),
            connect_ms: Some(18.0),
            handshake_ms: Some(24.0),
            ttfb_ms: Some(61.0),
            dom_content_loaded_ms: Some(410.0),
            load_event_ms: Some(842.0),
            network_idle_ms: None,
            capture_end_ms: Some(1100.0),
            total_requests: 37,
            total_transfer_bytes: 2_800_000,
            peak_concurrent_connections: Some(8),
            redirect_count: 1,
            failure_count: 1,
            har_path: Some("/tmp/url-test.har".into()),
            pcap_path: None,
            pcap_summary: None,
            capture_errors: vec!["pcap unavailable".into()],
            environment_notes: Some("linux runner".into()),
            origin_summaries: vec![],
            connection_summary: None,
            resources: vec![UrlTestResource {
                url_test_run_id: run_id,
                resource_url: "https://www.example.com/app.js".into(),
                origin: "https://www.example.com".into(),
                resource_type: "script".into(),
                mime_type: Some("application/javascript".into()),
                status_code: Some(200),
                protocol: Some("h3".into()),
                transfer_size: Some(2048),
                encoded_body_size: Some(1800),
                decoded_body_size: Some(4096),
                duration_ms: Some(32.0),
                connection_id: Some("conn-1".into()),
                reused_connection: Some(true),
                initiator_type: Some("parser".into()),
                from_cache: Some(false),
                redirected: Some(false),
                failed: false,
            }],
            protocol_runs: vec![UrlTestProtocolRun {
                url_test_run_id: run_id,
                protocol_mode: "h3".into(),
                run_number: 1,
                attempt_type: UrlProbeAttemptType::Probe,
                observed_protocol: Some("h3".into()),
                fallback_occurred: Some(false),
                succeeded: true,
                status_code: Some(200),
                ttfb_ms: Some(55.0),
                total_ms: Some(320.0),
                failure_reason: None,
                error: None,
            }],
        };

        let json = serde_json::to_string(&run).unwrap();
        let de: UrlTestRun = serde_json::from_str(&json).unwrap();
        assert_eq!(de.id, run.id);
        assert_eq!(de.status, UrlDiagnosticStatus::Partial);
        assert_eq!(de.page_load_strategy, UrlPageLoadStrategy::Browser);
        assert_eq!(de.resources.len(), 1);
        assert_eq!(de.protocol_runs.len(), 1);
        assert_eq!(de.validated_http_versions, vec!["h1", "h2", "h3"]);
    }

    #[test]
    fn compute_stats_empty_returns_none() {
        assert!(compute_stats(&[]).is_none());
    }

    #[test]
    fn compute_stats_single_value() {
        let s = compute_stats(&[7.0]).unwrap();
        assert_eq!(s.count, 1);
        assert!((s.min - 7.0).abs() < 1e-9);
        assert!((s.max - 7.0).abs() < 1e-9);
        assert!((s.mean - 7.0).abs() < 1e-9);
        assert!((s.p50 - 7.0).abs() < 1e-9);
        // n=1 is far below the sample-size guard — tail percentiles suppressed.
        assert!(s.p95.is_none());
        assert!(s.p99.is_none());
        assert!((s.stddev - 0.0).abs() < 1e-9);
    }

    #[test]
    fn compute_stats_known_values() {
        // 1..=10: mean=5.5, stddev=sqrt(8.25)≈2.872, p50=5.5
        let vals: Vec<f64> = (1..=10).map(|v| v as f64).collect();
        let s = compute_stats(&vals).unwrap();
        assert_eq!(s.count, 10);
        assert!((s.min - 1.0).abs() < 1e-9);
        assert!((s.max - 10.0).abs() < 1e-9);
        assert!((s.mean - 5.5).abs() < 1e-9);
        // p50: rank=4.5 → 5+0.5*(6-5)=5.5
        assert!((s.p50 - 5.5).abs() < 1e-9);
        // n=10 < MIN_SAMPLES_P95 — p95/p99 suppressed.
        assert!(s.p95.is_none());
        assert!(s.p99.is_none());
        // stddev of 1..10: variance = (sum of (i-5.5)^2 for i in 1..10)/10 = 8.25
        assert!((s.stddev - 8.25f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn compute_stats_p95_reported_at_min_sample_size() {
        // n=20 (== MIN_SAMPLES_P95): p95 present, p99 still suppressed.
        let vals: Vec<f64> = (1..=20).map(|v| v as f64).collect();
        let s = compute_stats(&vals).unwrap();
        // p95: rank = 0.95 * 19 = 18.05 → 19 + 0.05*(20-19) = 19.05
        assert!((s.p95.expect("p95 at n=20") - 19.05).abs() < 1e-9);
        assert!(s.p99.is_none(), "p99 must stay suppressed below n=100");
    }

    #[test]
    fn compute_stats_p99_reported_at_min_sample_size() {
        // n=100 (== MIN_SAMPLES_P99): both tail percentiles present.
        let vals: Vec<f64> = (1..=100).map(|v| v as f64).collect();
        let s = compute_stats(&vals).unwrap();
        // p95: rank = 0.95 * 99 = 94.05 → 95 + 0.05*(96-95) = 95.05
        assert!((s.p95.expect("p95 at n=100") - 95.05).abs() < 1e-9);
        // p99: rank = 0.99 * 99 = 98.01 → 99 + 0.01*(100-99) = 99.01
        assert!((s.p99.expect("p99 at n=100") - 99.01).abs() < 1e-9);
    }

    #[test]
    fn compute_stats_percentiles_suppressed_below_guard() {
        let vals19: Vec<f64> = (1..=19).map(|v| v as f64).collect();
        assert!(compute_stats(&vals19).unwrap().p95.is_none());
        let vals99: Vec<f64> = (1..=99).map(|v| v as f64).collect();
        let s = compute_stats(&vals99).unwrap();
        assert!(s.p95.is_some());
        assert!(s.p99.is_none());
    }

    #[test]
    fn compute_stats_percentile_ordering_property() {
        // p50 ≤ p95 ≤ p99 ≤ max whenever all are reported.
        let vals: Vec<f64> = (0..150).map(|v| ((v * 37) % 151) as f64).collect();
        let s = compute_stats(&vals).unwrap();
        let p95 = s.p95.unwrap();
        let p99 = s.p99.unwrap();
        assert!(s.p50 <= p95, "p50 {} > p95 {}", s.p50, p95);
        assert!(p95 <= p99, "p95 {p95} > p99 {p99}");
        assert!(p99 <= s.max, "p99 {} > max {}", p99, s.max);
    }

    // Helper to build a minimal RequestAttempt with no sub-results.
    fn bare_attempt(proto: Protocol) -> RequestAttempt {
        RequestAttempt {
            phase: None,
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: proto,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
            rpm: None,
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
            mthroughput: None,
        }
    }

    // ── primary_metric_label ──────────────────────────────────────────────────

    #[test]
    fn primary_metric_label_dns() {
        assert_eq!(primary_metric_label(&Protocol::Dns), "Resolve ms");
    }

    #[test]
    fn primary_metric_label_tls() {
        assert_eq!(primary_metric_label(&Protocol::Tls), "Handshake ms");
    }

    #[test]
    fn primary_metric_label_browser() {
        assert_eq!(primary_metric_label(&Protocol::Browser), "Load ms");
    }

    #[test]
    fn primary_metric_label_throughput_protocols() {
        for proto in [
            Protocol::Download,
            Protocol::Upload,
            Protocol::WebDownload,
            Protocol::WebUpload,
            Protocol::UdpDownload,
            Protocol::UdpUpload,
        ] {
            assert_eq!(primary_metric_label(&proto), "Throughput MB/s");
        }
    }

    // ── primary_metric_value ──────────────────────────────────────────────────

    #[test]
    fn primary_metric_value_dns_present() {
        let mut a = bare_attempt(Protocol::Dns);
        a.dns = Some(DnsResult {
            query_name: "example.com".into(),
            resolved_ips: vec![],
            duration_ms: 42.0,
            started_at: Utc::now(),
            success: true,
            resolver: None,
            a_ms: None,
            aaaa_ms: None,
            a_record_count: None,
            aaaa_record_count: None,
            cname_chain: Vec::new(),
            a_ttl_secs: None,
            aaaa_ttl_secs: None,
        });
        assert!((primary_metric_value(&a).unwrap() - 42.0).abs() < 1e-9);
    }

    #[test]
    fn primary_metric_value_dns_absent() {
        let a = bare_attempt(Protocol::Dns);
        assert!(primary_metric_value(&a).is_none());
    }

    #[test]
    fn primary_metric_value_tls_present() {
        let mut a = bare_attempt(Protocol::Tls);
        a.tls = Some(TlsResult {
            protocol_version: "TLSv1.3".into(),
            cipher_suite: "AES_256_GCM".into(),
            alpn_negotiated: None,
            cert_subject: None,
            cert_issuer: None,
            cert_expiry: None,
            handshake_duration_ms: 7.5,
            chain_diagnosis: None,
            started_at: Utc::now(),
            success: true,
            cert_chain: vec![],
            tls_backend: None,
            resumed: None,
            handshake_kind: None,
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
            ocsp_stapled: None,
            ocsp_response_bytes: None,
            quic_resumed: None,
            zero_rtt_attempted: None,
            zero_rtt_accepted: None,
            quic_resumed_handshake_ms: None,
        });
        assert!((primary_metric_value(&a).unwrap() - 7.5).abs() < 1e-9);
    }

    #[test]
    fn primary_metric_value_tls_absent() {
        let a = bare_attempt(Protocol::Tls);
        assert!(primary_metric_value(&a).is_none());
    }

    #[test]
    fn primary_metric_value_browser_present() {
        let mut a = bare_attempt(Protocol::Browser);
        a.browser = Some(BrowserResult {
            load_ms: 350.0,
            dom_content_loaded_ms: 200.0,
            ttfb_ms: 50.0,
            resource_count: 10,
            transferred_bytes: 102400,
            protocol: "h2".into(),
            resource_protocols: vec![("h2".into(), 10)],
            started_at: Utc::now(),
            lcp_ms: None,
            cls: None,
            fcp_ms: None,
            tbt_ms: None,
            wire_bytes_total: None,
            waterfall: Vec::new(),
            waterfall_truncated: false,
        });
        assert!((primary_metric_value(&a).unwrap() - 350.0).abs() < 1e-9);
    }

    #[test]
    fn primary_metric_value_browser_absent() {
        let a = bare_attempt(Protocol::Browser);
        assert!(primary_metric_value(&a).is_none());
    }

    #[test]
    fn primary_metric_value_http_family() {
        for proto in [
            Protocol::Http1,
            Protocol::Http2,
            Protocol::Http3,
            Protocol::Native,
            Protocol::Curl,
        ] {
            let mut a = bare_attempt(proto.clone());
            assert!(
                primary_metric_value(&a).is_none(),
                "{proto}: absent http should be None"
            );
            a.http = Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code: 200,
                headers_size_bytes: 0,
                body_size_bytes: 0,
                ttfb_ms: 5.0,
                total_duration_ms: 25.0,
                redirect_count: 0,
                started_at: Utc::now(),
                response_headers: vec![],
                payload_bytes: 0,
                throughput_mbps: None,
                goodput_mbps: None,
                cpu_time_ms: None,
                csw_voluntary: None,
                csw_involuntary: None,
                http_handshake_ms: None,
                socket_stats: None,
                content_encoding: None,
                content_length_header: None,
                security_headers: None,
                quic_stats: None,
                quic_resumption_stats: None,
            });
            assert!(
                (primary_metric_value(&a).unwrap() - 25.0).abs() < 1e-9,
                "{proto}: expected total_duration_ms"
            );
        }
    }

    #[test]
    fn primary_metric_value_tcp() {
        let mut a = bare_attempt(Protocol::Tcp);
        a.tcp = Some(TcpResult {
            local_addr: None,
            remote_addr: "1.2.3.4:443".into(),
            connect_duration_ms: 3.5,
            attempt_count: 1,
            started_at: Utc::now(),
            success: true,
            mss_bytes: None,
            rtt_estimate_ms: None,
            retransmits: None,
            total_retrans: None,
            snd_cwnd: None,
            snd_ssthresh: None,
            rtt_variance_ms: None,
            rcv_space: None,
            segs_out: None,
            segs_in: None,
            congestion_algorithm: None,
            delivery_rate_bps: None,
            min_rtt_ms: None,
        });
        assert!((primary_metric_value(&a).unwrap() - 3.5).abs() < 1e-9);
    }

    #[test]
    fn primary_metric_value_udp() {
        let mut a = bare_attempt(Protocol::Udp);
        a.udp = Some(UdpResult {
            local_drops: None,
            so_rcvbuf_bytes: None,
            loss_pattern: None,
            remote_addr: "1.2.3.4:9999".into(),
            probe_count: 10,
            success_count: 10,
            rtt_min_ms: 1.0,
            rtt_avg_ms: 1.5,
            rtt_p95_ms: 2.0,
            jitter_ms: 0.1,
            ipdv_p95_ms: None,
            ipdv_p99_ms: None,
            loss_percent: 0.0,
            started_at: Utc::now(),
            probe_rtts_ms: vec![Some(1.0), Some(2.0)],
        });
        assert!((primary_metric_value(&a).unwrap() - 1.5).abs() < 1e-9);
    }

    /// Regression test for trust-audit V11: a fully-lost UDP attempt carries
    /// sentinel 0.0 RTT fields. It must contribute NOTHING to the RTT
    /// distribution — before the fix each dead attempt injected 0.0 ms into
    /// min/mean/p50, so a flaky path reported *better* RTTs the more attempts
    /// fully failed.
    #[test]
    fn primary_metric_value_udp_fully_lost_attempt_is_excluded() {
        let mut a = bare_attempt(Protocol::Udp);
        a.success = false;
        a.udp = Some(UdpResult {
            local_drops: None,
            so_rcvbuf_bytes: None,
            loss_pattern: None,
            remote_addr: "1.2.3.4:9999".into(),
            probe_count: 10,
            success_count: 0,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_p95_ms: 0.0,
            jitter_ms: 0.0,
            ipdv_p95_ms: None,
            ipdv_p99_ms: None,
            loss_percent: 100.0,
            started_at: Utc::now(),
            probe_rtts_ms: vec![None; 10],
        });
        assert_eq!(
            primary_metric_value(&a),
            None,
            "100%-loss UDP attempt must not contribute a 0.0 ms sentinel RTT to stats"
        );
    }

    #[test]
    fn primary_metric_label_rpm() {
        assert_eq!(primary_metric_label(&Protocol::Rpm), "RPM");
    }

    #[test]
    fn primary_metric_value_rpm() {
        let mut a = bare_attempt(Protocol::Rpm);
        assert!(primary_metric_value(&a).is_none(), "absent rpm → None");
        a.rpm = Some(RpmResult {
            loaded_local_drops: None,
            remote_addr: "1.2.3.4:9999".into(),
            unloaded_probe_count: 10,
            unloaded_success_count: 10,
            unloaded_loss_percent: 0.0,
            unloaded_rtt_min_ms: 1.0,
            unloaded_rtt_avg_ms: 2.0,
            unloaded_rtt_p95_ms: 3.0,
            unloaded_jitter_ms: 0.2,
            loaded_probe_count: 40,
            loaded_success_count: 40,
            loaded_loss_percent: 0.0,
            loaded_rtt_min_ms: 4.0,
            loaded_rtt_avg_ms: 20.0,
            loaded_rtt_p95_ms: 60.0,
            loaded_jitter_ms: 5.0,
            loaded_probes_censored: None,
            rpm: Some(3000.0),
            bufferbloat_factor: Some(10.0),
            load_duration_ms: 5000.0,
            load_bytes_transferred: 32 * 1024 * 1024,
            load_downloads_completed: 1,
            load_throughput_mbps: Some(6.4),
            started_at: Utc::now(),
        });
        assert!((primary_metric_value(&a).unwrap() - 3000.0).abs() < 1e-9);
    }

    /// A fully-lost loaded phase carries `rpm: None` — it must not contribute
    /// a sentinel value to stats (same rule as UDP, trust audit V11).
    #[test]
    fn primary_metric_value_rpm_lost_loaded_phase_is_excluded() {
        let mut a = bare_attempt(Protocol::Rpm);
        a.success = false;
        a.rpm = Some(RpmResult {
            loaded_local_drops: None,
            remote_addr: "1.2.3.4:9999".into(),
            unloaded_probe_count: 10,
            unloaded_success_count: 10,
            unloaded_loss_percent: 0.0,
            unloaded_rtt_min_ms: 1.0,
            unloaded_rtt_avg_ms: 2.0,
            unloaded_rtt_p95_ms: 3.0,
            unloaded_jitter_ms: 0.2,
            loaded_probe_count: 40,
            loaded_success_count: 0,
            loaded_loss_percent: 100.0,
            loaded_rtt_min_ms: 0.0,
            loaded_rtt_avg_ms: 0.0,
            loaded_rtt_p95_ms: 0.0,
            loaded_jitter_ms: 0.0,
            loaded_probes_censored: None,
            rpm: None,
            bufferbloat_factor: None,
            load_duration_ms: 5000.0,
            load_bytes_transferred: 0,
            load_downloads_completed: 0,
            load_throughput_mbps: None,
            started_at: Utc::now(),
        });
        assert_eq!(primary_metric_value(&a), None);
    }

    #[test]
    fn primary_metric_value_throughput_protocols() {
        for proto in [
            Protocol::Download,
            Protocol::Download1,
            Protocol::Download2,
            Protocol::Download3,
            Protocol::Upload,
            Protocol::Upload1,
            Protocol::Upload2,
            Protocol::Upload3,
            Protocol::WebDownload,
            Protocol::WebUpload,
        ] {
            let mut a = bare_attempt(proto.clone());
            assert!(
                primary_metric_value(&a).is_none(),
                "{proto}: absent http should be None"
            );
            a.http = Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code: 200,
                headers_size_bytes: 0,
                body_size_bytes: 0,
                ttfb_ms: 0.0,
                total_duration_ms: 10.0,
                redirect_count: 0,
                started_at: Utc::now(),
                response_headers: vec![],
                payload_bytes: 1024,
                throughput_mbps: Some(100.0),
                goodput_mbps: None,
                cpu_time_ms: None,
                csw_voluntary: None,
                csw_involuntary: None,
                http_handshake_ms: None,
                socket_stats: None,
                content_encoding: None,
                content_length_header: None,
                security_headers: None,
                quic_stats: None,
                quic_resumption_stats: None,
            });
            assert!(
                (primary_metric_value(&a).unwrap() - 100.0).abs() < 1e-9,
                "{proto}: expected throughput_mbps"
            );
        }
    }

    #[test]
    fn primary_metric_value_udp_throughput_protocols() {
        for proto in [Protocol::UdpDownload, Protocol::UdpUpload] {
            let mut a = bare_attempt(proto.clone());
            a.udp_throughput = Some(UdpThroughputResult {
                local_drops: None,
                so_rcvbuf_bytes: None,
                remote_addr: "127.0.0.1:9998".into(),
                payload_bytes: 1_048_576,
                datagrams_sent: 100,
                datagrams_received: Some(100),
                bytes_acked: None,
                loss_percent: 0.0,
                transfer_ms: 50.0,
                throughput_mbps: Some(20.0),
                started_at: Utc::now(),
            });
            assert!(
                (primary_metric_value(&a).unwrap() - 20.0).abs() < 1e-9,
                "{proto}: expected udp throughput_mbps"
            );
        }
    }

    #[test]
    fn primary_metric_value_pageload() {
        for proto in [Protocol::PageLoad, Protocol::PageLoad2, Protocol::PageLoad3] {
            let mut a = bare_attempt(proto.clone());
            a.page_load = Some(PageLoadResult {
                asset_count: 5,
                assets_fetched: 5,
                total_bytes: 10000,
                total_ms: 500.0,
                ttfb_ms: 50.0,
                connections_opened: 1,
                asset_timings_ms: vec![],
                started_at: Utc::now(),
                tls_setup_ms: 0.0,
                tls_overhead_ratio: 0.0,
                per_connection_tls_ms: vec![],
                cpu_time_ms: None,
                connection_reused: false,
                per_connection_socket_stats: vec![],
                assets_failed: Some(0),
            });
            assert!(
                (primary_metric_value(&a).unwrap() - 500.0).abs() < 1e-9,
                "{proto}: expected total_ms"
            );
        }
    }

    #[test]
    fn primary_metric_value_browser_variants() {
        for proto in [Protocol::Browser1, Protocol::Browser2, Protocol::Browser3] {
            let mut a = bare_attempt(proto.clone());
            a.browser = Some(BrowserResult {
                load_ms: 700.0,
                dom_content_loaded_ms: 300.0,
                ttfb_ms: 80.0,
                resource_count: 15,
                transferred_bytes: 200000,
                protocol: "h2".into(),
                resource_protocols: vec![],
                started_at: Utc::now(),
                lcp_ms: None,
                cls: None,
                fcp_ms: None,
                tbt_ms: None,
                wire_bytes_total: None,
                waterfall: Vec::new(),
                waterfall_truncated: false,
            });
            assert!(
                (primary_metric_value(&a).unwrap() - 700.0).abs() < 1e-9,
                "{proto}: expected load_ms"
            );
        }
    }

    #[test]
    fn primary_metric_value_tls_resume() {
        let mut a = bare_attempt(Protocol::TlsResume);
        a.tls = Some(TlsResult {
            protocol_version: "TLSv1.3".into(),
            cipher_suite: "AES_256_GCM".into(),
            alpn_negotiated: None,
            cert_subject: None,
            cert_issuer: None,
            cert_expiry: None,
            handshake_duration_ms: 3.0,
            chain_diagnosis: None,
            started_at: Utc::now(),
            success: true,
            cert_chain: vec![],
            tls_backend: None,
            resumed: None,
            handshake_kind: None,
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
            ocsp_stapled: None,
            ocsp_response_bytes: None,
            quic_resumed: None,
            zero_rtt_attempted: None,
            zero_rtt_accepted: None,
            quic_resumed_handshake_ms: None,
        });
        assert!((primary_metric_value(&a).unwrap() - 3.0).abs() < 1e-9);
    }

    // ── primary_metric_label exhaustive ──────────────────────────────────────

    #[test]
    fn primary_metric_label_all_variants_have_labels() {
        // Every protocol variant must produce a non-empty label.
        let all = [
            Protocol::Tcp,
            Protocol::Http1,
            Protocol::Http2,
            Protocol::Http3,
            Protocol::Udp,
            Protocol::Download,
            Protocol::Download1,
            Protocol::Download2,
            Protocol::Download3,
            Protocol::Upload,
            Protocol::Upload1,
            Protocol::Upload2,
            Protocol::Upload3,
            Protocol::WebDownload,
            Protocol::WebUpload,
            Protocol::UdpDownload,
            Protocol::UdpUpload,
            Protocol::Dns,
            Protocol::Tls,
            Protocol::TlsResume,
            Protocol::Native,
            Protocol::Curl,
            Protocol::PageLoad,
            Protocol::PageLoad2,
            Protocol::PageLoad3,
            Protocol::Browser,
            Protocol::Browser1,
            Protocol::Browser2,
            Protocol::Browser3,
            Protocol::SdkProbe,
            Protocol::Rpm,
            Protocol::Ping,
            Protocol::Path,
            Protocol::DualStack,
            Protocol::WebSocket,
            Protocol::Pmtud,
        ];
        for proto in &all {
            let label = primary_metric_label(proto);
            assert!(!label.is_empty(), "{proto}: label must not be empty");
        }
    }

    // ── websocket / pmtud (measurement gaps #10, #13) ────────────────────────

    fn sample_websocket_result() -> WebSocketResult {
        WebSocketResult {
            url: "ws://127.0.0.1:8080/ws".into(),
            upgrade_ms: 3.2,
            upgrade_status: Some(101),
            message_count: 10,
            echo_count: 10,
            loss_percent: 0.0,
            msg_rtt_min_ms: 0.4,
            msg_rtt_avg_ms: 0.9,
            msg_rtt_p95_ms: 1.8,
            jitter_ms: 0.2,
            msg_rtts_ms: vec![Some(0.9); 10],
            payload_size: 64,
            started_at: Utc::now(),
        }
    }

    #[test]
    fn primary_metric_label_websocket_and_pmtud() {
        assert_eq!(primary_metric_label(&Protocol::WebSocket), "Msg RTT avg ms");
        assert_eq!(primary_metric_label(&Protocol::Pmtud), "Path MTU");
    }

    #[test]
    fn primary_metric_value_websocket_present() {
        let mut a = bare_attempt(Protocol::WebSocket);
        a.websocket = Some(sample_websocket_result());
        assert_eq!(primary_metric_value(&a), Some(0.9));
    }

    #[test]
    fn primary_metric_value_websocket_all_echoes_lost_is_excluded() {
        // A 0.0 sentinel avg with zero echoes is not a measurement (V11).
        let mut a = bare_attempt(Protocol::WebSocket);
        a.success = false;
        a.websocket = Some(WebSocketResult {
            echo_count: 0,
            loss_percent: 100.0,
            msg_rtt_min_ms: 0.0,
            msg_rtt_avg_ms: 0.0,
            msg_rtt_p95_ms: 0.0,
            jitter_ms: 0.0,
            msg_rtts_ms: vec![None; 10],
            ..sample_websocket_result()
        });
        assert_eq!(primary_metric_value(&a), None);
    }

    #[test]
    fn primary_metric_value_pmtud_is_the_discovered_mtu() {
        let mut a = bare_attempt(Protocol::Pmtud);
        a.pmtud = Some(PmtudResult {
            remote_addr: "1.2.3.4".into(),
            path_mtu: Some(1500),
            max_unfragmented_payload: Some(1472),
            probes_sent: 12,
            method: "df-udp-echo/ip-recverr".into(),
            icmp_mtu: None,
            local_mtu: Some(1500),
            header_bytes: 28,
            lower_bound_only: false,
            started_at: Utc::now(),
        });
        assert_eq!(primary_metric_value(&a), Some(1500.0));
    }

    #[test]
    fn primary_metric_value_pmtud_unknown_mtu_is_excluded() {
        // No feedback → path_mtu None → an unknown MTU must not enter the
        // distribution as a number.
        let mut a = bare_attempt(Protocol::Pmtud);
        a.success = false;
        a.pmtud = Some(PmtudResult {
            remote_addr: "1.2.3.4".into(),
            path_mtu: None,
            max_unfragmented_payload: None,
            probes_sent: 12,
            method: "df-no-feedback".into(),
            icmp_mtu: None,
            local_mtu: Some(1500),
            header_bytes: 28,
            lower_bound_only: false,
            started_at: Utc::now(),
        });
        assert_eq!(primary_metric_value(&a), None);
    }

    #[test]
    fn websocket_and_pmtud_attempt_fields_are_additive() {
        // Pre-0.28.77 JSON (no websocket/pmtud fields) must still deserialize,
        // and None fields must be omitted on the wire (schema stays 1.0).
        let a = bare_attempt(Protocol::Tcp);
        let json = serde_json::to_value(&a).unwrap();
        assert!(json.get("websocket").is_none());
        assert!(json.get("pmtud").is_none());

        let mut old = serde_json::to_value(&a).unwrap();
        old.as_object_mut().unwrap().remove("websocket");
        old.as_object_mut().unwrap().remove("pmtud");
        let back: RequestAttempt = serde_json::from_value(old).unwrap();
        assert!(back.websocket.is_none());
        assert!(back.pmtud.is_none());
    }

    // ── attempt_payload_bytes ─────────────────────────────────────────────────

    #[test]
    fn attempt_payload_bytes_from_http() {
        let mut a = bare_attempt(Protocol::Download);
        a.http = Some(HttpResult {
            negotiated_version: "HTTP/1.1".into(),
            status_code: 200,
            headers_size_bytes: 0,
            body_size_bytes: 0,
            ttfb_ms: 0.0,
            total_duration_ms: 10.0,
            redirect_count: 0,
            started_at: Utc::now(),
            response_headers: vec![],
            payload_bytes: 65536,
            throughput_mbps: None,
            goodput_mbps: None,
            cpu_time_ms: None,
            csw_voluntary: None,
            csw_involuntary: None,
            http_handshake_ms: None,
            socket_stats: None,
            content_encoding: None,
            content_length_header: None,
            security_headers: None,
            quic_stats: None,
            quic_resumption_stats: None,
        });
        assert_eq!(attempt_payload_bytes(&a), Some(65536));
    }

    #[test]
    fn attempt_payload_bytes_zero_is_filtered() {
        let mut a = bare_attempt(Protocol::Download);
        a.http = Some(HttpResult {
            negotiated_version: "HTTP/1.1".into(),
            status_code: 200,
            headers_size_bytes: 0,
            body_size_bytes: 0,
            ttfb_ms: 0.0,
            total_duration_ms: 10.0,
            redirect_count: 0,
            started_at: Utc::now(),
            response_headers: vec![],
            payload_bytes: 0,
            throughput_mbps: None,
            goodput_mbps: None,
            cpu_time_ms: None,
            csw_voluntary: None,
            csw_involuntary: None,
            http_handshake_ms: None,
            socket_stats: None,
            content_encoding: None,
            content_length_header: None,
            security_headers: None,
            quic_stats: None,
            quic_resumption_stats: None,
        });
        assert!(attempt_payload_bytes(&a).is_none());
    }

    #[test]
    fn attempt_payload_bytes_from_udp_throughput() {
        let mut a = bare_attempt(Protocol::UdpDownload);
        a.udp_throughput = Some(UdpThroughputResult {
            local_drops: None,
            so_rcvbuf_bytes: None,
            remote_addr: "127.0.0.1:9998".into(),
            payload_bytes: 1_048_576,
            datagrams_sent: 100,
            datagrams_received: Some(100),
            bytes_acked: None,
            loss_percent: 0.0,
            transfer_ms: 50.0,
            throughput_mbps: Some(20.0),
            started_at: Utc::now(),
        });
        assert_eq!(attempt_payload_bytes(&a), Some(1_048_576));
    }

    #[test]
    fn attempt_payload_bytes_none_when_absent() {
        let a = bare_attempt(Protocol::Http1);
        assert!(attempt_payload_bytes(&a).is_none());
    }

    // ── TestRun::protocols_tested ─────────────────────────────────────────────

    #[test]
    fn protocols_tested_deduplicates() {
        let run_id = Uuid::new_v4();
        let mk = |proto: Protocol| {
            let mut a = bare_attempt(proto);
            a.run_id = run_id;
            a
        };
        let run = TestRun {
            schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
            run_id,
            started_at: Utc::now(),
            finished_at: None,
            target_url: "http://x".into(),
            target_host: "x".into(),
            modes: vec![],
            total_runs: 4,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            client_network: None,
            client_load_before: None,
            client_load_after: None,
            cpu_usage: None,
            clock_sync: None,
            baseline: None,
            packet_capture_summary: None,
            benchmark_environment_check: None,
            benchmark_stability_check: None,
            benchmark_phase: None,
            benchmark_scenario: None,
            benchmark_launch_index: None,
            benchmark_warmup_attempt_count: 0,
            benchmark_pilot_attempt_count: 0,
            benchmark_overhead_attempt_count: 0,
            benchmark_cooldown_attempt_count: 0,
            benchmark_execution_plan: None,
            benchmark_noise_thresholds: None,
            client_geo: None,
            target_geo: None,
            attempts: vec![
                mk(Protocol::Http1),
                mk(Protocol::Http2),
                mk(Protocol::Http1), // duplicate
                mk(Protocol::Http2), // duplicate
            ],
        };
        let protos = run.protocols_tested();
        assert_eq!(protos.len(), 2);
        assert!(protos.contains(&"http1".to_string()));
        assert!(protos.contains(&"http2".to_string()));
    }

    // ── RequestAttempt::total_duration_ms ────────────────────────────────────

    #[test]
    fn total_duration_ms_some_when_finished() {
        let start = Utc::now();
        let end = start + chrono::Duration::milliseconds(150);
        let mut a = bare_attempt(Protocol::Http1);
        a.started_at = start;
        a.finished_at = Some(end);
        let dur = a.total_duration_ms().unwrap();
        assert!((dur - 150.0).abs() < 1.0, "expected ~150ms, got {dur}");
    }

    #[test]
    fn total_duration_ms_none_when_not_finished() {
        let mut a = bare_attempt(Protocol::Http1);
        a.finished_at = None;
        assert!(a.total_duration_ms().is_none());
    }

    // ── SecurityHeaders derivation (measurement gap #14) ─────────────────────

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn security_headers_none_when_no_headers_captured() {
        assert_eq!(SecurityHeaders::from_response_headers(&[]), None);
    }

    #[test]
    fn security_headers_full_extraction_is_case_insensitive() {
        let sec = SecurityHeaders::from_response_headers(&hdrs(&[
            (
                "STRICT-TRANSPORT-SECURITY",
                "max-age=31536000; includeSubDomains; preload",
            ),
            ("Content-Security-Policy", "default-src 'self'"),
            ("X-CONTENT-TYPE-OPTIONS", "NOSNIFF"),
            ("x-frame-options", "SAMEORIGIN"),
            ("Referrer-Policy", "strict-origin-when-cross-origin"),
            ("Server", "nginx/1.24.0"),
        ]))
        .expect("headers present");
        assert_eq!(
            sec.hsts.as_deref(),
            Some("max-age=31536000; includeSubDomains; preload")
        );
        assert_eq!(sec.hsts_max_age_secs, Some(31_536_000));
        assert_eq!(sec.csp_present, Some(true));
        assert_eq!(sec.x_content_type_options_nosniff, Some(true));
        assert_eq!(sec.x_frame_options.as_deref(), Some("SAMEORIGIN"));
        assert_eq!(
            sec.referrer_policy.as_deref(),
            Some("strict-origin-when-cross-origin")
        );
        assert_eq!(sec.server_header.as_deref(), Some("nginx/1.24.0"));
    }

    #[test]
    fn security_headers_absent_headers_reported_honestly() {
        // Headers were captured, but none of the audited ones are present:
        // booleans are Some(false) (checked and absent), strings stay None.
        let sec =
            SecurityHeaders::from_response_headers(&hdrs(&[("content-type", "application/json")]))
                .expect("headers present");
        assert_eq!(sec.hsts, None);
        assert_eq!(sec.hsts_max_age_secs, None);
        assert_eq!(sec.csp_present, Some(false));
        assert_eq!(sec.x_content_type_options_nosniff, Some(false));
        assert_eq!(sec.x_frame_options, None);
        assert_eq!(sec.referrer_policy, None);
        assert_eq!(sec.server_header, None);
    }

    #[test]
    fn hsts_max_age_parses_directive_variants() {
        // Directive order, spacing, casing, quoting.
        assert_eq!(parse_hsts_max_age("max-age=600"), Some(600));
        assert_eq!(
            parse_hsts_max_age("includeSubDomains; MAX-AGE = 31536000"),
            Some(31_536_000)
        );
        assert_eq!(
            parse_hsts_max_age("max-age=\"86400\"; preload"),
            Some(86_400)
        );
        // Malformed max-age values → None, never a fabricated number.
        assert_eq!(parse_hsts_max_age("max-age=abc"), None);
        assert_eq!(parse_hsts_max_age("max-age=-1"), None);
        assert_eq!(parse_hsts_max_age("max-age="), None);
        assert_eq!(parse_hsts_max_age("includeSubDomains"), None);
        assert_eq!(parse_hsts_max_age(""), None);
    }

    #[test]
    fn nosniff_requires_the_nosniff_token() {
        let sec =
            SecurityHeaders::from_response_headers(&hdrs(&[("x-content-type-options", "sniff")]))
                .unwrap();
        assert_eq!(sec.x_content_type_options_nosniff, Some(false));
    }

    // ── LoadSample (measurement gap #15) ─────────────────────────────────────

    #[test]
    fn load_avg_parses_linux_and_macos_formats() {
        // Linux /proc/loadavg
        assert_eq!(parse_load_avg_1m("0.52 0.58 0.59 1/467 12034"), Some(0.52));
        // macOS `sysctl -n vm.loadavg`
        assert_eq!(parse_load_avg_1m("{ 1.86 1.99 2.06 }"), Some(1.86));
        // Garbage / empty → None
        assert_eq!(parse_load_avg_1m(""), None);
        assert_eq!(parse_load_avg_1m("not-a-number"), None);
    }

    #[test]
    fn empty_load_sample_collapses_to_none_semantics() {
        assert!(LoadSample::default().is_empty());
        let sample = LoadSample {
            load_avg_1m: Some(0.5),
            ..Default::default()
        };
        assert!(!sample.is_empty());
    }

    // ── CpuTicks / cpu_busy_percent (reserved-field collector) ───────────────

    #[test]
    fn proc_stat_cpu_line_parses_idle_including_iowait() {
        // user nice system idle iowait irq softirq steal guest guest_nice
        let stat = "cpu  100 20 30 400 50 5 5 10 7 3\n\
                    cpu0 50 10 15 200 25 2 2 5 0 0\n\
                    intr 12345\n";
        let ticks = parse_proc_stat_cpu(stat).unwrap();
        // idle = idle + iowait; total = first 8 fields (guest excluded).
        // Steal (field 8) is in the total but NOT in idle → counts as busy.
        assert_eq!(ticks.idle, 450);
        assert_eq!(ticks.total, 100 + 20 + 30 + 400 + 50 + 5 + 5 + 10);
        assert_eq!(ticks.steal, Some(10));
    }

    #[test]
    fn proc_stat_cpu_line_parses_older_kernels_without_steal() {
        // Pre-2.6.11 shape: user nice system idle (no iowait either).
        let ticks = parse_proc_stat_cpu("cpu 10 0 10 80\n").unwrap();
        assert_eq!(ticks.idle, 80);
        assert_eq!(ticks.total, 100);
        // No 8th field → steal is honestly absent, not fabricated 0.
        assert_eq!(ticks.steal, None);
    }

    #[test]
    fn proc_stat_cpu_rejects_malformed_input() {
        assert_eq!(parse_proc_stat_cpu(""), None);
        assert_eq!(parse_proc_stat_cpu("intr 12345\n"), None);
        // Per-cpu line only (no aggregate) must not match.
        assert_eq!(parse_proc_stat_cpu("cpu0 10 0 10 80\n"), None);
        assert_eq!(parse_proc_stat_cpu("cpu ten zero ten eighty\n"), None);
        // Too few fields to be meaningful.
        assert_eq!(parse_proc_stat_cpu("cpu 10 0 10\n"), None);
    }

    #[test]
    fn cpu_busy_percent_from_two_snapshots() {
        let before = CpuTicks {
            idle: 400,
            total: 1000,
            steal: None,
        };
        let after = CpuTicks {
            idle: 480, // 80 idle ticks over a 200-tick window → 60% busy
            total: 1200,
            steal: None,
        };
        let busy = after.busy_percent_since(&before).unwrap();
        assert!((busy - 60.0).abs() < 1e-9);
        // No elapsed ticks → None (never NaN/fabricated).
        assert_eq!(before.busy_percent_since(&before), None);
        // Counters going backwards (reboot / u32 wrap on macOS) → None.
        assert_eq!(before.busy_percent_since(&after), None);
        // idle_delta > total_delta (inconsistent sources) → None.
        let weird = CpuTicks {
            idle: 900,
            total: 1100,
            steal: None,
        };
        assert_eq!(weird.busy_percent_since(&before), None);
    }

    #[test]
    fn cpu_busy_percent_counts_steal_as_busy_never_idle() {
        // Window of 200 ticks: 80 idle, 40 steal, 80 genuinely busy.
        // Correct busy% = (200 - 80) / 200 = 60% — steal must land in busy.
        let before = CpuTicks {
            idle: 1000,
            total: 5000,
            steal: Some(100),
        };
        let after = CpuTicks {
            idle: 1080,
            total: 5200,
            steal: Some(140),
        };
        let busy = after.busy_percent_since(&before).unwrap();
        assert!((busy - 60.0).abs() < 1e-9);
        let steal = after.steal_percent_since(&before).unwrap();
        assert!((steal - 20.0).abs() < 1e-9);
    }

    #[test]
    fn cpu_steal_percent_requires_both_counters() {
        let with = CpuTicks {
            idle: 100,
            total: 1000,
            steal: Some(10),
        };
        let without = CpuTicks {
            idle: 200,
            total: 2000,
            steal: None,
        };
        // Either side missing steal (macOS/Windows/old kernels) → None.
        assert_eq!(without.steal_percent_since(&with), None);
        let later = CpuTicks {
            idle: 200,
            total: 2000,
            steal: Some(30),
        };
        assert!((later.steal_percent_since(&with).unwrap() - 2.0).abs() < 1e-9);
        // Steal counter going backwards → None.
        assert_eq!(with.steal_percent_since(&later), None);
    }

    #[test]
    fn cpu_tick_granularity_guard_suppresses_tiny_deltas() {
        let before = CpuTicks {
            idle: 100,
            total: 1000,
            steal: Some(0),
        };
        // 19 elapsed ticks (< MIN_CPU_WINDOW_TICKS = 20) → both None.
        let after = CpuTicks {
            idle: 105,
            total: 1019,
            steal: Some(1),
        };
        assert_eq!(after.busy_percent_since(&before), None);
        assert_eq!(after.steal_percent_since(&before), None);
        // Exactly at the guard → reported.
        let at_guard = CpuTicks {
            idle: 105,
            total: 1020,
            steal: Some(1),
        };
        assert!(at_guard.busy_percent_since(&before).is_some());
        assert!(at_guard.steal_percent_since(&before).is_some());
    }

    #[test]
    fn cpu_min_window_guard_suppresses_short_wall_clock_windows() {
        let before = CpuTicks {
            idle: 0,
            total: 0,
            steal: None,
        };
        let after = CpuTicks {
            idle: 500,
            total: 1000,
            steal: None,
        };
        // Plenty of ticks but the wall-clock window is too short → None.
        assert_eq!(
            cpu_window_sample(&before, &after, MIN_CPU_WINDOW_MS - 1),
            None
        );
        let sample = cpu_window_sample(&before, &after, MIN_CPU_WINDOW_MS).unwrap();
        assert!((sample.busy_percent - 50.0).abs() < 1e-9);
        assert_eq!(sample.steal_percent, None);
    }

    #[test]
    fn windows_cpu_math_kernel_includes_idle() {
        // GetSystemTimes contract: kernel time INCLUDES idle time, so
        // total = kernel + user counts idle exactly once and busy needs no
        // extra subtraction. kernel=1000 (600 of it idle), user=400:
        // total=1400, busy=(1400-600)/1400.
        let before = cpu_ticks_from_windows_times(0, 0, 0).unwrap();
        let after = cpu_ticks_from_windows_times(600, 1000, 400).unwrap();
        assert_eq!(after.total, 1400);
        assert_eq!(after.idle, 600);
        assert_eq!(after.steal, None); // Windows: no steal counter.
        let busy = after.busy_percent_since(&before).unwrap();
        assert!((busy - (800.0 / 1400.0 * 100.0)).abs() < 1e-9);
        // Idle exceeding the kernel bucket that contains it → inconsistent.
        assert_eq!(cpu_ticks_from_windows_times(1100, 1000, 400), None);
    }

    #[test]
    fn cpu_usage_aggregate_gates_p95_on_sample_count() {
        let sample = |busy: f64| CpuWindowSample {
            busy_percent: busy,
            steal_percent: None,
        };
        // 19 samples (< MIN_SAMPLES_P95 = 20) → p95 suppressed, max kept.
        let short: Vec<_> = (0..19).map(|i| sample(i as f64)).collect();
        let usage = CpuUsage::aggregate(None, &short, 1000).unwrap();
        assert_eq!(usage.p95_busy_percent, None);
        assert_eq!(usage.max_busy_percent, Some(18.0));
        assert_eq!(usage.sample_count, 19);
        assert_eq!(usage.sample_interval_ms, 1000);
        // 20 samples → p95 reported and ≤ max.
        let long: Vec<_> = (0..20).map(|i| sample(i as f64 * 5.0)).collect();
        let usage = CpuUsage::aggregate(None, &long, 1000).unwrap();
        let p95 = usage.p95_busy_percent.unwrap();
        assert!(p95 <= usage.max_busy_percent.unwrap());
        assert!(p95 > 85.0); // tail of a 0..95 ramp
    }

    #[test]
    fn cpu_usage_aggregate_combines_whole_run_and_samples() {
        // Nothing measured at all → None (field omitted, never zeros).
        assert_eq!(CpuUsage::aggregate(None, &[], 1000), None);
        let whole = CpuWindowSample {
            busy_percent: 30.0,
            steal_percent: Some(2.5),
        };
        // Whole-run delta only (run shorter than one sampler tick).
        let usage = CpuUsage::aggregate(Some(whole), &[], 1000).unwrap();
        assert_eq!(usage.mean_busy_percent, Some(30.0));
        assert_eq!(usage.mean_steal_percent, Some(2.5));
        assert_eq!(usage.max_busy_percent, None);
        assert_eq!(usage.sample_count, 0);
        // Samples carry the steal max independently of the whole-run mean.
        let samples = [
            CpuWindowSample {
                busy_percent: 20.0,
                steal_percent: Some(1.0),
            },
            CpuWindowSample {
                busy_percent: 90.0,
                steal_percent: Some(7.0),
            },
        ];
        let usage = CpuUsage::aggregate(Some(whole), &samples, 1000).unwrap();
        assert_eq!(usage.max_busy_percent, Some(90.0));
        assert_eq!(usage.max_steal_percent, Some(7.0));
        assert_eq!(usage.mean_busy_percent, Some(30.0));
    }

    /// Self-validation: burn exactly one thread for ~600 ms while the tick
    /// counters run, then check the measured busy% is at least half of one
    /// core's share and at most 100%. Wide tolerance by design — shared CI
    /// runners are noisy enough to flake tighter bounds, hence #[ignore];
    /// run manually with `cargo test -- --ignored cpu_self_validation`.
    #[test]
    #[ignore = "wall-clock CPU burn — flaky on shared CI runners; run manually"]
    fn cpu_self_validation_one_thread_burn() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as f64;
        let before = CpuTicks::snapshot().expect("collector available on this platform");
        let start = std::time::Instant::now();
        let burner = std::thread::spawn(move || {
            let mut x: u64 = 0;
            while start.elapsed() < std::time::Duration::from_millis(600) {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
            }
            x
        });
        std::thread::sleep(std::time::Duration::from_millis(600));
        let _ = burner.join();
        let after = CpuTicks::snapshot().expect("collector available on this platform");
        let window_ms = start.elapsed().as_millis() as u64;
        let sample = cpu_window_sample(&before, &after, window_ms)
            .expect("600 ms window passes the min-window guard");
        let floor = 100.0 / cores * 0.5;
        println!(
            "self-validation: cores={cores} window_ms={window_ms} busy={:.2}% (floor {:.2}%)",
            sample.busy_percent, floor
        );
        assert!(
            sample.busy_percent >= floor,
            "busy {} < floor {}",
            sample.busy_percent,
            floor
        );
        assert!(sample.busy_percent <= 100.0);
    }
}
