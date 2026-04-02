use crate::callback::CallbackClient;
use crate::config::{DashboardBenchmarkConfig, MethodologyConfig, TestbedConfig};
use crate::deployer;
use crate::provisioner::{self, VmInfo};
use crate::runner;
use crate::ssh;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

/// Start a pre-deployed language server on an existing VM.
async fn start_existing_server(vm: &VmInfo, language: &str) -> Result<()> {
    // Kill anything on port 8443
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8443 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let start_cmd = match language {
        "rust" => "nohup /opt/bench/rust-server --https-port 8443 > /dev/null 2>&1 &",
        "go" => "BENCH_CERT_DIR=/opt/bench nohup /opt/bench/go-server > /dev/null 2>&1 &",
        "cpp" => "BENCH_CERT_DIR=/opt/bench nohup /opt/bench/cpp-build/server > /dev/null 2>&1 &",
        "nodejs" => "BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup node /opt/bench/nodejs-server.js > /dev/null 2>&1 &",
        "python" => "cd /opt/bench && BENCH_CERT_DIR=/opt/bench nohup uvicorn server:app --host 0.0.0.0 --port 8443 --ssl-keyfile /opt/bench/key.pem --ssl-certfile /opt/bench/cert.pem --log-level error > /dev/null 2>&1 &",
        "java" => "cd /opt/bench && BENCH_CERT_DIR=/opt/bench nohup java Server > /dev/null 2>&1 &",
        "ruby" => "cd /opt/bench/ruby && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup bundle exec puma -C puma.rb > /dev/null 2>&1 &",
        "php" => "BENCH_CERT_DIR=/opt/bench nohup php /opt/bench/php/server.php > /dev/null 2>&1 &",
        "nginx" => "sudo systemctl restart nginx",
        _ if language.starts_with("csharp-") => {
            // Handled below with dynamic string
            ""
        }
        _ => anyhow::bail!("Unknown language: {language}"),
    };

    if language.starts_with("csharp-") {
        let cmd = format!(
            "chmod +x /opt/bench/{lang}/{lang} 2>/dev/null; BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup /opt/bench/{lang}/{lang} > /dev/null 2>&1 &",
            lang = language
        );
        ssh::ssh_exec(&vm.ip, &cmd).await?;
    } else {
        ssh::ssh_exec(&vm.ip, start_cmd).await?;
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Health check
    for i in 0..15 {
        if let Ok(out) = ssh::ssh_exec(
            &vm.ip,
            "curl -sk --max-time 2 https://localhost:8443/health 2>/dev/null",
        )
        .await
        {
            if out.contains("ok") || out.contains("status") {
                tracing::info!("{} server healthy on {}", language, vm.ip);
                return Ok(());
            }
        }
        if i < 14 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    anyhow::bail!("{} server failed health check after 15s", language)
}

/// Stop any running server on port 8443.
#[allow(dead_code)]
async fn stop_existing_server(vm: &VmInfo) {
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8443 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
}

/// Outcome of a single testbed execution.
#[allow(dead_code)]
struct TestbedOutcome {
    testbed_id: String,
    languages_completed: u32,
    languages_failed: u32,
    provisioned_vm: bool,
}

/// Execute the full benchmark sweep triggered by the dashboard.
///
/// For each testbed in the config, provisions/reuses a VM, deploys each language,
/// runs the benchmark, reports results via callback, and optionally tears down.
pub async fn execute_dashboard_benchmark(
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    bench_dir: &Path,
) -> Result<()> {
    let overall_start = Instant::now();
    let total_testbeds = config.testbeds.len();

    tracing::info!(
        "Starting dashboard benchmark: config_id={}, testbeds={}, languages_per_testbed=variable",
        config.config_id,
        total_testbeds,
    );

    let mut any_failure = false;

    for (testbed_index, testbed) in config.testbeds.iter().enumerate() {
        // Check cancellation before each testbed.
        if *cancel_rx.borrow() {
            tracing::warn!(
                "Cancellation requested before testbed {}",
                testbed.testbed_id
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Cancelled before testbed {} of {}",
                    testbed_index + 1,
                    total_testbeds
                )],
            )
            .await;
            break;
        }

        tracing::info!(
            "--- Testbed {}/{}: {} ({}/{}) ---",
            testbed_index + 1,
            total_testbeds,
            testbed.testbed_id,
            testbed.cloud,
            testbed.region,
        );

        let outcome =
            execute_testbed(testbed, &config.methodology, callback, cancel_rx, bench_dir).await;

        match outcome {
            Ok(outcome) => {
                if outcome.languages_failed > 0 {
                    any_failure = true;
                }
                // Teardown if auto_teardown and we provisioned the VM
                if config.auto_teardown && outcome.provisioned_vm {
                    teardown_testbed(testbed, callback).await;
                }
            }
            Err(e) => {
                any_failure = true;
                tracing::error!("Testbed {} failed: {:#}", testbed.testbed_id, e);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Testbed failed: {e:#}")],
                )
                .await;
            }
        }
    }

    // Report overall completion.
    let duration_secs = overall_start.elapsed().as_secs_f64();
    let final_status = if *cancel_rx.borrow() {
        "cancelled"
    } else if any_failure {
        "completed_with_errors"
    } else {
        "completed"
    };

    tracing::info!(
        "Benchmark run finished: status={}, duration={:.1}s",
        final_status,
        duration_secs,
    );

    let error_msg = if any_failure {
        Some("One or more testbeds had errors".to_string())
    } else {
        None
    };
    if let Err(e) = callback
        .complete(final_status, duration_secs, error_msg)
        .await
    {
        tracing::error!("Failed to report completion: {e:#}");
    }

    Ok(())
}

