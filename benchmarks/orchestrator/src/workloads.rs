//! apibench workload definitions — the measured `/api/*` compute suite.
//!
//! Contract: `benchmarks/shared/API-SPEC.md` §4 (measured endpoints) and §7
//! (canonical requests). The committed workload set lives at
//! `benchmarks/configs/apibench.json`; the same file is embedded into the
//! binary at compile time so the orchestrator can run apibench even when the
//! benchmarks directory isn't shipped alongside it (and so the committed
//! config is parse-validated by `cargo test`).
//!
//! Audit C1 / P0#7: before this module nothing in the product ever measured
//! the `/api/*` workloads — the tester only drove `/health` and
//! `/download/{size}`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The committed workload set, embedded at compile time.
const EMBEDDED_APIBENCH_JSON: &str = include_str!("../../configs/apibench.json");

/// Spec-measured `/api/*` endpoints (API-SPEC.md §4). A workload whose path
/// does not target one of these is rejected — apibench must never quietly
/// measure an infrastructure endpoint like `/health` (audit F4).
const MEASURED_API_PREFIXES: &[&str] = &[
    "/api/users",
    "/api/transform",
    "/api/aggregate",
    "/api/search",
    "/api/upload/process",
];

/// Mode keyword that selects the apibench workload suite in
/// `methodology.modes`. It is an orchestrator-level mode: it must be stripped
/// before building `networker-tester --modes` args (the tester has no such
/// protocol and would silently run nothing).
pub const APIBENCH_MODE: &str = "apibench";

/// Languages that do not implement the `/api/*` suite (API-SPEC.md §9).
pub fn language_supports_apibench(language: &str) -> bool {
    language != "nginx"
}

/// One measured API workload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiWorkload {
    /// Short identifier, e.g. "api-users". Used in artifact labels and
    /// result file names — must be slug-safe.
    pub name: String,
    /// HTTP method: GET or POST.
    pub method: String,
    /// Path + query, e.g. "/api/search?q=network&limit=10".
    pub path: String,
    /// Request body (POST only). Committed literal — deterministic.
    #[serde(default)]
    pub body: Option<String>,
    /// Content-Type for the body (default application/json).
    #[serde(default)]
    pub content_type: Option<String>,
    /// Human-readable description (informational).
    #[serde(default)]
    pub description: Option<String>,
}

/// The full workload set as committed in `benchmarks/configs/apibench.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiWorkloadSet {
    #[serde(rename = "_version")]
    pub version: u32,
    pub workloads: Vec<ApiWorkload>,
}

impl ApiWorkloadSet {
    /// Parse and validate a workload set from a JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        let set: Self = serde_json::from_str(json).context("parsing apibench workload set")?;
        set.validate()?;
        Ok(set)
    }

    /// Load the workload set: `<bench_dir>/configs/apibench.json` if present,
    /// otherwise the compile-time embedded copy of the same file.
    pub fn load_or_embedded(bench_dir: &Path) -> Result<Self> {
        let path = bench_dir.join("configs/apibench.json");
        if path.is_file() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            return Self::from_json(&content)
                .with_context(|| format!("loading {}", path.display()));
        }
        tracing::debug!(
            "apibench config not found at {} — using embedded copy",
            path.display()
        );
        Self::from_json(EMBEDDED_APIBENCH_JSON).context("parsing embedded apibench config")
    }

    /// Validate invariants: unique slug-safe names, GET/POST only, POST ⇔
    /// body, and every path targets a spec-measured `/api/*` endpoint.
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.workloads.is_empty(),
            "apibench workload set must not be empty"
        );
        let mut seen = std::collections::HashSet::new();
        for w in &self.workloads {
            anyhow::ensure!(
                !w.name.is_empty()
                    && w.name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "workload name {:?} must be a non-empty [A-Za-z0-9_-] slug",
                w.name
            );
            anyhow::ensure!(seen.insert(w.name.clone()), "duplicate workload {}", w.name);
            anyhow::ensure!(
                matches!(w.method.as_str(), "GET" | "POST"),
                "workload {}: method must be GET or POST, got {:?}",
                w.name,
                w.method
            );
            anyhow::ensure!(
                MEASURED_API_PREFIXES.iter().any(|p| {
                    w.path == *p
                        || w.path.starts_with(&format!("{p}?"))
                        || w.path.starts_with(&format!("{p}/"))
                }),
                "workload {}: path {:?} is not a spec-measured /api/* endpoint \
                 (API-SPEC.md §4); allowed: {}",
                w.name,
                w.path,
                MEASURED_API_PREFIXES.join(", ")
            );
            match w.method.as_str() {
                "POST" => anyhow::ensure!(
                    w.body.as_ref().is_some_and(|b| !b.is_empty()),
                    "workload {}: POST requires a non-empty body",
                    w.name
                ),
                _ => anyhow::ensure!(
                    w.body.is_none(),
                    "workload {}: GET must not carry a body",
                    w.name
                ),
            }
        }
        Ok(())
    }
}

