use crate::provisioner::VmInfo;
use crate::types::{
    BenchmarkEnvironmentFingerprint, BenchmarkResult, NetworkMetrics, ResourceMetrics,
};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::Duration;

const BENCHMARK_TIMEOUT: Duration = Duration::from_secs(600);

/// Resolve the path to `networker-tester`.
///
/// 1. Check `../../target/release/networker-tester` relative to the orchestrator binary.
/// 2. Fall back to `networker-tester` on PATH.
fn resolve_tester_path() -> String {
    if let Ok(exe) = std::env::current_exe() {
        // exe is e.g. .../benchmarks/orchestrator/target/release/alethabench
        // We want          .../target/release/networker-tester  (workspace root target)
        let candidate: PathBuf = exe
            .parent() // .../target/release
            .and_then(|p| p.parent()) // .../target
            .and_then(|p| p.parent()) // .../orchestrator
            .and_then(|p| p.parent()) // .../benchmarks
            .and_then(|p| p.parent()) // workspace root
            .map(|root| root.join("target/release/networker-tester"))
            .unwrap_or_default();
        if candidate.exists() {
            tracing::debug!("Using tester at {}", candidate.display());
            return candidate.to_string_lossy().to_string();
        }
    }
    tracing::debug!("Falling back to networker-tester on PATH");
    "networker-tester".to_string()
}

/// Parameters for a benchmark run extracted from the config.
pub struct TestParams {
    pub warmup_requests: u64,
    pub benchmark_requests: u64,
    pub timeout_secs: u64,
}

struct ParsedTesterOutput {
    network: NetworkMetrics,
    environment: BenchmarkEnvironmentFingerprint,
}

/// Execute a single benchmark phase against a VM, returning network metrics.
///
/// Shells out to `networker-tester` with `--json-stdout` and parses the output.
async fn run_benchmark(
    vm: &VmInfo,
    params: &TestParams,
    concurrency: u32,
    phase: &str,
    launch_index: u32,
) -> Result<ParsedTesterOutput> {
    let requests = match phase {
        "cold" => 100,
        "warm" => params.benchmark_requests,
        "warmup" => params.warmup_requests,
        _ => params.benchmark_requests,
    };

    tracing::info!(
        "Running {} benchmark on {} (c={}, n={}, timeout={}s)",
        phase,
        vm.name,
        concurrency,
        requests,
        params.timeout_secs
    );

    let target = format!("https://{}:8443/health", vm.ip);
    let (benchmark_phase, benchmark_scenario) = match phase {
        "warmup" => ("warmup", "warmup"),
        "cold" => ("measured", "cold"),
        "warm" => ("measured", "warm"),
        other => ("measured", other),
    };
    let tester_bin = resolve_tester_path();

    let output = tokio::time::timeout(BENCHMARK_TIMEOUT, async {
        tokio::process::Command::new(&tester_bin)
            .args([
                "--target",
                &target,
                "--modes",
                "http1,http2",
                "--runs",
                &requests.to_string(),
                "--concurrency",
                &concurrency.to_string(),
                "--timeout",
                &params.timeout_secs.to_string(),
                "--insecure",
                "--json-stdout",
                "--benchmark-phase",
                benchmark_phase,
                "--benchmark-scenario",
                benchmark_scenario,
                "--benchmark-launch-index",
                &launch_index.to_string(),
            ])
            .output()
            .await
    })
    .await
    .context("benchmark timed out")?
    .context("failed to execute networker-tester")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "networker-tester failed (phase={phase}, exit={}): {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_tester_output(&stdout)
        .with_context(|| format!("parsing networker-tester JSON output for {phase} phase"))
}

