//! Per-language benchmark runs: the application proxy x language matrix,
//! Chrome harness runs, tester invocations, and the apibench workload suite.

use super::cycle::TestbedOutcome;
use super::ssh_exec::{
    collect_server_logs, deploy_app_language, deploy_proxy, shell_quote, stop_app_language,
    stop_proxy,
};
use super::status::{log_callback, status_callback, write_phase};
use crate::callback::CallbackClient;
use crate::config::{DashboardBenchmarkConfig, MethodologyConfig, TestbedConfig};
use crate::provisioner::VmInfo;
use crate::runner;
use crate::ssh;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use tokio_postgres::Client as PgClient;
use uuid::Uuid;

/// Run the Chrome-based benchmark for a proxy+language combination.
async fn run_chrome_benchmark(
    vm: &VmInfo,
    proxy: &str,
    language: &str,
    http_version: &str,
    connection_mode: &str,
    methodology: &MethodologyConfig,
    bench_token: &str,
) -> Result<serde_json::Value> {
    let token_arg = if bench_token.is_empty() {
        String::new()
    } else {
        format!(" --token {}", shell_quote(bench_token))
    };
    // Write output to a file to avoid SSH stdout buffer limits (64KB).
    // Then read the file back separately.
    let output_file = "/tmp/bench-chrome-result.json";
    let cmd = format!(
        "export PATH=/usr/bin:/usr/local/bin:$PATH && cd /opt/bench/chrome-harness && \
         node runner.js \
         --target https://localhost:8443 \
         --warmup {} \
         --measured {} \
         --concurrency 10 \
         --http-version {} \
         --connection-mode {} \
         --timeout {}{} > {} 2>/dev/null",
        methodology.warmup_runs,
        methodology.min_measured,
        shell_quote(http_version),
        shell_quote(connection_mode),
        methodology.timeout_secs,
        token_arg,
        output_file,
    );

    tracing::info!(
        "Running Chrome benchmark: {} behind {}, http={}, conn={}",
        language,
        proxy,
        http_version,
        connection_mode,
    );

    ssh::ssh_exec(&vm.ip, &cmd)
        .await
        .with_context(|| {
            format!(
                "Chrome benchmark failed for {language} behind {proxy} (http={http_version}, conn={connection_mode})"
            )
        })?;

    // Read the result file
    let output = ssh::ssh_exec(&vm.ip, &format!("cat {output_file}"))
        .await
        .with_context(|| format!("Failed to read Chrome benchmark output from {}", vm.ip))?;

    // Parse JSON output
    let result: serde_json::Value = serde_json::from_str(&output).with_context(|| {
        format!(
            "Failed to parse Chrome benchmark output for {language} behind {proxy}: {}",
            &output[..output.len().min(200)]
        )
    })?;

    // Check for error in results
    if result.get("error").is_some() {
        anyhow::bail!(
            "Chrome benchmark returned error: {}",
            result["error"].as_str().unwrap_or("unknown")
        );
    }

    Ok(result)
}

/// In application mode, HTTP/3 support depends on the proxy, not the language.
fn proxy_supports_http3(proxy: &str) -> bool {
    matches!(proxy, "nginx" | "caddy" | "traefik" | "iis")
}

/// Convert methodology modes to HTTP version labels for Chrome harness.
/// Maps http1→h1, http2→h2, http3→h3, filters by proxy capability.
fn effective_http_versions_for_proxy(proxy: &str, modes: &[String]) -> Vec<String> {
    modes
        .iter()
        .filter_map(|m| match m.as_str() {
            "http1" => Some("h1".to_string()),
            "http2" => Some("h2".to_string()),
            "http3" if proxy_supports_http3(proxy) => Some("h3".to_string()),
            "http3" => {
                tracing::info!("Skipping http3 for proxy {} (no QUIC support)", proxy);
                None
            }
            _ => None, // skip non-HTTP modes like download/upload
        })
        .collect()
}