/// Execute a single testbed: provision/reuse VM, deploy + benchmark each language.
async fn execute_testbed(
    testbed: &TestbedConfig,
    methodology: &MethodologyConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    bench_dir: &Path,
) -> Result<TestbedOutcome> {
    let language_total = testbed.languages.len() as u32;
    let mut languages_completed = 0u32;
    let mut languages_failed = 0u32;

    // Step 1: Resolve VM — use existing_vm_ip or provision.
    status_callback(
        callback,
        &testbed.testbed_id,
        "provisioning",
        "",
        0,
        language_total,
        "Resolving VM...",
    )
    .await;

    let (vm, provisioned) = resolve_vm(testbed)
        .await
        .with_context(|| format!("resolving VM for testbed {}", testbed.testbed_id))?;

    log_callback(
        callback,
        &testbed.testbed_id,
        vec![format!(
            "VM ready: {} at {} (provisioned={})",
            vm.name, vm.ip, provisioned
        )],
    )
    .await;

    // Step 2: Iterate over languages.
    for (lang_index, language) in testbed.languages.iter().enumerate() {
        let lang_index_u32 = lang_index as u32;

        // Check cancellation between languages.
        if *cancel_rx.borrow() {
            tracing::warn!(
                "Cancellation requested, stopping testbed {}",
                testbed.testbed_id
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Cancelled after {languages_completed} of {language_total} languages"
                )],
            )
            .await;
            break;
        }

        // Also check via callback (in case heartbeat hasn't caught up yet).
        match callback.check_cancelled().await {
            Ok(true) => {
                tracing::warn!(
                    "Dashboard cancelled, stopping testbed {}",
                    testbed.testbed_id
                );
                break;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("Cancellation check failed: {e}"),
        }

        tracing::info!(
            "Language {}/{}: {} on testbed {}",
            lang_index + 1,
            language_total,
            language,
            testbed.testbed_id,
        );

        status_callback(
            callback,
            &testbed.testbed_id,
            "running",
            language,
            lang_index_u32 + 1,
            language_total,
            &format!(
                "Running language {} of {}: {}",
                lang_index + 1,
                language_total,
                language
            ),
        )
        .await;

        // Start language server — skip full deploy for existing VMs (already deployed).
        let use_existing = testbed.existing_vm_ip.is_some();
        if use_existing {
            // Existing VM: just start the server, skip build+deploy
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!("Starting {} server on existing VM...", language)],
            )
            .await;

            if let Err(e) = start_existing_server(&vm, language).await {
                tracing::error!(
                    "Start failed for {} on testbed {}: {:#}",
                    language,
                    testbed.testbed_id,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Start failed for {}: {e:#}", language)],
                )
                .await;
                languages_failed += 1;
                continue;
            }
        } else {
            // New VM: full deploy (build + copy + start)
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!("Deploying {} server...", language)],
            )
            .await;

            if let Err(e) = deployer::deploy_api(&vm, language, bench_dir).await {
                tracing::error!(
                    "Deploy failed for {} on testbed {}: {:#}",
                    language,
                    testbed.testbed_id,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Deploy failed for {}: {e:#}", language)],
                )
                .await;
                languages_failed += 1;
                continue;
            }
        }

        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("{} server ready", language)],
        )
        .await;

        // Run benchmark for each mode.
        let modes_str = methodology
            .modes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",");

        let test_params = runner::TestParams {
            warmup_requests: methodology.warmup_runs as u64,
            benchmark_requests: methodology.min_measured as u64,
            timeout_secs: methodology.timeout_secs as u64,
        };

        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!(
                "Running benchmark: modes={}, warmup={}, measured={}, timeout={}s",
                modes_str,
                methodology.warmup_runs,
                methodology.min_measured,
                methodology.timeout_secs,
            )],
        )
        .await;

        match run_language_benchmark(&vm, &test_params, language, &modes_str).await {
            Ok(artifact_json) => {
                tracing::info!(
                    "Benchmark complete for {} on testbed {}",
                    language,
                    testbed.testbed_id
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("{} benchmark complete", language)],
                )
                .await;

                // Report result via callback.
                if let Err(e) = callback
                    .result(&testbed.testbed_id, language, artifact_json)
                    .await
                {
                    tracing::error!("Failed to report result for {}: {e:#}", language);
                }

                languages_completed += 1;
            }
            Err(e) => {
                tracing::error!(
                    "Benchmark failed for {} on testbed {}: {:#}",
                    language,
                    testbed.testbed_id,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Benchmark failed for {}: {e:#}", language)],
                )
                .await;
                languages_failed += 1;
            }
        }

        // Stop the server before the next language.
        if use_existing {
            stop_existing_server(&vm).await;
        } else if let Err(e) = deployer::stop_api(&vm).await {
            tracing::warn!("Failed to stop API after {}: {e}", language);
        }
    }

    // Report testbed complete.
    let testbed_status = if languages_failed == 0 && !*cancel_rx.borrow() {
        "completed"
    } else if *cancel_rx.borrow() {
        "cancelled"
    } else {
        "completed_with_errors"
    };

    status_callback(
        callback,
        &testbed.testbed_id,
        testbed_status,
        "",
        language_total,
        language_total,
        &format!("Testbed complete: {languages_completed} succeeded, {languages_failed} failed"),
    )
    .await;

    Ok(TestbedOutcome {
        testbed_id: testbed.testbed_id.clone(),
        languages_completed,
        languages_failed,
        provisioned_vm: provisioned,
    })
}