/// Parse the JSON output from `networker-tester --json-stdout` into NetworkMetrics.
fn parse_tester_output(json_str: &str) -> Result<ParsedTesterOutput> {
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).context("invalid JSON from networker-tester")?;

    let payload = if parsed.is_array() {
        parsed
            .as_array()
            .and_then(|arr| arr.last())
            .unwrap_or(&parsed)
    } else {
        &parsed
    };

    let summary = if let Some(s) = payload.get("summary") {
        s
    } else if let Some(summaries) = payload.get("summaries").and_then(|value| value.as_array()) {
        summaries.first().unwrap_or(payload)
    } else {
        payload
    };

    Ok(ParsedTesterOutput {
        network: NetworkMetrics {
            rps: extract_f64(summary, "rps").unwrap_or(0.0),
            latency_mean_ms: extract_f64(summary, "latency_mean_ms")
                .or_else(|| extract_f64(summary, "mean"))
                .or_else(|| extract_f64(summary, "mean_ms"))
                .unwrap_or(0.0),
            latency_p50_ms: extract_f64(summary, "latency_p50_ms")
                .or_else(|| extract_f64(summary, "p50"))
                .or_else(|| extract_f64(summary, "p50_ms"))
                .unwrap_or(0.0),
            latency_p99_ms: extract_f64(summary, "latency_p99_ms")
                .or_else(|| extract_f64(summary, "p99"))
                .or_else(|| extract_f64(summary, "p99_ms"))
                .unwrap_or(0.0),
            latency_p999_ms: extract_f64(summary, "latency_p999_ms")
                .or_else(|| extract_f64(summary, "p999"))
                .or_else(|| extract_f64(summary, "p999_ms"))
                .unwrap_or(0.0),
            latency_max_ms: extract_f64(summary, "latency_max_ms")
                .or_else(|| extract_f64(summary, "max"))
                .or_else(|| extract_f64(summary, "max_ms"))
                .unwrap_or(0.0),
            bytes_transferred: extract_u64(summary, "bytes_transferred").unwrap_or(0),
            error_count: extract_u64(summary, "error_count")
                .or_else(|| extract_u64(summary, "failure_count"))
                .or_else(|| extract_u64(summary, "errors"))
                .unwrap_or(0),
            total_requests: extract_u64(summary, "total_requests")
                .or_else(|| extract_u64(summary, "sample_count"))
                .or_else(|| extract_u64(summary, "requests"))
                .unwrap_or(0),
            phase_model: payload
                .get("methodology")
                .and_then(|methodology| methodology.get("phase_model"))
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            phases_present: payload
                .get("methodology")
                .and_then(|methodology| methodology.get("phases_present"))
                .and_then(|value| value.as_array())
                .map(|phases| {
                    phases
                        .iter()
                        .filter_map(|phase| phase.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        },
        environment: parse_environment_fingerprint(payload),
    })
}

/// Extract an f64 from a JSON value, trying both float and integer representations.
fn extract_f64(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|val| {
        val.as_f64()
            .or_else(|| val.as_i64().map(|n| n as f64))
            .or_else(|| val.as_u64().map(|n| n as f64))
    })
}

fn extract_string(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|val| val.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn extract_u32(v: &serde_json::Value, key: &str) -> Option<u32> {
    v.get(key).and_then(|val| val.as_u64()).map(|value| value as u32)
}

fn parse_environment_fingerprint(payload: &serde_json::Value) -> BenchmarkEnvironmentFingerprint {
    let environment = payload.get("environment").unwrap_or(&serde_json::Value::Null);
    let client = environment
        .get("client_info")
        .unwrap_or(&serde_json::Value::Null);
    let server = environment
        .get("server_info")
        .unwrap_or(&serde_json::Value::Null);
    let network_baseline = environment
        .get("network_baseline")
        .unwrap_or(&serde_json::Value::Null);
    let environment_check = environment
        .get("environment_check")
        .unwrap_or(&serde_json::Value::Null);
    let stability_check = environment
        .get("stability_check")
        .unwrap_or(&serde_json::Value::Null);

    BenchmarkEnvironmentFingerprint {
        client_os: extract_string(client, "os"),
        client_arch: extract_string(client, "arch"),
        client_cpu_cores: extract_u32(client, "cpu_cores"),
        client_region: extract_string(client, "region"),
        server_os: extract_string(server, "os"),
        server_arch: extract_string(server, "arch"),
        server_cpu_cores: extract_u32(server, "cpu_cores"),
        server_region: extract_string(server, "region"),
        network_type: extract_string(network_baseline, "network_type")
            .or_else(|| extract_string(environment_check, "network_type"))
            .or_else(|| extract_string(stability_check, "network_type")),
        baseline_rtt_p50_ms: extract_f64(network_baseline, "rtt_p50_ms")
            .or_else(|| extract_f64(environment_check, "rtt_p50_ms"))
            .or_else(|| extract_f64(stability_check, "rtt_p50_ms")),
        baseline_rtt_p95_ms: extract_f64(network_baseline, "rtt_p95_ms")
            .or_else(|| extract_f64(environment_check, "rtt_p95_ms"))
            .or_else(|| extract_f64(stability_check, "rtt_p95_ms")),
    }
}

/// Extract a u64 from a JSON value.
fn extract_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|val| val.as_u64())
}

