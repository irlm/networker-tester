//! The benchmark cycle: full sweep orchestration across testbeds, including
//! the persistent-tester lock flow for application-mode benchmarks.

use super::benchmark::{run_apibench_suite, run_application_matrix, run_language_benchmark};
use super::ssh_exec::{start_existing_server, stop_existing_server};
use super::status::{
    connect_orchestrator_db, log_callback, notify_queue_dispatcher, status_callback, write_phase,
    write_terminal_status,
};
use super::vm::{
    ensure_running_via_azure, lookup_tester, resolve_vm, teardown_testbed, ReleaseGuard,
};
use crate::callback::CallbackClient;
use crate::config::{DashboardBenchmarkConfig, TestbedConfig};
use crate::deployer;
use crate::provisioner::VmInfo;
use crate::runner;
use crate::ssh;
use crate::tester_state::{self, AcquireOutcome};
use anyhow::{Context, Result};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use uuid::Uuid;

/// Outcome of a single testbed execution.
#[allow(dead_code)]
pub(super) struct TestbedOutcome {
    pub(super) testbed_id: String,
    pub(super) languages_completed: u32,
    pub(super) languages_failed: u32,
    pub(super) provisioned_vm: bool,
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

        let outcome = execute_testbed(testbed, config, callback, cancel_rx, bench_dir).await;

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
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    bench_dir: &Path,
) -> Result<TestbedOutcome> {
    let methodology = &config.methodology;
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

    // Wait for SSH to become available (fresh VMs need 30-60s after creation)
    if provisioned {
        tracing::info!("Waiting for SSH on {}...", vm.ip);
        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("Waiting for SSH on {}...", vm.ip)],
        )
        .await;
        let mut ssh_ready = false;
        for attempt in 1..=30 {
            match ssh::ssh_exec(&vm.ip, "echo ok").await {
                Ok(_) => {
                    tracing::info!("SSH ready on {} (attempt {})", vm.ip, attempt);
                    ssh_ready = true;
                    break;
                }
                Err(_) => {
                    if attempt % 5 == 0 {
                        tracing::info!("SSH not ready on {} (attempt {}/30)", vm.ip, attempt);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
        if !ssh_ready {
            anyhow::bail!("SSH not available on {} after 5 minutes", vm.ip);
        }
    }

    log_callback(
        callback,
        &testbed.testbed_id,
        vec![format!(
            "VM ready: {} at {} (provisioned={})",
            vm.name, vm.ip, provisioned
        )],
    )
    .await;

    // Branch: application mode uses proxy × language matrix.
    if config.benchmark_type == "application" {
        return execute_testbed_application(
            testbed,
            config,
            callback,
            cancel_rx,
            bench_dir,
            &vm,
            provisioned,
        )
        .await;
    }

    // Step 2: Iterate over languages (fullstack mode).
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

        // Run benchmark for each mode. "apibench" is an orchestrator-level
        // mode (the measured /api/* workload suite) — it must be stripped
        // before building the tester's --modes list, because the tester has
        // no such protocol and would silently run nothing (audit C1).
        let apibench_requested = methodology
            .modes
            .iter()
            .any(|m| m == crate::workloads::APIBENCH_MODE);
        let modes_str = methodology
            .modes
            .iter()
            .map(|s| s.as_str())
            .filter(|m| *m != crate::workloads::APIBENCH_MODE)
            .collect::<Vec<_>>()
            .join(",");

        let test_params = runner::TestParams {
            warmup_requests: methodology.warmup_runs as u64,
            benchmark_requests: methodology.min_measured as u64,
            timeout_secs: methodology.timeout_secs as u64,
        };

        let mut lang_ok = true;

        if !modes_str.is_empty() {
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

            match run_language_benchmark(
                &vm,
                &test_params,
                language,
                &modes_str,
                config.callback_url.as_deref(),
                config.callback_token.as_deref(),
                &config.config_id,
                &testbed.testbed_id,
            )
            .await
            {
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
                        log_callback(
                            callback,
                            &testbed.testbed_id,
                            vec![format!("Result callback failed for {}: {e:#}", language)],
                        )
                        .await;
                        lang_ok = false;
                    }
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
                    lang_ok = false;
                }
            }
        }

        if apibench_requested
            && !run_apibench_suite(
                &vm,
                &test_params,
                language,
                None,
                None,
                callback,
                &testbed.testbed_id,
                bench_dir,
            )
            .await
        {
            lang_ok = false;
        }

        if lang_ok {
            languages_completed += 1;
        } else {
            languages_failed += 1;
        }

        // Stop the server before the next language.
        if use_existing {
            stop_existing_server(&vm).await;
        } else if let Err(e) = deployer::stop_api(&vm).await {
            tracing::warn!("Failed to stop API after {}: {e}", language);
        }
    }

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