/// Pre-Task-23 body of `execute_testbed_application`. Extracted verbatim so
/// the lock flow can wrap it in a `ReleaseGuard` without reshuffling the
/// proxy × language loop. Task 24 will prune the dead chrome-harness deploy.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_application_matrix(
    testbed: &TestbedConfig,
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    vm: &VmInfo,
    provisioned: bool,
    db: &PgClient,
    config_uuid: &Uuid,
    bench_dir: &Path,
) -> Result<TestbedOutcome> {
    let methodology = &config.methodology;

    // Filter out OS-incompatible languages (e.g. csharp-net48 on Linux).
    let languages: Vec<String> = testbed
        .languages
        .iter()
        .filter(|lang| {
            let needs_windows = matches!(lang.as_str(), "csharp-net48");
            if needs_windows && testbed.os != "windows" {
                tracing::warn!(
                    "Skipping {} on {} testbed {} (requires Windows)",
                    lang,
                    testbed.os,
                    testbed.testbed_id
                );
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();

    let mut languages_completed = 0u32;
    let mut languages_failed = 0u32;
    let total_combinations = (testbed.proxies.len() * languages.len()) as u32;
    let mut combination_index = 0u32;

    // NOTE: deadline is set AFTER setup (token deploy + harness install) completes,
    // not at function entry. Setup can take several minutes and shouldn't count
    // against the benchmark execution time budget.

    // Generate a unique API token for this VM (isolated per testbed)
    let bench_token = crate::token_manager::generate_token();
    tracing::info!(
        "Generated bench API token for testbed {} ({}...)",
        testbed.testbed_id,
        &bench_token[..8]
    );

    // Store token in Key Vault (if configured) for audit trail + revocation
    if let Err(e) = crate::token_manager::store_in_keyvault(
        &config.config_id,
        &testbed.testbed_id,
        &bench_token,
        config.created_by_email.as_deref().unwrap_or("unknown"),
        config.project_id.as_deref().unwrap_or("unknown"),
    )
    .await
    {
        tracing::warn!("Key Vault store failed (non-fatal): {e:#}");
    }

    // Deploy token to VM via SCP (secure file, not command line)
    if let Err(e) = crate::token_manager::deploy_to_vm(&vm.ip, &bench_token).await {
        tracing::error!("Failed to deploy API token to VM: {e:#}");
        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("Failed to deploy API token: {e:#}")],
        )
        .await;
        return Ok(TestbedOutcome {
            testbed_id: testbed.testbed_id.clone(),
            languages_completed: 0,
            languages_failed: total_combinations,
            provisioned_vm: provisioned,
        });
    }

    // Deploy test harness (Node.js HTTP client — not Chrome browser)
    log_callback(
        callback,
        &testbed.testbed_id,
        vec!["Installing test harness (Node.js)...".to_string()],
    )
    .await;

    // Chrome harness is installed once at tester creation (services::tester_install); no per-benchmark install.

    // Set deadline AFTER setup completes — setup (token deploy, harness install)
    // can take several minutes and must not count against benchmark time.
    let deadline = Instant::now()
        + std::time::Duration::from_secs(
            methodology.timeout_secs as u64 * total_combinations.max(1) as u64,
        );

    tracing::info!(
        testbed_id = %testbed.testbed_id,
        proxies = ?testbed.proxies,
        languages = ?languages,
        total_combinations,
        deadline_secs = methodology.timeout_secs as u64 * total_combinations.max(1) as u64,
        "Starting application benchmark proxy/language matrix"
    );

    write_phase(db, config_uuid, "running").await;

    for proxy in &testbed.proxies {
        // Check cancellation or deadline
        if *cancel_rx.borrow() {
            break;
        }
        if Instant::now() > deadline {
            tracing::warn!(
                "Application benchmark exceeded deadline on testbed {}, stopping",
                testbed.testbed_id
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec!["Benchmark exceeded wall-clock deadline, stopping".to_string()],
            )
            .await;
            break;
        }

        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("Deploying proxy: {}", proxy)],
        )
        .await;

        // Deploy proxy
        if let Err(e) = deploy_proxy(vm, proxy).await {
            tracing::error!(
                "Proxy deploy failed for {} on testbed {}: {:#}",
                proxy,
                testbed.testbed_id,
                e
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!("Proxy {} deploy failed: {e:#}", proxy)],
            )
            .await;
            languages_failed += testbed.languages.len() as u32;
            continue;
        }

        for language in &languages {
            combination_index += 1;

            if *cancel_rx.borrow() {
                break;
            }

            tracing::info!(
                "Application benchmark {}/{}: {} behind {} on testbed {}",
                combination_index,
                total_combinations,
                language,
                proxy,
                testbed.testbed_id,
            );

            status_callback(
                callback,
                &testbed.testbed_id,
                "running",
                language,
                combination_index,
                total_combinations,
                &format!(
                    "{} behind {} ({}/{})",
                    language, proxy, combination_index, total_combinations
                ),
            )
            .await;

            // Deploy language in application mode
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Deploying {} (application mode, behind {})...",
                    language, proxy
                )],
            )
            .await;

            if let Err(e) = deploy_app_language(vm, language, proxy).await {
                tracing::error!(
                    "App deploy failed for {} behind {}: {:#}",
                    language,
                    proxy,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!(
                        "Deploy failed for {} behind {}: {e:#}",
                        language, proxy
                    )],
                )
                .await;
                languages_failed += 1;
                continue;
            }

            // Run Chrome benchmark for each HTTP version the proxy supports
            let http_versions = effective_http_versions_for_proxy(proxy, &methodology.modes);

            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Running Chrome benchmark: {} behind {}, versions={:?}",
                    language, proxy, http_versions,
                )],
            )
            .await;

            let mut lang_ok = true;
            for http_ver in &http_versions {
                // Run warm connection phase
                match run_chrome_benchmark(
                    vm,
                    proxy,
                    language,
                    http_ver,
                    "warm",
                    methodology,
                    &bench_token,
                )
                .await
                {
                    Ok(result) => {
                        tracing::info!(
                            "{} behind {} ({}:warm) complete",
                            language,
                            proxy,
                            http_ver
                        );

                        // Wrap result with metadata for callback
                        let artifact = serde_json::json!({
                            "proxy": proxy,
                            "http_version": http_ver,
                            "connection_mode": "warm",
                            "chrome_results": result,
                        });

                        if let Err(e) = callback
                            .result(&testbed.testbed_id, language, artifact)
                            .await
                        {
                            tracing::error!(
                                "Failed to report result for {} behind {} ({}:warm): {e:#}",
                                language,
                                proxy,
                                http_ver
                            );
                            log_callback(
                                callback,
                                &testbed.testbed_id,
                                vec![format!(
                                    "Result callback failed for {} behind {} ({}:warm): {e:#}",
                                    language, proxy, http_ver
                                )],
                            )
                            .await;
                            lang_ok = false;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "{} behind {} ({}:warm) failed: {:#}",
                            language,
                            proxy,
                            http_ver,
                            e
                        );
                        log_callback(
                            callback,
                            &testbed.testbed_id,
                            vec![format!(
                                "{} behind {} ({}:warm) failed: {e:#}",
                                language, proxy, http_ver
                            )],
                        )
                        .await;
                        lang_ok = false;
                    }
                }
            }

            // apibench: drive the measured /api/* workload suite through the
            // proxy with the per-testbed bearer token (audit C1 — these
            // endpoints were never measured before).
            if methodology
                .modes
                .iter()
                .any(|m| m == crate::workloads::APIBENCH_MODE)
            {
                let api_params = runner::TestParams {
                    warmup_requests: methodology.warmup_runs as u64,
                    benchmark_requests: methodology.min_measured as u64,
                    timeout_secs: methodology.timeout_secs as u64,
                };
                if !run_apibench_suite(
                    vm,
                    &api_params,
                    language,
                    Some(proxy),
                    Some(&bench_token),
                    callback,
                    &testbed.testbed_id,
                    bench_dir,
                )
                .await
                {
                    lang_ok = false;
                }
            }

            if lang_ok {
                languages_completed += 1;
            } else {
                languages_failed += 1;
            }

            // Collect server logs before stopping
            collect_server_logs(vm, language, callback, &testbed.testbed_id).await;

            // Stop language server before next language
            stop_app_language(vm).await;
        }

        // Stop proxy before swap (isolation protocol)
        stop_proxy(vm).await;
    }

    // Detect anomaly: 0 completed + 0 failed means loops didn't execute
    if languages_completed == 0 && languages_failed == 0 {
        tracing::error!(
            testbed_id = %testbed.testbed_id,
            total_combinations,
            proxies = ?testbed.proxies,
            languages = ?languages,
            "Application benchmark produced 0 completed and 0 failed — \
             proxy/language loops may not have executed"
        );
        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!(
                "BUG: 0 completed + 0 failed with {} combinations (proxies={:?}, languages={:?})",
                total_combinations, testbed.proxies, languages,
            )],
        )
        .await;
    }

    write_phase(db, config_uuid, "collect").await;

    // Report testbed complete.
    let testbed_status = if languages_completed > 0 && languages_failed == 0 && !*cancel_rx.borrow()
    {
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
        total_combinations,
        total_combinations,
        &format!("Testbed complete: {languages_completed} succeeded, {languages_failed} failed"),
    )
    .await;

    // Cleanup: delete token from VM and Key Vault
    crate::token_manager::cleanup_vm(&vm.ip).await;
    if let Err(e) =
        crate::token_manager::cleanup_keyvault_vm(&config.config_id, &testbed.testbed_id).await
    {
        tracing::warn!("Key Vault cleanup failed (non-fatal): {e:#}");
    }

    Ok(TestbedOutcome {
        testbed_id: testbed.testbed_id.clone(),
        languages_completed,
        languages_failed,
        provisioned_vm: provisioned,
    })
}