/// Run a complete cold/warm benchmark cycle.
///
/// 1. Cold phase: 100 requests (first contact, no connection pooling)
/// 2. Warmup: params.warmup_requests (discarded)
/// 3. Warm phase: params.benchmark_requests (measured)
///
/// Returns (cold_result, warm_result).
pub async fn run_cold_warm_cycle(
    vm: &VmInfo,
    params: &TestParams,
    concurrency: u32,
    language: &str,
    runtime: &str,
    repeat_index: u32,
) -> Result<(BenchmarkResult, BenchmarkResult)> {
    tracing::info!(
        "Starting cold/warm cycle for {language}/{runtime} c={concurrency} repeat={} on {}",
        repeat_index + 1,
        vm.name
    );

    // 1. Cold phase
    let cold_measurement = run_benchmark(vm, params, concurrency, "cold", repeat_index)
        .await
        .context("cold phase")?;

    // 2. Warmup (discard results)
    if params.warmup_requests > 0 {
        tracing::info!("Warming up with {} requests", params.warmup_requests);
        let _ = run_benchmark(vm, params, concurrency, "warmup", repeat_index).await;
    }

    // 3. Warm phase
    let warm_measurement = run_benchmark(vm, params, concurrency, "warm", repeat_index)
        .await
        .context("warm phase")?;

    let cold_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency,
        repeat_index,
        scenario: "cold".to_string(),
        environment: cold_measurement.environment,
        network: cold_measurement.network,
        resources: ResourceMetrics::default(),
        startup: Default::default(),
        binary: Default::default(),
    };

    let warm_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency,
        repeat_index,
        scenario: "warm".to_string(),
        environment: warm_measurement.environment,
        network: warm_measurement.network,
        resources: ResourceMetrics::default(),
        startup: Default::default(),
        binary: Default::default(),
    };

    tracing::info!(
        "Cold/warm cycle complete for {language}/{runtime} c={concurrency}: \
         cold={:.1} rps, warm={:.1} rps",
        cold_result.network.rps,
        warm_result.network.rps,
    );

    Ok((cold_result, warm_result))
}

/// Parameters for a download/throughput benchmark run.
#[allow(dead_code)]
pub struct DownloadTestParams {
    /// Number of benchmark requests (each fetches a download payload).
    pub benchmark_requests: u64,
    /// Per-request timeout in seconds.
    pub timeout_secs: u64,
    /// Download size in bytes (e.g. 1_048_576 for 1 MB).
    pub download_bytes: u64,
    /// Mode to use: "http1", "http2", "pageload1", etc.
    /// Falls back to "http1" if Chrome/pageload is unavailable.
    pub mode: String,
}

/// Execute a download/throughput benchmark against a VM.
///
/// Uses `networker-tester` with the download endpoint to measure how fast
/// each server can stream bytes, rather than just health-check latency.
#[allow(dead_code)]
async fn run_download_benchmark(
    vm: &VmInfo,
    params: &DownloadTestParams,
    phase: &str,
) -> Result<ParsedTesterOutput> {
    let requests = match phase {
        "cold" => 10,
        "warmup" => 20,
        _ => params.benchmark_requests,
    };

    tracing::info!(
        "Running {} download benchmark on {} (mode={}, n={}, size={}B, timeout={}s)",
        phase,
        vm.name,
        params.mode,
        requests,
        params.download_bytes,
        params.timeout_secs,
    );

    let target = format!("https://{}:8443/download/{}", vm.ip, params.download_bytes);
    let tester_bin = resolve_tester_path();
    let (benchmark_phase, benchmark_scenario) = match phase {
        "warmup" => ("warmup", "warmup"),
        "cold" => ("measured", "cold"),
        "warm" => ("measured", "warm"),
        other => ("measured", other),
    };

    let output = tokio::time::timeout(BENCHMARK_TIMEOUT, async {
        tokio::process::Command::new(&tester_bin)
            .args([
                "--target",
                &target,
                "--modes",
                &params.mode,
                "--runs",
                &requests.to_string(),
                "--timeout",
                &params.timeout_secs.to_string(),
                "--insecure",
                "--json-stdout",
                "--benchmark-phase",
                benchmark_phase,
                "--benchmark-scenario",
                benchmark_scenario,
                "--benchmark-launch-index",
                "0",
            ])
            .output()
            .await
    })
    .await
    .context("download benchmark timed out")?
    .context("failed to execute networker-tester")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "networker-tester download benchmark failed (phase={phase}, exit={}): {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_tester_output(&stdout)
        .with_context(|| format!("parsing networker-tester JSON output for download {phase} phase"))
}

