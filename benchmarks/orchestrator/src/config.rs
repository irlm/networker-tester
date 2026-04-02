use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------------------
// Dashboard-triggered benchmark config (used by executor.rs)
// ---------------------------------------------------------------------------

/// Top-level config for a dashboard-triggered benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardBenchmarkConfig {
    /// Unique identifier for this config (matches the dashboard DB row).
    pub config_id: String,
    /// Testbeds to execute — each is a VM + set of languages.
    #[serde(alias = "cells")]
    pub testbeds: Vec<TestbedConfig>,
    /// Benchmark methodology parameters.
    pub methodology: MethodologyConfig,
    /// Whether to destroy provisioned VMs after the run.
    #[serde(default)]
    pub auto_teardown: bool,
}

/// A single testbed in the benchmark matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestbedConfig {
    /// Unique identifier for this testbed.
    #[serde(alias = "cell_id")]
    pub testbed_id: String,
    /// Cloud provider: "azure", "aws", "gcp".
    pub cloud: String,
    /// Cloud region, e.g. "eastus".
    pub region: String,
    /// VM topology: "loopback", "cross-region", etc.
    #[serde(default = "default_topology")]
    pub topology: String,
    /// VM size / instance type.
    pub vm_size: String,
    /// IP of an existing VM to reuse (skip provisioning if set).
    #[serde(default)]
    pub existing_vm_ip: Option<String>,
    /// Operating system: "linux" or "windows".
    #[serde(default = "default_os")]
    pub os: String,
    /// Languages to benchmark on this testbed.
    pub languages: Vec<String>,
}

/// Benchmark methodology parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodologyConfig {
    /// Number of warmup requests before measurement.
    #[serde(default = "default_warmup")]
    pub warmup_runs: u32,
    /// Minimum number of measured requests.
    #[serde(default = "default_min_measured", alias = "measured_runs")]
    pub min_measured: u32,
    /// Maximum number of measured requests.
    #[serde(default = "default_max_measured")]
    pub max_measured: u32,
    /// Target relative error for adaptive stopping.
    #[serde(default = "default_target_relative_error")]
    pub target_relative_error: f64,
    /// Confidence level (e.g. 0.95 for 95%).
    #[serde(default = "default_confidence_level")]
    pub confidence_level: f64,
    /// Protocol modes to test, e.g. ["http1", "http2"].
    #[serde(default = "default_modes")]
    pub modes: Vec<String>,
    /// Per-request timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u32,
}

fn default_topology() -> String {
    "loopback".to_string()
}
fn default_os() -> String {
    "linux".to_string()
}
fn default_warmup() -> u32 {
    10
}
fn default_min_measured() -> u32 {
    50
}
fn default_max_measured() -> u32 {
    200
}
fn default_target_relative_error() -> f64 {
    0.05
}
fn default_confidence_level() -> f64 {
    0.95
}
fn default_modes() -> Vec<String> {
    vec!["http1".to_string(), "http2".to_string()]
}
fn default_timeout_secs() -> u32 {
    30
}

impl DashboardBenchmarkConfig {
    /// Load a dashboard benchmark config from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("parsing dashboard config from {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the config.
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.config_id.is_empty(), "config_id must not be empty");
        anyhow::ensure!(!self.testbeds.is_empty(), "testbeds list must not be empty");
        for testbed in &self.testbeds {
            anyhow::ensure!(
                !testbed.testbed_id.is_empty(),
                "testbed_id must not be empty"
            );
            anyhow::ensure!(
                !testbed.languages.is_empty(),
                "testbed {} has no languages",
                testbed.testbed_id
            );
        }
        anyhow::ensure!(
            self.methodology.min_measured > 0,
            "min_measured must be > 0"
        );
        anyhow::ensure!(
            self.methodology.timeout_secs > 0,
            "timeout_secs must be > 0"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Original orchestrator config (used by cmd_run)
// ---------------------------------------------------------------------------

/// A single language implementation to benchmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageEntry {
    /// Language name, e.g. "rust", "go", "python".
    pub name: String,
    /// Runtime / framework label, e.g. "axum", "gin", "fastapi".
    pub runtime: String,
    /// Path to the API project directory (relative to config file).
    pub path: String,
    /// Build command executed before benchmarking.
    pub build_cmd: String,
    /// Command to start the API server.
    pub run_cmd: String,
    /// Port the API server listens on.
    pub port: u16,
    /// Optional Docker image name (if using container-based deploy).
    pub docker_image: Option<String>,
}