/// Languages that support HTTP/3 (QUIC).
/// Others will have http3 stripped from modes to avoid wasted benchmark time.
fn supports_http3(language: &str) -> bool {
    matches!(
        language,
        "rust"
            | "nginx"
            | "go"
            | "python"
            | "csharp-net7"
            | "csharp-net8"
            | "csharp-net8-aot"
            | "csharp-net9"
            | "csharp-net9-aot"
            | "csharp-net10"
            | "csharp-net10-aot"
            | "php"
    )
}

/// Run the benchmark for a single language and collect JSON output.
// Parameter count reflects the benchmark cycle's real coordination surface;
// bundling into a struct adds indirection without clarity (measurement-path code).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_language_benchmark(
    vm: &VmInfo,
    params: &runner::TestParams,
    language: &str,
    modes: &str,
    callback_url: Option<&str>,
    callback_token: Option<&str>,
    config_id: &str,
    testbed_id: &str,
) -> Result<serde_json::Value> {
    // Skip http3 for languages that don't support QUIC
    let effective_modes = if supports_http3(language) {
        modes.to_string()
    } else {
        let filtered: Vec<&str> = modes.split(',').filter(|m| m.trim() != "http3").collect();
        if filtered.len() < modes.split(',').count() {
            tracing::info!("Skipping http3 for {} (no QUIC support)", language);
        }
        filtered.join(",")
    };

    let target = format!("https://{}:8443/health", vm.ip);
    let tester_bin = resolve_tester_path();

    tracing::info!(
        "Running tester: target={}, modes={}, runs={}, timeout={}s",
        target,
        effective_modes,
        params.benchmark_requests,
        params.timeout_secs,
    );

    // Build args; add --payload-sizes if download/upload modes are present
    let mut args = vec![
        "--target".to_string(),
        target.clone(),
        "--modes".to_string(),
        effective_modes.clone(),
        "--runs".to_string(),
        params.benchmark_requests.to_string(),
        "--timeout".to_string(),
        params.timeout_secs.to_string(),
        "--insecure".to_string(),
        "--json-stdout".to_string(),
        "--benchmark-mode".to_string(),
    ];

    let needs_payload = effective_modes.split(',').any(|m| {
        let m = m.trim();
        m.starts_with("download") || m.starts_with("upload") || m.starts_with("udp")
    });
    if needs_payload {
        args.push("--payload-sizes".to_string());
        args.push("4k,64k,1m".to_string());
    }

    // Pass progress callback flags so the tester can report live progress to the dashboard.
    if let Some(url) = callback_url {
        args.push("--progress-url".to_string());
        args.push(url.to_string());
        if let Some(token) = callback_token {
            args.push("--progress-token".to_string());
            args.push(token.to_string());
        }
        args.push("--progress-config-id".to_string());
        args.push(config_id.to_string());
        args.push("--progress-testbed-id".to_string());
        args.push(testbed_id.to_string());
        args.push("--benchmark-language".to_string());
        args.push(language.to_string());
    }

    // Timeout: account for modes * payload-sizes * runs * timeout, plus warmup buffer
    let mode_count = effective_modes.split(',').count() as u64;
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

/// Run the full apibench workload suite (the measured /api/* endpoints,
/// API-SPEC.md §4) for one language and report one result artifact per
/// workload via the callback.
///
/// `proxy`/`bearer_token` are set in application mode (the token is the
/// per-testbed BENCH_API_TOKEN; the reference servers enforce it on every
/// route except /health). Returns true when every workload ran and reported
/// successfully; nginx is skipped (it never implements /api/*, spec §9) and
/// does not count as a failure.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_apibench_suite(
    vm: &VmInfo,
    params: &runner::TestParams,
    language: &str,
    proxy: Option<&str>,
    bearer_token: Option<&str>,
    callback: &Arc<CallbackClient>,
    testbed_id: &str,
    bench_dir: &Path,
) -> bool {
    if !crate::workloads::language_supports_apibench(language) {
        tracing::info!("Skipping apibench for {} (no /api/* suite)", language);
        log_callback(
            callback,
            testbed_id,
            vec![format!(
                "Skipping apibench for {} (serves no /api/* endpoints)",
                language
            )],
        )
        .await;
        return true;
    }

    let workload_set = match crate::workloads::ApiWorkloadSet::load_or_embedded(bench_dir) {
        Ok(set) => set,
        Err(e) => {
            tracing::error!("Failed to load apibench workload set: {e:#}");
            log_callback(
                callback,
                testbed_id,
                vec![format!("apibench workload set failed to load: {e:#}")],
            )
            .await;
            return false;
        }
    };

    let mut all_ok = true;
    for workload in &workload_set.workloads {
        log_callback(
            callback,
            testbed_id,
            vec![format!(
                "Running apibench workload {} ({} {}) for {}{}",
                workload.name,
                workload.method,
                workload.path,
                language,
                proxy.map(|p| format!(" behind {p}")).unwrap_or_default(),
            )],
        )
        .await;

        match run_apibench_workload(vm, params, workload, bearer_token).await {
            Ok(tester_json) => {
                let mut artifact = serde_json::json!({
                    "mode": "apibench",
                    "workload": workload.name,
                    "endpoint": workload.path,
                    "method": workload.method,
                    "tester": tester_json,
                });
                if let Some(p) = proxy {
                    artifact["proxy"] = serde_json::Value::String(p.to_string());
                }
                if let Err(e) = callback.result(testbed_id, language, artifact).await {
                    tracing::error!(
                        "Failed to report apibench result {} for {}: {e:#}",
                        workload.name,
                        language
                    );
                    log_callback(
                        callback,
                        testbed_id,
                        vec![format!(
                            "apibench result callback failed for {}/{}: {e:#}",
                            language, workload.name
                        )],
                    )
                    .await;
                    all_ok = false;
                }
            }
            Err(e) => {
                tracing::error!(
                    "apibench workload {} failed for {}: {:#}",
                    workload.name,
                    language,
                    e
                );
                log_callback(
                    callback,
                    testbed_id,
                    vec![format!(
                        "apibench workload {} failed for {}: {e:#}",
                        workload.name, language
                    )],
                )
                .await;
                all_ok = false;
            }
        }
    }
    all_ok
}