/// Resolve the VM for a testbed: use existing IP or provision a new one.
async fn resolve_vm(testbed: &TestbedConfig) -> Result<(VmInfo, bool)> {
    if let Some(ip) = &testbed.existing_vm_ip {
        tracing::info!(
            "Using existing VM at {} for testbed {}",
            ip,
            testbed.testbed_id
        );
        let vm = VmInfo {
            name: format!(
                "existing-{}",
                &testbed.testbed_id[..8.min(testbed.testbed_id.len())]
            ),
            ip: ip.clone(),
            cloud: testbed.cloud.clone(),
            region: testbed.region.clone(),
            os: "ubuntu".to_string(),
            vm_size: testbed.vm_size.clone(),
            resource_group: String::new(),
            ssh_user: "azureuser".to_string(),
        };
        Ok((vm, false))
    } else {
        // For now, auto-provisioning requires cloud CLI tools (az/aws/gcloud).
        // If none are available, fail fast with a helpful message.
        let vm_name = format!(
            "ab-{}-{}",
            &testbed.testbed_id[..8.min(testbed.testbed_id.len())],
            testbed.region
        );

        // Check if VM already exists.
        if let Some(existing) = provisioner::find_existing_vm(&vm_name).await? {
            if !existing.ip.is_empty() {
                tracing::info!("Reusing existing VM {} at {}", existing.name, existing.ip);
                return Ok((existing, false));
            }
        }

        tracing::info!(
            "Provisioning new VM: name={}, cloud={}, region={}, size={}",
            vm_name,
            testbed.cloud,
            testbed.region,
            testbed.vm_size,
        );

        let cloud_lower = testbed.cloud.to_lowercase();
        let size_lower = testbed.vm_size.to_lowercase();
        let resolved_size = crate::vm_tiers::resolve_vm_size(&cloud_lower, &size_lower);
        let vm = provisioner::provision_vm(
            &testbed.cloud,
            &testbed.region,
            "ubuntu",
            resolved_size,
            &vm_name,
        )
        .await?;
        Ok((vm, true))
    }
}