/// Top-level benchmark configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    /// Human-readable name for this benchmark suite.
    pub name: String,
    /// Benchmark version / revision tag.
    pub version: String,
    /// Optional baseline language used for report comparisons.
    #[serde(default)]
    pub baseline_language: Option<String>,
    /// Optional baseline runtime to disambiguate the baseline language.
    #[serde(default)]
    pub baseline_runtime: Option<String>,
    /// Total number of HTTP requests per test case.
    pub total_requests: u64,
    /// List of concurrency levels to sweep.
    pub concurrency_levels: Vec<u32>,
    /// Number of times to repeat each (language, concurrency) pair.
    pub repeat: u32,
    /// Warm-up requests sent before measurement begins.
    pub warmup_requests: u64,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// API endpoint path to benchmark (e.g. "/api/health").
    pub endpoint: String,
    /// HTTP method (GET, POST, ...).
    pub method: String,
    /// Optional request body (for POST/PUT).
    pub body: Option<String>,
    /// Languages / runtimes to benchmark.
    pub languages: Vec<LanguageEntry>,
}

/// One cell in the test matrix: a specific (language, concurrency, repeat) triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    pub language: LanguageEntry,
    pub concurrency: u32,
    pub repeat_index: u32,
}

#[derive(Debug, Clone)]
struct MatrixRng {
    state: u64,
}

impl MatrixRng {
    fn new(seed: u64) -> Self {
        let state = if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        };
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
}

fn shuffle_cases(cases: &mut [TestCase], seed: u64) {
    if cases.len() < 2 {
        return;
    }
    let mut rng = MatrixRng::new(seed ^ cases.len() as u64);
    for idx in (1..cases.len()).rev() {
        let swap_idx = (rng.next_u64() as usize) % (idx + 1);
        cases.swap(idx, swap_idx);
    }
}

