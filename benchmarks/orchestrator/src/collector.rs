use crate::provisioner::VmInfo;
use crate::ssh::ssh_exec;
use crate::types::{BinaryMetrics, ResourceMetrics};
use anyhow::{bail, Context, Result};
use std::time::Duration;

/// Collect system and optional per-process resource metrics from the metrics agent.
///
/// Returns `ResourceMetrics::default()` when the metrics agent is unreachable,
/// so callers can proceed without resource data.
pub async fn collect_metrics(vm: &VmInfo, server_pid: Option<u32>) -> Result<ResourceMetrics> {
    // System-level metrics
    let system_json = match http_get(&format!("http://{}:9100/metrics", vm.ip)).await {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(
                "Metrics agent unreachable on {} (returning defaults): {e}",
                vm.ip
            );
            return Ok(ResourceMetrics::default());
        }
    };

    let system: serde_json::Value = match serde_json::from_str(&system_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "Invalid metrics JSON from {} (returning defaults): {e}",
                vm.ip
            );
            return Ok(ResourceMetrics::default());
        }
    };

    let mut metrics = ResourceMetrics {
        peak_rss_bytes: system
            .get("memory_rss_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        avg_cpu_fraction: system
            .get("cpu_percent")
            .and_then(|v| v.as_f64())
            .map(|pct| pct / 100.0)
            .unwrap_or(0.0),
        peak_cpu_fraction: system
            .get("cpu_percent")
            .and_then(|v| v.as_f64())
            .map(|pct| pct / 100.0)
            .unwrap_or(0.0),
        peak_open_fds: 0,
    };

    // Per-process metrics (more accurate if we know the server PID)
    if let Some(pid) = server_pid {
        let proc_url = format!("http://{}:9100/metrics/process/{}", vm.ip, pid);
        match http_get(&proc_url).await {
            Ok(proc_json) => {
                if let Ok(proc) = serde_json::from_str::<serde_json::Value>(&proc_json) {
                    if let Some(rss) = proc.get("memory_rss_bytes").and_then(|v| v.as_u64()) {
                        metrics.peak_rss_bytes = rss;
                    }
                    if let Some(cpu) = proc.get("cpu_percent").and_then(|v| v.as_f64()) {
                        metrics.avg_cpu_fraction = cpu / 100.0;
                        metrics.peak_cpu_fraction = cpu / 100.0;
                    }
                    if let Some(fds) = proc.get("open_fds").and_then(|v| v.as_u64()) {
                        metrics.peak_open_fds = fds;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Could not fetch process metrics for PID {pid}: {e}");
            }
        }
    }

    Ok(metrics)
}

/// Measure binary size and idle memory for a language implementation.
pub async fn measure_binary_size(vm: &VmInfo, language: &str) -> Result<BinaryMetrics> {
    tracing::info!("Measuring binary size for {language} on {}", vm.name);

    // Determine the binary path based on language
    let binary_path = binary_path_for_language(language);

    // Get file size via SSH
    let size_cmd = format!(
        "stat -c '%s' {binary_path} 2>/dev/null || stat -f '%z' {binary_path} 2>/dev/null || echo 0"
    );
    let size_output = ssh_exec(&vm.ip, &size_cmd)
        .await
        .context("getting binary size")?;

    let size_bytes: u64 = size_output.trim().parse().unwrap_or(0);

    // Get compressed size
    let compress_cmd = format!("gzip -c {binary_path} 2>/dev/null | wc -c || echo 0");
    let compressed_output = ssh_exec(&vm.ip, &compress_cmd)
        .await
        .unwrap_or_else(|_| "0".to_string());

    let compressed_size_bytes: u64 = compressed_output.trim().parse().unwrap_or(0);

    tracing::info!(
        "{language} binary: {} bytes ({} compressed)",
        size_bytes,
        compressed_size_bytes
    );

    Ok(BinaryMetrics {
        size_bytes,
        compressed_size_bytes,
        docker_image_bytes: None,
    })
}

/// Map a language name to its expected binary path on the VM.
fn binary_path_for_language(language: &str) -> &'static str {
    match language {
        "rust" => "/opt/bench/server",
        "go" => "/opt/bench/go-server",
        "cpp" => "/opt/bench/cpp-server",
        "java" => "/opt/bench/java-server.jar",
        "csharp-net10" | "csharp-net10-aot" => "/opt/bench/csharp-server",
        "nodejs" => "/opt/bench/server.js",
        "python" => "/opt/bench/server.py",
        "nginx" => "/usr/sbin/nginx",
        _ => "/opt/bench/server",
    }
}

/// Collect resource metrics at 1-second intervals for the specified duration.
///
/// Returns a time series of ResourceMetrics snapshots.
pub async fn collect_during_test(
    vm: &VmInfo,
    duration_secs: u64,
    server_pid: Option<u32>,
) -> Result<Vec<ResourceMetrics>> {
    tracing::info!(
        "Collecting metrics for {}s on {} (pid={:?})",
        duration_secs,
        vm.name,
        server_pid
    );

    let mut samples = Vec::with_capacity(duration_secs as usize);
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    for i in 0..duration_secs {
        interval.tick().await;

        match collect_metrics(vm, server_pid).await {
            Ok(m) => {
                tracing::trace!(
                    "Sample {i}: rss={} cpu={:.2}",
                    m.peak_rss_bytes,
                    m.avg_cpu_fraction
                );
                samples.push(m);
            }
            Err(e) => {
                tracing::warn!("Failed to collect sample {i}: {e}");
            }
        }
    }

    if samples.is_empty() {
        tracing::warn!(
            "No metrics samples collected during {}s window on {} — returning empty series",
            duration_secs,
            vm.name
        );
        return Ok(vec![]);
    }

    tracing::info!("Collected {} metric samples", samples.len());
    Ok(samples)
}

/// Compute aggregate ResourceMetrics from a time series of samples.
pub fn aggregate_metrics(samples: &[ResourceMetrics]) -> ResourceMetrics {
    if samples.is_empty() {
        return ResourceMetrics::default();
    }

    let n = samples.len() as f64;
    let mut peak_rss: u64 = 0;
    let mut sum_cpu: f64 = 0.0;
    let mut peak_cpu: f64 = 0.0;
    let mut peak_fds: u64 = 0;

    for s in samples {
        peak_rss = peak_rss.max(s.peak_rss_bytes);
        sum_cpu += s.avg_cpu_fraction;
        peak_cpu = peak_cpu.max(s.peak_cpu_fraction);
        peak_fds = peak_fds.max(s.peak_open_fds);
    }

    ResourceMetrics {
        peak_rss_bytes: peak_rss,
        avg_cpu_fraction: sum_cpu / n,
        peak_cpu_fraction: peak_cpu,
        peak_open_fds: peak_fds,
    }
}

/// HTTP GET via curl, returning the response body.
async fn http_get(url: &str) -> Result<String> {
    let output = tokio::process::Command::new("curl")
        .args(["-s", "--connect-timeout", "5", "--max-time", "10", url])
        .output()
        .await
        .context("curl failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("HTTP GET {url} failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// SSH helper for executing commands on the VM.
// ssh_exec imported from crate::ssh (shared, hardened with timeout + keepalive)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_empty() {
        let result = aggregate_metrics(&[]);
        assert_eq!(result.peak_rss_bytes, 0);
        assert!((result.avg_cpu_fraction - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_single_sample() {
        let samples = vec![ResourceMetrics {
            peak_rss_bytes: 1024,
            avg_cpu_fraction: 0.5,
            peak_cpu_fraction: 0.5,
            peak_open_fds: 42,
        }];
        let agg = aggregate_metrics(&samples);
        assert_eq!(agg.peak_rss_bytes, 1024);
        assert!((agg.avg_cpu_fraction - 0.5).abs() < f64::EPSILON);
        assert_eq!(agg.peak_open_fds, 42);
    }

    #[test]
    fn test_aggregate_multiple_samples() {
        let samples = vec![
            ResourceMetrics {
                peak_rss_bytes: 1000,
                avg_cpu_fraction: 0.2,
                peak_cpu_fraction: 0.3,
                peak_open_fds: 10,
            },
            ResourceMetrics {
                peak_rss_bytes: 2000,
                avg_cpu_fraction: 0.4,
                peak_cpu_fraction: 0.6,
                peak_open_fds: 20,
            },
            ResourceMetrics {
                peak_rss_bytes: 1500,
                avg_cpu_fraction: 0.3,
                peak_cpu_fraction: 0.4,
                peak_open_fds: 15,
            },
        ];
        let agg = aggregate_metrics(&samples);
        assert_eq!(agg.peak_rss_bytes, 2000);
        assert!((agg.avg_cpu_fraction - 0.3).abs() < 0.001);
        assert!((agg.peak_cpu_fraction - 0.6).abs() < f64::EPSILON);
        assert_eq!(agg.peak_open_fds, 20);
    }

    #[test]
    fn test_binary_path_for_language() {
        assert_eq!(binary_path_for_language("rust"), "/opt/bench/server");
        assert_eq!(binary_path_for_language("go"), "/opt/bench/go-server");
        assert_eq!(binary_path_for_language("nginx"), "/usr/sbin/nginx");
        assert_eq!(
            binary_path_for_language("unknown-lang"),
            "/opt/bench/server"
        );
    }
}
