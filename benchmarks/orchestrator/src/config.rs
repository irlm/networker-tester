use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
}