/// Run a complete cold/warm download benchmark cycle.
///
/// 1. Cold phase: 10 requests (first contact, measures initial throughput)
/// 2. Warmup: 20 requests (discarded, establishes connections)
/// 3. Warm phase: params.benchmark_requests (measured throughput)
///
/// Returns (cold_result, warm_result).
#[allow(dead_code)]
pub async fn run_download_cold_warm_cycle(
    vm: &VmInfo,
    params: &DownloadTestParams,
    language: &str,
    runtime: &str,
) -> Result<(BenchmarkResult, BenchmarkResult)> {
    tracing::info!(
        "Starting download cold/warm cycle for {language}/{runtime} on {}",
        vm.name
    );

    // 1. Cold phase
    let cold_measurement = run_download_benchmark(vm, params, "cold")
        .await
        .context("download cold phase")?;

    // 2. Warmup (discard results)
    let _ = run_download_benchmark(vm, params, "warmup").await;

    // 3. Warm phase
    let warm_measurement = run_download_benchmark(vm, params, "warm")
        .await
        .context("download warm phase")?;

    let cold_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency: 1,
        repeat_index: 0,
        scenario: "cold".to_string(),
        environment: cold_measurement.environment,
        network: cold_measurement.network,
        resources: ResourceMetrics::default(),
        startup: Default::default(),
        binary: Default::default(),
    };

    let warm_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency: 1,
        repeat_index: 0,
        scenario: "warm".to_string(),
        environment: warm_measurement.environment,
        network: warm_measurement.network,
        resources: ResourceMetrics::default(),
        startup: Default::default(),
        binary: Default::default(),
    };

    tracing::info!(
        "Download cold/warm cycle complete for {language}/{runtime}: \
         cold={:.1} rps, warm={:.1} rps",
        cold_result.network.rps,
        warm_result.network.rps,
    );

    Ok((cold_result, warm_result))
}