/// Execute a single apibench workload via `networker-tester` and return the
/// parsed tester JSON output. Uses the same measurement pipeline as
/// `run_language_benchmark` (`--benchmark-mode` + `--json-stdout`); only the
/// request shape differs (path/query, optional POST body, bearer token).
async fn run_apibench_workload(
    vm: &VmInfo,
    params: &runner::TestParams,
    workload: &crate::workloads::ApiWorkload,
    bearer_token: Option<&str>,
) -> Result<serde_json::Value> {
    let base_url = format!("https://{}:8443", vm.ip);
    let args = crate::workloads::tester_args_for_workload(
        &base_url,
        workload,
        params.benchmark_requests,
        params.timeout_secs,
        bearer_token,
    );
    let tester_bin = resolve_tester_path();

    tracing::info!(
        "Running apibench tester: workload={}, target={}{}, runs={}, timeout={}s",
        workload.name,
        base_url,
        workload.path,
        params.benchmark_requests,
        params.timeout_secs,
    );

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(params.timeout_secs * params.benchmark_requests + 120),
        tokio::process::Command::new(&tester_bin)
            .args(&args)
            .output(),
    )
    .await
    .with_context(|| format!("apibench workload {} timed out", workload.name))?
    .context("failed to execute networker-tester")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "networker-tester failed for apibench workload {} (exit={}): {}",
            workload.name,
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let artifact: serde_json::Value = serde_json::from_str(&stdout)
        .with_context(|| format!("parsing tester JSON output for workload {}", workload.name))?;
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