/// Run the benchmark for a single language and collect JSON output.
async fn run_language_benchmark(
    vm: &VmInfo,
    params: &runner::TestParams,
    language: &str,
    modes: &str,
) -> Result<serde_json::Value> {
    let target = format!("https://{}:8443/health", vm.ip);
    let tester_bin = resolve_tester_path();

    tracing::info!(
        "Running tester: target={}, modes={}, runs={}, timeout={}s",
        target,
        modes,
        params.benchmark_requests,
        params.timeout_secs,
    );

    // Build args; add --payload-sizes if download/upload modes are present
    let mut args = vec![
        "--target".to_string(),
        target.clone(),
        "--modes".to_string(),
        modes.to_string(),
        "--runs".to_string(),
        params.benchmark_requests.to_string(),
        "--timeout".to_string(),
        params.timeout_secs.to_string(),
        "--insecure".to_string(),
        "--json-stdout".to_string(),
        "--benchmark-mode".to_string(),
    ];

    let needs_payload = modes.split(',').any(|m| {
        let m = m.trim();
        m.starts_with("download") || m.starts_with("upload") || m.starts_with("udp")
    });
    if needs_payload {
        args.push("--payload-sizes".to_string());
        args.push("4k,64k,1m".to_string());
    }

    // Timeout: account for modes * payload-sizes * runs * timeout, plus warmup buffer
    let mode_count = modes.split(',').count() as u64;
    let payload_multiplier = if needs_payload { 3u64 } else { 1u64 }; // 4k, 64k, 1m
    let total_requests = mode_count * payload_multiplier * params.benchmark_requests;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(params.timeout_secs * total_requests + 120),
        tokio::process::Command::new(&tester_bin)
            .args(&args)
            .output(),
    )
    .await
    .context("benchmark timed out")?
    .context("failed to execute networker-tester")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "networker-tester failed for {language} (exit={}): {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let artifact: serde_json::Value =
        serde_json::from_str(&stdout).context("parsing tester JSON output")?;

    Ok(artifact)
}

/// Resolve the path to `networker-tester` (same logic as runner.rs).
fn resolve_tester_path() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|root| root.join("target/release/networker-tester"))
            .unwrap_or_default();
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    "networker-tester".to_string()
}

/// Tear down a provisioned VM for a testbed.
async fn teardown_testbed(testbed: &TestbedConfig, callback: &Arc<CallbackClient>) {
    let vm_name = format!(
        "ab-{}-{}",
        &testbed.testbed_id[..8.min(testbed.testbed_id.len())],
        testbed.region
    );

    log_callback(
        callback,
        &testbed.testbed_id,
        vec![format!("Tearing down VM {vm_name}...")],
    )
    .await;

    // Find and destroy the VM.
    match provisioner::find_existing_vm(&vm_name).await {
        Ok(Some(vm)) => {
            if let Err(e) = provisioner::destroy_vm(&vm).await {
                tracing::error!("Failed to destroy VM {}: {e:#}", vm_name);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Teardown failed for {vm_name}: {e:#}")],
                )
                .await;
            } else {
                tracing::info!("VM {} destroyed", vm_name);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("VM {vm_name} destroyed")],
                )
                .await;
            }
        }
        Ok(None) => {
            tracing::debug!("VM {} not found, nothing to tear down", vm_name);
        }
        Err(e) => {
            tracing::warn!("Failed to look up VM {} for teardown: {e}", vm_name);
        }
    }
}

/// Helper: send a status callback, logging errors but not failing.
async fn status_callback(
    callback: &CallbackClient,
    testbed_id: &str,
    status: &str,
    current_language: &str,
    language_index: u32,
    language_total: u32,
    message: &str,
) {
    if let Err(e) = callback
        .status(
            testbed_id,
            status,
            current_language,
            language_index,
            language_total,
            message,
        )
        .await
    {
        tracing::warn!("Status callback failed: {e}");
    }
}

/// Helper: send a log callback, logging errors but not failing.
async fn log_callback(callback: &CallbackClient, testbed_id: &str, lines: Vec<String>) {
    if let Err(e) = callback.log(testbed_id, lines).await {
        tracing::warn!("Log callback failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_tester_path_fallback() {
        // When running tests, the binary path won't resolve to a real tester,
        // so we expect the PATH fallback.
        let path = resolve_tester_path();
        // Should either be an absolute path or the fallback "networker-tester"
        assert!(
            path == "networker-tester" || std::path::Path::new(&path).is_absolute(),
            "unexpected tester path: {path}"
        );
    }
}