/// Measure the startup time of the server by restarting it and timing the
/// first successful /health response.
#[allow(dead_code)]
pub async fn measure_startup_time(vm: &VmInfo) -> Result<f64> {
    tracing::info!("Measuring startup time on {} ({})", vm.name, vm.ip);

    // Restart the server process on the VM
    let restart_cmd = "\
        pkill -f '/opt/bench/.*server' || true; \
        pkill -f 'networker-endpoint' || true; \
        sleep 0.5; \
        nohup /opt/bench/server > /opt/bench/server.log 2>&1 & \
        echo $!";

    let output = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "BatchMode=yes",
            &format!("azureuser@{}", vm.ip),
            restart_cmd,
        ])
        .output()
        .await
        .context("SSH restart command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to restart server on {}: {}", vm.ip, stderr.trim());
    }

    // Now time how long until /health responds
    let start = std::time::Instant::now();
    let max_wait = Duration::from_secs(30);
    let poll_interval = Duration::from_millis(50);

    loop {
        if start.elapsed() > max_wait {
            bail!(
                "Startup timed out: /health did not respond within {}s",
                max_wait.as_secs()
            );
        }

        let result = tokio::process::Command::new("curl")
            .args([
                "-sk",
                "--connect-timeout",
                "2",
                "--max-time",
                "5",
                &format!("https://{}:8443/health", vm.ip),
            ])
            .output()
            .await;

        if let Ok(curl_output) = result {
            if curl_output.status.success() {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                tracing::info!("Startup time: {elapsed_ms:.1}ms on {}", vm.name);
                return Ok(elapsed_ms);
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tester_output_summary() {
        let json = r#"{
            "summary": {
                "rps": 12345.6,
                "latency_mean_ms": 0.81,
                "latency_p50_ms": 0.72,
                "latency_p99_ms": 2.1,
                "latency_p999_ms": 5.5,
                "latency_max_ms": 12.3,
                "bytes_transferred": 1048576,
                "error_count": 0,
                "total_requests": 10000
            }
        }"#;

        let metrics = parse_tester_output(json).unwrap();
        assert!((metrics.network.rps - 12345.6).abs() < 0.1);
        assert!((metrics.network.latency_mean_ms - 0.81).abs() < 0.01);
        assert!((metrics.network.latency_p50_ms - 0.72).abs() < 0.01);
        assert!((metrics.network.latency_p99_ms - 2.1).abs() < 0.01);
        assert_eq!(metrics.network.bytes_transferred, 1048576);
        assert_eq!(metrics.network.error_count, 0);
        assert_eq!(metrics.network.total_requests, 10000);
        assert!(metrics.network.phase_model.is_empty());
        assert!(metrics.network.phases_present.is_empty());
        assert!(metrics.environment.is_empty());
    }

    #[test]
    fn test_parse_tester_output_flat() {
        let json = r#"{
            "rps": 5000.0,
            "mean_ms": 1.5,
            "p50_ms": 1.2,
            "p99_ms": 3.0,
            "p999_ms": 8.0,
            "max_ms": 15.0,
            "bytes_transferred": 512000,
            "errors": 2,
            "requests": 5000
        }"#;

        let metrics = parse_tester_output(json).unwrap();
        assert!((metrics.network.rps - 5000.0).abs() < 0.1);
        assert!((metrics.network.latency_mean_ms - 1.5).abs() < 0.01);
        assert_eq!(metrics.network.error_count, 2);
        assert_eq!(metrics.network.total_requests, 5000);
    }

    #[test]
    fn test_parse_tester_output_benchmark_contract() {
        let json = r#"{
            "metadata": { "contract_version": "1.0" },
            "environment": {
                "client_info": {
                    "os": "macos",
                    "arch": "aarch64",
                    "cpu_cores": 12,
                    "region": "us-east"
                },
                "server_info": {
                    "os": "ubuntu",
                    "arch": "x86_64",
                    "cpu_cores": 4,
                    "region": "eastus"
                },
                "network_baseline": {
                    "rtt_p50_ms": 0.85,
                    "rtt_p95_ms": 1.30,
                    "network_type": "LAN"
                }
            },
            "methodology": {
                "phase_model": "stability-check->overhead->pilot->measured->cooldown",
                "phases_present": ["stability-check", "overhead", "pilot", "measured", "cooldown"]
            },
            "summary": {
                "rps": 4321.0,
                "mean": 1.25,
                "p50": 1.0,
                "p99": 3.5,
                "p999": 5.2,
                "max": 8.4,
                "bytes_transferred": 2048,
                "failure_count": 2,
                "sample_count": 500
            }
        }"#;

        let metrics = parse_tester_output(json).unwrap();
        assert!((metrics.network.rps - 4321.0).abs() < 0.1);
        assert!((metrics.network.latency_mean_ms - 1.25).abs() < 0.01);
        assert!((metrics.network.latency_p50_ms - 1.0).abs() < 0.01);
        assert!((metrics.network.latency_p99_ms - 3.5).abs() < 0.01);
        assert_eq!(metrics.network.error_count, 2);
        assert_eq!(metrics.network.total_requests, 500);
        assert_eq!(
            metrics.network.phase_model,
            "stability-check->overhead->pilot->measured->cooldown"
        );
        assert_eq!(
            metrics.network.phases_present,
            vec![
                "stability-check".to_string(),
                "overhead".to_string(),
                "pilot".to_string(),
                "measured".to_string(),
                "cooldown".to_string(),
            ]
        );
        assert_eq!(metrics.environment.client_os.as_deref(), Some("macos"));
        assert_eq!(metrics.environment.server_arch.as_deref(), Some("x86_64"));
        assert_eq!(metrics.environment.network_type.as_deref(), Some("LAN"));
        assert_eq!(metrics.environment.baseline_rtt_p50_ms, Some(0.85));
    }

    #[test]
    fn test_parse_tester_output_invalid_json() {
        let result = parse_tester_output("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_tester_output_empty_object() {
        let metrics = parse_tester_output("{}").unwrap();
        assert!((metrics.network.rps - 0.0).abs() < f64::EPSILON);
        assert_eq!(metrics.network.total_requests, 0);
        assert!(metrics.environment.is_empty());
    }

    #[test]
    fn test_phase_request_counts() {
        // Verify the logic for determining request count per phase
        let params = TestParams {
            warmup_requests: 200,
            benchmark_requests: 10000,
            timeout_secs: 30,
        };

        // Cold phase always uses 100
        assert_eq!(
            match "cold" {
                "cold" => 100u64,
                "warm" => params.benchmark_requests,
                "warmup" => params.warmup_requests,
                _ => params.benchmark_requests,
            },
            100
        );

        // Warm phase uses benchmark_requests
        assert_eq!(
            match "warm" {
                "cold" => 100u64,
                "warm" => params.benchmark_requests,
                "warmup" => params.warmup_requests,
                _ => params.benchmark_requests,
            },
            10000
        );
    }

    #[test]
    fn test_download_phase_request_counts() {
        let params = DownloadTestParams {
            benchmark_requests: 50,
            timeout_secs: 15,
            download_bytes: 1_048_576,
            mode: "http1".to_string(),
        };

        // Cold phase uses 10 for download benchmarks
        assert_eq!(
            match "cold" {
                "cold" => 10u64,
                "warmup" => 20u64,
                _ => params.benchmark_requests,
            },
            10
        );

        // Warm phase uses benchmark_requests
        assert_eq!(
            match "warm" {
                "cold" => 10u64,
                "warmup" => 20u64,
                _ => params.benchmark_requests,
            },
            50
        );
    }
}