/// Execute application benchmark: proxy × language matrix, guarded by the
/// persistent-tester lock flow.
///
/// Task 23 rewrite: this function now looks up the `project_tester` row bound
/// to the benchmark config, acquires its lock via `tester_state::try_acquire`,
/// runs the existing proxy × language matrix under a `ReleaseGuard`, then
/// releases and notifies the queue dispatcher. Queued-class outcomes short
/// circuit with `benchmark_config.status='queued'` so the dashboard
/// dispatcher can promote the next waiter.
async fn execute_testbed_application(
    testbed: &TestbedConfig,
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    bench_dir: &Path,
    vm: &VmInfo,
    provisioned: bool,
) -> Result<TestbedOutcome> {
    // ---------------------------------------------------------------
    // Persistent-tester lock flow
    // ---------------------------------------------------------------
    let config_uuid = Uuid::parse_str(&config.config_id)
        .with_context(|| format!("config_id {:?} is not a valid UUID", config.config_id))?;

    let db = connect_orchestrator_db().await?;
    let tester = lookup_tester(&db, &config_uuid).await?;
    tracing::info!(
        tester_id = %tester.tester_id,
        tester_name = %tester.name,
        power_state = %tester.power_state,
        allocation = %tester.allocation,
        "resolved persistent tester for config"
    );

    write_phase(&db, &config_uuid, "starting").await;

    // Helper: queued-class outcomes short-circuit with a `queued` status.
    // Centralising the TestbedOutcome shape keeps the acquire-loop arms tidy.
    let queued_outcome = || TestbedOutcome {
        testbed_id: testbed.testbed_id.clone(),
        languages_completed: 0,
        languages_failed: 0,
        provisioned_vm: provisioned,
    };

    // Acquire loop: bounded retries with a small backoff so a stuck
    // transient state never spins hot. NeedsStart is the one outcome where
    // the orchestrator actively drives the VM back to running; everything
    // else either retries briefly, queues, or bails.
    let max_attempts = 20u32;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if attempt > max_attempts {
            // No guard held yet; record a terminal failed status before bailing.
            write_terminal_status(&db, &config_uuid, "failed").await;
            anyhow::bail!(
                "could not acquire tester {} after {} attempts",
                tester.tester_id,
                max_attempts
            );
        }

        let outcome = match tester_state::try_acquire(&db, &tester.tester_id, &config_uuid).await {
            Ok(o) => o,
            Err(e) => {
                // No guard yet; failing to even issue the acquire UPDATE is
                // terminal for this benchmark attempt.
                write_terminal_status(&db, &config_uuid, "failed").await;
                return Err(e);
            }
        };
        match outcome {
            AcquireOutcome::Acquired => {
                break;
            }
            AcquireOutcome::NeedsStart => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    "tester stopped — starting VM before retrying acquire"
                );
                if let Err(e) = ensure_running_via_azure(&tester, &db).await {
                    tracing::error!(
                        tester_id = %tester.tester_id,
                        "ensure_running_via_azure failed: {e:#}"
                    );
                    write_terminal_status(&db, &config_uuid, "failed").await;
                    anyhow::bail!("failed to start tester {}: {e:#}", tester.tester_id);
                }
                // Nudge power_state forward; best-effort, dispatcher also reconciles.
                let _ = tester_state::try_power_transition(
                    &db,
                    &tester.tester_id,
                    "stopped",
                    "running",
                )
                .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            AcquireOutcome::Transient(state) => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    state,
                    "tester in transient state — queuing"
                );
                // No guard yet; record queued and return Ok (short-circuit).
                write_terminal_status(&db, &config_uuid, "queued").await;
                return Ok(queued_outcome());
            }
            AcquireOutcome::Upgrading => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    "tester upgrading — queuing"
                );
                write_terminal_status(&db, &config_uuid, "queued").await;
                return Ok(queued_outcome());
            }
            AcquireOutcome::AlreadyLockedBy(other) => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    locked_by = %other,
                    "tester already locked by another config — queuing"
                );
                write_terminal_status(&db, &config_uuid, "queued").await;
                return Ok(queued_outcome());
            }
            AcquireOutcome::Errored => {
                write_terminal_status(&db, &config_uuid, "failed").await;
                anyhow::bail!(
                    "tester {} is in error state — cannot run benchmark",
                    tester.tester_id
                );
            }
            AcquireOutcome::Gone => {
                // RR-007: tester row was deleted during acquire. Treat as
                // terminal failure — there is nothing to queue against.
                tracing::error!(
                    tester_id = %tester.tester_id,
                    config_id = %config_uuid,
                    "tester deleted during acquire — failing benchmark"
                );
                write_terminal_status(&db, &config_uuid, "failed").await;
                anyhow::bail!("tester {} deleted during acquire", tester.tester_id);
            }
            AcquireOutcome::NotIdle(state) => {
                tracing::warn!(
                    tester_id = %tester.tester_id,
                    state,
                    "tester in unexpected state — queuing"
                );
                write_terminal_status(&db, &config_uuid, "queued").await;
                return Ok(queued_outcome());
            }
        }
    }

    // ---------------------------------------------------------------
    // RR-002: we now hold the lock. The matrix must run inside a panic
    // boundary so that a panic in deploy/runner code cannot skip
    // `release_now().await`. Drop is a defensive backstop only — Drop
    // spawns release as a detached task, which can be cancelled by
    // runtime shutdown, leaking the lock.
    // ---------------------------------------------------------------
    let guard = ReleaseGuard::new(db.clone(), tester.tester_id, config_uuid);
    write_phase(&db, &config_uuid, "deploy").await;

    let matrix_result = AssertUnwindSafe(run_application_matrix(
        testbed,
        config,
        callback,
        cancel_rx,
        vm,
        provisioned,
        &db,
        &config_uuid,
        bench_dir,
    ))
    .catch_unwind()
    .await;

    // Synchronous release, awaited before any terminal status write so the
    // dispatcher notification below observes an idle row.
    guard.release_now().await;
    notify_queue_dispatcher(&tester.tester_id).await;

    match matrix_result {
        Ok(Ok(outcome)) => {
            // Happy path — final status based on matrix outcome.
            let final_status = if outcome.languages_completed > 0 && outcome.languages_failed == 0 {
                "completed"
            } else if outcome.languages_completed == 0 && outcome.languages_failed == 0 {
                // No work ran (cancel before loop body). Leave as completed.
                "completed"
            } else if outcome.languages_completed == 0 {
                "failed"
            } else {
                "completed_with_errors"
            };
            write_terminal_status(&db, &config_uuid, final_status).await;
            write_phase(&db, &config_uuid, "done").await;
            Ok(outcome)
        }
        Ok(Err(e)) => {
            // Matrix returned Err — record failed, surface the error.
            tracing::error!(
                config_id = %config_uuid,
                "application matrix returned error: {e:#}"
            );
            write_terminal_status(&db, &config_uuid, "failed").await;
            write_phase(&db, &config_uuid, "done").await;
            Err(e)
        }
        Err(panic_payload) => {
            // Matrix panicked — lock already released above, now record
            // a terminal failed status and surface a synthesised error.
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            tracing::error!(
                target: "orchestrator_matrix_panic",
                config_id = %config_uuid,
                tester_id = %tester.tester_id,
                panic = %panic_msg,
                "application matrix panicked — released lock, recording failed status"
            );
            write_terminal_status(&db, &config_uuid, "failed").await;
            write_phase(&db, &config_uuid, "done").await;
            Err(anyhow::anyhow!(
                "application matrix panicked for config {}: {}",
                config_uuid,
                panic_msg
            ))
        }
    }
}