impl BenchmarkConfig {
    /// Load config from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate invariants.
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.languages.is_empty(),
            "languages list must not be empty"
        );
        anyhow::ensure!(
            !self.concurrency_levels.is_empty(),
            "concurrency_levels must not be empty"
        );
        anyhow::ensure!(self.total_requests > 0, "total_requests must be > 0");
        anyhow::ensure!(self.repeat > 0, "repeat must be > 0");
        for lang in &self.languages {
            anyhow::ensure!(!lang.name.is_empty(), "language name must not be empty");
            anyhow::ensure!(lang.port > 0, "port must be > 0 for {}", lang.name);
        }
        if let Some(baseline_language) = &self.baseline_language {
            let baseline = self
                .languages
                .iter()
                .find(|lang| lang.name.eq_ignore_ascii_case(baseline_language))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "baseline_language '{}' does not match any configured language",
                        baseline_language
                    )
                })?;

            if let Some(baseline_runtime) = &self.baseline_runtime {
                anyhow::ensure!(
                    baseline.runtime.eq_ignore_ascii_case(baseline_runtime),
                    "baseline_runtime '{}' does not match runtime '{}' for baseline language '{}'",
                    baseline_runtime,
                    baseline.runtime,
                    baseline_language
                );
            }
        } else if self.baseline_runtime.is_some() {
            anyhow::bail!("baseline_runtime requires baseline_language");
        }
        Ok(())
    }

    /// Expand the config into a flat list of test cases.
    pub fn test_matrix(&self) -> Vec<TestCase> {
        self.test_matrix_with_seed(None)
    }

    /// Expand the config into a flat list of test cases, optionally shuffled with a deterministic seed.
    pub fn test_matrix_with_seed(&self, seed: Option<u64>) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for lang in &self.languages {
            for &conc in &self.concurrency_levels {
                for rep in 0..self.repeat {
                    cases.push(TestCase {
                        language: lang.clone(),
                        concurrency: conc,
                        repeat_index: rep,
                    });
                }
            }
        }
        if let Some(seed) = seed {
            shuffle_cases(&mut cases, seed);
        }
        cases
    }

    /// Apply --quick overrides: 1000 requests, concurrency [1, 10], repeat 1.
    pub fn apply_quick(&mut self) {
        self.total_requests = 1000;
        self.concurrency_levels = vec![1, 10];
        self.repeat = 1;
        self.warmup_requests = 50;
    }

    /// Filter languages to only those whose names are in the given list.
    pub fn filter_languages(&mut self, names: &[String]) {
        self.languages
            .retain(|l| names.iter().any(|n| n.eq_ignore_ascii_case(&l.name)));
    }

    /// Return a list of all available languages with their runtimes.
    #[allow(dead_code)]
    pub fn language_summary(&self) -> Vec<(String, String)> {
        self.languages
            .iter()
            .map(|l| (l.name.clone(), l.runtime.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> BenchmarkConfig {
        BenchmarkConfig {
            name: "test".into(),
            version: "0.1.0".into(),
            baseline_language: Some("rust".into()),
            baseline_runtime: Some("axum".into()),
            total_requests: 10_000,
            concurrency_levels: vec![1, 10, 100],
            repeat: 2,
            warmup_requests: 100,
            timeout_secs: 30,
            endpoint: "/api/health".into(),
            method: "GET".into(),
            body: None,
            languages: vec![
                LanguageEntry {
                    name: "rust".into(),
                    runtime: "axum".into(),
                    path: "implementations/rust-axum".into(),
                    build_cmd: "cargo build --release".into(),
                    run_cmd: "./target/release/api".into(),
                    port: 3001,
                    docker_image: None,
                },
                LanguageEntry {
                    name: "go".into(),
                    runtime: "gin".into(),
                    path: "implementations/go-gin".into(),
                    build_cmd: "go build -o api .".into(),
                    run_cmd: "./api".into(),
                    port: 3002,
                    docker_image: None,
                },
            ],
        }
    }

    #[test]
    fn test_matrix_length() {
        let cfg = sample_config();
        let matrix = cfg.test_matrix();
        // 2 languages * 3 concurrency * 2 repeats = 12
        assert_eq!(matrix.len(), 12);
    }

    #[test]
    fn test_matrix_covers_all_combinations() {
        let cfg = sample_config();
        let matrix = cfg.test_matrix();
        // Check that each language appears with each concurrency level
        for lang in &cfg.languages {
            for &conc in &cfg.concurrency_levels {
                let count = matrix
                    .iter()
                    .filter(|tc| tc.language.name == lang.name && tc.concurrency == conc)
                    .count();
                assert_eq!(count, cfg.repeat as usize);
            }
        }
    }

    #[test]
    fn test_matrix_with_seed_is_deterministic() {
        let cfg = sample_config();
        let first = cfg.test_matrix_with_seed(Some(42));
        let second = cfg.test_matrix_with_seed(Some(42));
        assert_eq!(first, second);
    }

    #[test]
    fn test_matrix_with_seed_changes_case_order() {
        let cfg = sample_config();
        let original = cfg.test_matrix();
        let randomized = cfg.test_matrix_with_seed(Some(42));

        assert_eq!(original.len(), randomized.len());
        assert_ne!(original, randomized);
        for case in original {
            assert!(randomized.contains(&case));
        }
    }

    #[test]
    fn test_quick_override() {
        let mut cfg = sample_config();
        cfg.apply_quick();
        assert_eq!(cfg.total_requests, 1000);
        assert_eq!(cfg.concurrency_levels, vec![1, 10]);
        assert_eq!(cfg.repeat, 1);
        assert_eq!(cfg.warmup_requests, 50);
    }

    #[test]
    fn test_filter_languages() {
        let mut cfg = sample_config();
        cfg.filter_languages(&["rust".to_string()]);
        assert_eq!(cfg.languages.len(), 1);
        assert_eq!(cfg.languages[0].name, "rust");
    }

    #[test]
    fn test_filter_languages_case_insensitive() {
        let mut cfg = sample_config();
        cfg.filter_languages(&["GO".to_string()]);
        assert_eq!(cfg.languages.len(), 1);
        assert_eq!(cfg.languages[0].name, "go");
    }

    #[test]
    fn test_validate_rejects_unknown_baseline_language() {
        let mut cfg = sample_config();
        cfg.baseline_language = Some("node".into());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_baseline_runtime_without_language() {
        let mut cfg = sample_config();
        cfg.baseline_language = None;
        cfg.baseline_runtime = Some("axum".into());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_empty_languages() {
        let mut cfg = sample_config();
        cfg.languages.clear();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_requests() {
        let mut cfg = sample_config();
        cfg.total_requests = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_language_summary() {
        let cfg = sample_config();
        let summary = cfg.language_summary();
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0], ("rust".into(), "axum".into()));
        assert_eq!(summary[1], ("go".into(), "gin".into()));
    }

    #[test]
    fn test_load_from_json() {
        let dir = std::env::temp_dir();
        let path = dir.join("alethabench-test-config.json");
        let cfg = sample_config();
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let loaded = BenchmarkConfig::load(&path).unwrap();
        assert_eq!(loaded.name, cfg.name);
        assert_eq!(loaded.languages.len(), cfg.languages.len());
        std::fs::remove_file(&path).ok();
    }

    // --- Dashboard config tests ---

    fn sample_dashboard_config() -> DashboardBenchmarkConfig {
        DashboardBenchmarkConfig {
            config_id: "test-uuid-1234".into(),
            testbeds: vec![TestbedConfig {
                testbed_id: "testbed-uuid-5678".into(),
                cloud: "azure".into(),
                region: "eastus".into(),
                topology: "loopback".into(),
                vm_size: "Standard_D2s_v3".into(),
                existing_vm_ip: Some("40.87.23.80".into()),
                os: "linux".into(),
                languages: vec!["rust".into(), "go".into()],
            }],
            methodology: MethodologyConfig {
                warmup_runs: 10,
                min_measured: 50,
                max_measured: 200,
                target_relative_error: 0.05,
                confidence_level: 0.95,
                modes: vec!["http1".into(), "http2".into()],
                timeout_secs: 30,
            },
            auto_teardown: true,
        }
    }

    #[test]
    fn test_dashboard_config_roundtrip() {
        let cfg = sample_dashboard_config();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let loaded: DashboardBenchmarkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.config_id, "test-uuid-1234");
        assert_eq!(loaded.testbeds.len(), 1);
        assert_eq!(loaded.testbeds[0].languages.len(), 2);
        assert_eq!(loaded.methodology.warmup_runs, 10);
        assert!(loaded.auto_teardown);
    }

    #[test]
    fn test_dashboard_config_load_from_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("alethabench-dashboard-test-config.json");
        let cfg = sample_dashboard_config();
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let loaded = DashboardBenchmarkConfig::load(&path).unwrap();
        assert_eq!(loaded.config_id, cfg.config_id);
        assert_eq!(loaded.testbeds.len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_dashboard_config_validates_empty_config_id() {
        let mut cfg = sample_dashboard_config();
        cfg.config_id = String::new();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_dashboard_config_validates_empty_cells() {
        let mut cfg = sample_dashboard_config();
        cfg.testbeds.clear();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_dashboard_config_validates_empty_languages() {
        let mut cfg = sample_dashboard_config();
        cfg.testbeds[0].languages.clear();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_dashboard_config_defaults() {
        let json = r#"{
            "config_id": "test",
            "testbeds": [{
                "testbed_id": "tb1",
                "cloud": "azure",
                "region": "eastus",
                "vm_size": "Standard_D2s_v3",
                "languages": ["rust"]
            }],
            "methodology": {}
        }"#;
        let cfg: DashboardBenchmarkConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.methodology.warmup_runs, 10);
        assert_eq!(cfg.methodology.min_measured, 50);
        assert_eq!(cfg.methodology.max_measured, 200);
        assert_eq!(cfg.methodology.timeout_secs, 30);
        assert_eq!(cfg.methodology.modes, vec!["http1", "http2"]);
        assert!(!cfg.auto_teardown);
        assert_eq!(cfg.testbeds[0].topology, "loopback");
        assert!(cfg.testbeds[0].existing_vm_ip.is_none());
    }
}