/// Build the `networker-tester` argument list for one apibench workload.
///
/// The workload is driven over HTTP/1.1 (the tester's request-body support is
/// http1/http2-only by design); every language gets the identical request
/// shape, so comparisons stay apples-to-apples. The body is passed via
/// `--request-body` and loaded by the tester once at startup — no
/// per-request allocation in the timed region.
pub fn tester_args_for_workload(
    base_url: &str,
    workload: &ApiWorkload,
    runs: u64,
    timeout_secs: u64,
    bearer_token: Option<&str>,
) -> Vec<String> {
    let target = format!("{}{}", base_url.trim_end_matches('/'), workload.path);
    let mut args = vec![
        "--target".to_string(),
        target,
        "--modes".to_string(),
        "http1".to_string(),
        "--runs".to_string(),
        runs.to_string(),
        "--timeout".to_string(),
        timeout_secs.to_string(),
        "--insecure".to_string(),
        "--json-stdout".to_string(),
        "--benchmark-mode".to_string(),
    ];
    if workload.method == "POST" {
        if let Some(body) = &workload.body {
            args.push("--request-body".to_string());
            args.push(body.clone());
            args.push("--request-content-type".to_string());
            args.push(
                workload
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/json".to_string()),
            );
        }
    }
    if let Some(token) = bearer_token {
        if !token.is_empty() {
            args.push("--bearer-token".to_string());
            args.push(token.to_string());
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed benchmarks/configs/apibench.json must parse and pass
    /// validation — this is the config-validation gate for the workload set.
    #[test]
    fn embedded_config_parses_and_validates() {
        let set = ApiWorkloadSet::from_json(EMBEDDED_APIBENCH_JSON)
            .expect("committed apibench.json must parse and validate");
        assert_eq!(set.version, 1);
        let names: Vec<&str> = set.workloads.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "api-users",
                "api-transform",
                "api-aggregate",
                "api-search",
                "api-compress"
            ]
        );
    }

    /// The canonical transform body must be exactly transform_inputs[0] from
    /// the frozen dataset (API-SPEC.md §7, transform_input0).
    #[test]
    fn embedded_transform_body_is_canonical() {
        let set = ApiWorkloadSet::from_json(EMBEDDED_APIBENCH_JSON).unwrap();
        let transform = set
            .workloads
            .iter()
            .find(|w| w.name == "api-transform")
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(transform.body.as_ref().unwrap()).expect("body must be JSON");
        assert_eq!(body["seed"], 1);
        assert_eq!(
            body["fields"],
            serde_json::json!(["throughput", "performance", "latency", "protocol"])
        );
        assert_eq!(body["values"], serde_json::json!([9216, 8962]));
    }

    #[test]
    fn embedded_workloads_only_reference_measured_endpoints() {
        let set = ApiWorkloadSet::from_json(EMBEDDED_APIBENCH_JSON).unwrap();
        for w in &set.workloads {
            assert!(
                MEASURED_API_PREFIXES.iter().any(|p| w.path.starts_with(p)),
                "{} path {} not spec-measured",
                w.name,
                w.path
            );
        }
    }

    #[test]
    fn rejects_infrastructure_endpoint() {
        let json = r#"{"_version":1,"workloads":[
            {"name":"bad-health","method":"GET","path":"/health"}]}"#;
        let err = ApiWorkloadSet::from_json(json).unwrap_err().to_string();
        assert!(err.contains("spec-measured"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_post_without_body() {
        let json = r#"{"_version":1,"workloads":[
            {"name":"t","method":"POST","path":"/api/transform"}]}"#;
        assert!(ApiWorkloadSet::from_json(json).is_err());
    }

    #[test]
    fn rejects_get_with_body() {
        let json = r#"{"_version":1,"workloads":[
            {"name":"t","method":"GET","path":"/api/aggregate","body":"x"}]}"#;
        assert!(ApiWorkloadSet::from_json(json).is_err());
    }

    #[test]
    fn rejects_duplicate_names() {
        let json = r#"{"_version":1,"workloads":[
            {"name":"t","method":"GET","path":"/api/aggregate"},
            {"name":"t","method":"GET","path":"/api/search"}]}"#;
        assert!(ApiWorkloadSet::from_json(json).is_err());
    }

    #[test]
    fn rejects_unknown_method() {
        let json = r#"{"_version":1,"workloads":[
            {"name":"t","method":"DELETE","path":"/api/users"}]}"#;
        assert!(ApiWorkloadSet::from_json(json).is_err());
    }

    #[test]
    fn rejects_prefix_smuggling() {
        // "/api/usersX" must not pass the /api/users prefix check.
        let json = r#"{"_version":1,"workloads":[
            {"name":"t","method":"GET","path":"/api/usersextra"}]}"#;
        assert!(ApiWorkloadSet::from_json(json).is_err());
    }

    #[test]
    fn tester_args_get_workload() {
        let w = ApiWorkload {
            name: "api-search".into(),
            method: "GET".into(),
            path: "/api/search?q=network&limit=10".into(),
            body: None,
            content_type: None,
            description: None,
        };
        let args = tester_args_for_workload("https://1.2.3.4:8443", &w, 50, 30, None);
        assert_eq!(args[0], "--target");
        assert_eq!(args[1], "https://1.2.3.4:8443/api/search?q=network&limit=10");
        assert!(args.contains(&"http1".to_string()));
        assert!(!args.contains(&"--request-body".to_string()));
        assert!(!args.contains(&"--bearer-token".to_string()));
    }

    #[test]
    fn tester_args_post_workload_with_token() {
        let w = ApiWorkload {
            name: "api-transform".into(),
            method: "POST".into(),
            path: "/api/transform".into(),
            body: Some(r#"{"seed":1}"#.into()),
            content_type: Some("application/json".into()),
            description: None,
        };
        let args = tester_args_for_workload("https://1.2.3.4:8443/", &w, 10, 5, Some("tok"));
        // trailing slash on base must not produce a double slash
        assert_eq!(args[1], "https://1.2.3.4:8443/api/transform");
        let body_idx = args.iter().position(|a| a == "--request-body").unwrap();
        assert_eq!(args[body_idx + 1], r#"{"seed":1}"#);
        let ct_idx = args
            .iter()
            .position(|a| a == "--request-content-type")
            .unwrap();
        assert_eq!(args[ct_idx + 1], "application/json");
        let tok_idx = args.iter().position(|a| a == "--bearer-token").unwrap();
        assert_eq!(args[tok_idx + 1], "tok");
    }

    #[test]
    fn nginx_excluded_from_apibench() {
        assert!(!language_supports_apibench("nginx"));
        assert!(language_supports_apibench("rust"));
        assert!(language_supports_apibench("csharp-net10-aot"));
    }

    #[test]
    fn load_or_embedded_falls_back_when_missing() {
        let dir = std::env::temp_dir().join("apibench-no-configs-here");
        std::fs::create_dir_all(&dir).unwrap();
        let set = ApiWorkloadSet::load_or_embedded(&dir).unwrap();
        assert_eq!(set.workloads.len(), 5);
    }
}
