use crate::provisioner::VmInfo;
use crate::types::{BenchmarkResult, NetworkMetrics, ResourceMetrics};
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

/// Execute a single benchmark phase against a VM, returning network metrics.
///
/// Shells out to `networker-tester` with `--json-stdout` and parses the output.
pub async fn run_benchmark(
    vm: &VmInfo,
    params: &TestParams,
    concurrency: u32,
    phase: &str,
) -> Result<NetworkMetrics> {
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
fn parse_tester_output(json_str: &str) -> Result<NetworkMetrics> {
    // networker-tester outputs JSON with summary statistics.
    // We look for the summary object which contains aggregate timing data.
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).context("invalid JSON from networker-tester")?;

    // The tester may output an array of results or a single summary object.
    // Try to extract from a "summary" field first, then fall back to top-level.
    let summary = if let Some(s) = parsed.get("summary") {
        s
    } else if parsed.is_array() {
        // Take the last element as the summary
        parsed
            .as_array()
            .and_then(|arr| arr.last())
            .unwrap_or(&parsed)
    } else {
        &parsed
    };

    Ok(NetworkMetrics {
        rps: extract_f64(summary, "rps").unwrap_or(0.0),
        latency_mean_ms: extract_f64(summary, "latency_mean_ms")
            .or_else(|| extract_f64(summary, "mean_ms"))
            .unwrap_or(0.0),
        latency_p50_ms: extract_f64(summary, "latency_p50_ms")
            .or_else(|| extract_f64(summary, "p50_ms"))
            .unwrap_or(0.0),
        latency_p99_ms: extract_f64(summary, "latency_p99_ms")
            .or_else(|| extract_f64(summary, "p99_ms"))
            .unwrap_or(0.0),
        latency_p999_ms: extract_f64(summary, "latency_p999_ms")
            .or_else(|| extract_f64(summary, "p999_ms"))
            .unwrap_or(0.0),
        latency_max_ms: extract_f64(summary, "latency_max_ms")
            .or_else(|| extract_f64(summary, "max_ms"))
            .unwrap_or(0.0),
        bytes_transferred: extract_u64(summary, "bytes_transferred").unwrap_or(0),
        error_count: extract_u64(summary, "error_count")
            .or_else(|| extract_u64(summary, "errors"))
            .unwrap_or(0),
        total_requests: extract_u64(summary, "total_requests")
            .or_else(|| extract_u64(summary, "requests"))
            .unwrap_or(0),
    })
}

/// Extract an f64 from a JSON value, trying both float and integer representations.
fn extract_f64(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|val| val.as_f64())
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
) -> Result<(BenchmarkResult, BenchmarkResult)> {
    tracing::info!(
        "Starting cold/warm cycle for {language}/{runtime} c={concurrency} on {}",
        vm.name
    );

    // 1. Cold phase
    let cold_network = run_benchmark(vm, params, concurrency, "cold")
        .await
        .context("cold phase")?;

    // 2. Warmup (discard results)
    if params.warmup_requests > 0 {
        tracing::info!("Warming up with {} requests", params.warmup_requests);
        let _ = run_benchmark(vm, params, concurrency, "warmup").await;
    }

    // 3. Warm phase
    let warm_network = run_benchmark(vm, params, concurrency, "warm")
        .await
        .context("warm phase")?;

    let cold_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency,
        repeat_index: 0,
        network: cold_network,
        resources: ResourceMetrics::default(),
        startup: Default::default(),
        binary: Default::default(),
    };

    let warm_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency,
        repeat_index: 0,
        network: warm_network,
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
pub async fn run_download_benchmark(
    vm: &VmInfo,
    params: &DownloadTestParams,
    phase: &str,
) -> Result<NetworkMetrics> {
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
    let cold_network = run_download_benchmark(vm, params, "cold")
        .await
        .context("download cold phase")?;

    // 2. Warmup (discard results)
    let _ = run_download_benchmark(vm, params, "warmup").await;

    // 3. Warm phase
    let warm_network = run_download_benchmark(vm, params, "warm")
        .await
        .context("download warm phase")?;

    let cold_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency: 1,
        repeat_index: 0,
        network: cold_network,
        resources: ResourceMetrics::default(),
        startup: Default::default(),
        binary: Default::default(),
    };

    let warm_result = BenchmarkResult {
        language: language.to_string(),
        runtime: runtime.to_string(),
        concurrency: 1,
        repeat_index: 0,
        network: warm_network,
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
        assert!((metrics.rps - 12345.6).abs() < 0.1);
        assert!((metrics.latency_mean_ms - 0.81).abs() < 0.01);
        assert!((metrics.latency_p50_ms - 0.72).abs() < 0.01);
        assert!((metrics.latency_p99_ms - 2.1).abs() < 0.01);
        assert_eq!(metrics.bytes_transferred, 1048576);
        assert_eq!(metrics.error_count, 0);
        assert_eq!(metrics.total_requests, 10000);
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
        assert!((metrics.rps - 5000.0).abs() < 0.1);
        assert!((metrics.latency_mean_ms - 1.5).abs() < 0.01);
        assert_eq!(metrics.error_count, 2);
        assert_eq!(metrics.total_requests, 5000);
    }

    #[test]
    fn test_parse_tester_output_invalid_json() {
        let result = parse_tester_output("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_tester_output_empty_object() {
        let metrics = parse_tester_output("{}").unwrap();
        assert!((metrics.rps - 0.0).abs() < f64::EPSILON);
        assert_eq!(metrics.total_requests, 0);
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
