//! Background scheduler — checks for due `test_schedule` rows every 30s and
//! creates `test_run` entries, dispatching them to online agents via WS v2.
//!
//! v0.28.0: Rewritten for the unified TestConfig model. The old polymorphic
//! schedule table (with separate job vs benchmark_config columns) is gone.

use std::str::FromStr;
use std::sync::Arc;

use crate::AppState;

fn compute_next_run(cron_expr: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let schedule = cron::Schedule::from_str(cron_expr).ok()?;
    schedule.upcoming(chrono::Utc).next()
}

/// Spawn the background scheduler loop.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        tracing::info!("Scheduler background task started (v2)");

        let mut last_invite_cleanup = std::time::Instant::now();
        let mut last_approval_cleanup = std::time::Instant::now();
        let mut last_inactivity_check: Option<std::time::Instant> = None;
        let mut last_health_check = std::time::Instant::now();
        let mut last_stale_job_check = std::time::Instant::now();
        let mut last_queued_redispatch = std::time::Instant::now();

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;

            if let Err(e) = tick(&state).await {
                tracing::error!(error = %e, "Scheduler tick failed");
            }

            // Re-dispatch queued runs that were launched while no agent was
            // connected (or whose WS send raced). Runs every scheduler tick
            // (~30s) so at most one tick of latency after an agent reconnects.
            if last_queued_redispatch.elapsed() > std::time::Duration::from_secs(30) {
                last_queued_redispatch = std::time::Instant::now();
                if let Err(e) = redispatch_queued_runs(&state).await {
                    tracing::error!(error = %e, "Queued-run redispatcher failed");
                }
            }

            // Expire stale workspace invites hourly
            if last_invite_cleanup.elapsed() > std::time::Duration::from_secs(3600) {
                last_invite_cleanup = std::time::Instant::now();
                if let Ok(client) = state.db.get().await {
                    match crate::db::invites::expire_stale_invites(&client).await {
                        Ok(count) if count > 0 => {
                            tracing::info!(count, "Expired stale workspace invites");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to expire stale invites");
                        }
                        _ => {}
                    }
                }
            }

            // Expire stale command approvals hourly
            if last_approval_cleanup.elapsed() > std::time::Duration::from_secs(3600) {
                last_approval_cleanup = std::time::Instant::now();
                if let Ok(client) = state.db.get().await {
                    match crate::db::command_approvals::expire_stale(&client).await {
                        Ok(count) if count > 0 => {
                            tracing::info!(count, "Expired stale command approvals");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to expire stale approvals");
                        }
                        _ => {}
                    }
                }
            }

            // Daily workspace inactivity check
            let should_check_inactivity = last_inactivity_check
                .map(|t| t.elapsed() > std::time::Duration::from_secs(86400))
                .unwrap_or(true);
            if should_check_inactivity {
                check_workspace_inactivity(&state).await;
                last_inactivity_check = Some(std::time::Instant::now());
            }

            // Stale assigned-job watchdog (every 60s)
            if last_stale_job_check.elapsed() > std::time::Duration::from_secs(60) {
                last_stale_job_check = std::time::Instant::now();
                if let Err(e) = reap_stale_assigned_jobs(&state).await {
                    tracing::error!(error = %e, "Stale assigned-job reaper failed");
                }
            }

            // Hourly system health checks
            if last_health_check.elapsed() > std::time::Duration::from_secs(3600) {
                last_health_check = std::time::Instant::now();
                run_health_checks(&state).await;
            }
        }
    });
}

async fn tick(state: &Arc<AppState>) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let due = crate::db::test_schedules::list_due(&client).await?;

    if due.is_empty() {
        return Ok(());
    }

    tracing::info!(count = due.len(), "Processing due schedules (v2)");

    for schedule in due {
        let schedule_id = schedule.id;

        // Load the test config referenced by this schedule
        let cfg = match crate::db::test_configs::get(&client, &schedule.test_config_id).await? {
            Some(c) => c,
            None => {
                tracing::warn!(
                    schedule_id = %schedule_id,
                    config_id = %schedule.test_config_id,
                    "Schedule references missing test_config, skipping"
                );
                let next = compute_next_run(&schedule.cron_expr);
                // mark_fired with a nil run_id just to advance the schedule
                let nil_run = uuid::Uuid::nil();
                crate::db::test_schedules::mark_fired(&client, &schedule_id, &nil_run, next)
                    .await?;
                continue;
            }
        };

        // Create a queued test_run
        let run = crate::db::test_runs::create(
            &client,
            &crate::db::test_runs::NewTestRun {
                test_config_id: &cfg.id,
                project_id: &cfg.project_id,
                tester_id: None,
                worker_id: None,
                comparison_group_id: None,
            },
        )
        .await?;

        tracing::info!(
            schedule_id = %schedule_id,
            run_id = %run.id,
            config_name = %cfg.name,
            "Created test_run from schedule"
        );

        // Dispatch now, or kick off provisioning if the endpoint is Pending.
        if let Err(e) = crate::provisioning::dispatch_or_provision(state, &run, &cfg).await {
            tracing::error!(
                schedule_id = %schedule_id,
                run_id = %run.id,
                error = %e,
                "dispatch_or_provision failed for scheduled run"
            );
        }

        // Advance the schedule
        let next = compute_next_run(&schedule.cron_expr);
        crate::db::test_schedules::mark_fired(&client, &schedule_id, &run.id, next).await?;

        tracing::info!(
            schedule_id = %schedule_id,
            next_fire_at = ?next,
            "Schedule run recorded"
        );
    }

    Ok(())
}

/// Start VMs associated with a deployment using cloud CLI tools.
/// Retained from v1 for use by `api::deployments`.
pub(crate) async fn start_deployment_vm(
    client: &tokio_postgres::Client,
    deployment_id: &uuid::Uuid,
) -> anyhow::Result<()> {
    let dep = crate::db::deployments::get(client, deployment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Deployment not found"))?;

    let config = &dep.config;
    let endpoints = config
        .get("endpoints")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();

    for ep in &endpoints {
        let provider = ep.get("provider").and_then(|p| p.as_str()).unwrap_or("");
        match provider {
            "azure" => {
                let rg = ep
                    .get("azure")
                    .and_then(|a| a.get("resource_group"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("networker-rg");
                let vm_name = dep.name.replace(' ', "-").to_lowercase();
                tracing::info!(provider = "azure", vm = %vm_name, rg = %rg, "Starting VM");
                let output = tokio::process::Command::new("az")
                    .args(["vm", "start", "--resource-group", rg, "--name", &vm_name])
                    .output()
                    .await;
                match output {
                    Ok(o) if o.status.success() => {
                        tracing::info!("Azure VM started successfully");
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        tracing::warn!(stderr = %stderr, "Azure VM start may have failed");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to run az vm start");
                    }
                }
            }
            "aws" => {
                if let Some(instance_id) = ep
                    .get("aws")
                    .and_then(|a| a.get("instance_id"))
                    .and_then(|i| i.as_str())
                {
                    let region = ep
                        .get("aws")
                        .and_then(|a| a.get("region"))
                        .and_then(|r| r.as_str())
                        .unwrap_or("us-east-1");
                    tracing::info!(provider = "aws", instance_id = %instance_id, "Starting VM");
                    let _ = tokio::process::Command::new("aws")
                        .args([
                            "ec2",
                            "start-instances",
                            "--instance-ids",
                            instance_id,
                            "--region",
                            region,
                        ])
                        .output()
                        .await;
                }
            }
            "gcp" => {
                if let Some(zone) = ep
                    .get("gcp")
                    .and_then(|g| g.get("zone"))
                    .and_then(|z| z.as_str())
                {
                    let vm_name = dep.name.replace(' ', "-").to_lowercase();
                    tracing::info!(provider = "gcp", vm = %vm_name, zone = %zone, "Starting VM");
                    let _ = tokio::process::Command::new("gcloud")
                        .args(["compute", "instances", "start", &vm_name, "--zone", zone])
                        .output()
                        .await;
                }
            }
            _ => {}
        }
    }

    // Wait for endpoint to be healthy
    if let Some(ref ips_val) = dep.endpoint_ips {
        let ips: Vec<String> = serde_json::from_value(ips_val.clone()).unwrap_or_default();
        let http_client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(5))
            .build()?;

        for ip in &ips {
            let url = format!("https://{ip}:8443/health");
            for attempt in 1..=12 {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                if let Ok(resp) = http_client.get(&url).send().await {
                    if resp.status().is_success() {
                        tracing::info!(ip = %ip, attempt, "Endpoint healthy after VM start");
                        break;
                    }
                }
                if attempt == 12 {
                    tracing::warn!(ip = %ip, "Endpoint not healthy after 2 minutes");
                }
            }
        }
    }

    Ok(())
}

/// Daily check for inactive workspaces: warn, suspend, and hard-delete as needed.
async fn check_workspace_inactivity(state: &crate::AppState) {
    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB error in inactivity check");
            return;
        }
    };

    // Expire stale invites
    match crate::db::invites::expire_stale_invites(&client).await {
        Ok(n) if n > 0 => tracing::info!("Expired {n} stale workspace invites"),
        _ => {}
    }

    // Find inactive workspaces (90 days)
    let inactive_90 = match crate::db::projects::find_inactive_workspaces(&client, 90).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to find inactive workspaces");
            return;
        }
    };

    for ws in &inactive_90 {
        if crate::db::workspace_warnings::has_warning(&client, &ws.project_id, "inactivity_90d")
            .await
            .unwrap_or(true)
        {
            continue;
        }
        if let Ok(members) = crate::db::projects::list_members(&client, &ws.project_id).await {
            for member in &members {
                let body = format!(
                    "Hi,\n\n\
                     Your workspace \"{}\" on AletheDash has had no activity for 90 days.\n\n\
                     It will be suspended in 30 days if no one logs in.\n\n\
                     Log in to keep your workspace active:\n{}\n\n\
                     -- AletheDash",
                    ws.name, state.public_url
                );
                let _ = crate::email::send_email(
                    &member.email,
                    &format!("AletheDash -- {} workspace will be suspended", ws.name),
                    &body,
                )
                .await;
            }
        }
        let _ = crate::db::workspace_warnings::record_warning(
            &client,
            &ws.project_id,
            "inactivity_90d",
        )
        .await;
        tracing::info!(workspace = %ws.name, "Sent 90-day inactivity warning");
    }

    // Auto-suspend warned + still inactive
    let warned_ids =
        crate::db::workspace_warnings::warnings_older_than(&client, "inactivity_90d", 30)
            .await
            .unwrap_or_default();
    for pid in &warned_ids {
        let still_inactive = crate::db::projects::find_inactive_workspaces(&client, 90)
            .await
            .map(|list| list.iter().any(|p| p.project_id == *pid))
            .unwrap_or(false);
        if still_inactive {
            let _ = crate::db::projects::suspend_project(&client, pid).await;
            tracing::info!(project_id = %pid, "Auto-suspended workspace due to inactivity");
        }
    }

    // Warn system admins about workspaces approaching hard delete
    let approaching_delete = crate::db::projects::find_suspended_older_than(&client, 360)
        .await
        .unwrap_or_default();
    for ws in &approaching_delete {
        if crate::db::workspace_warnings::has_warning(&client, &ws.project_id, "hard_delete_5d")
            .await
            .unwrap_or(true)
        {
            continue;
        }
        let admins = client
            .query(
                "SELECT email FROM dash_user WHERE is_platform_admin = TRUE AND status = 'active'",
                &[],
            )
            .await
            .unwrap_or_default();
        for admin in &admins {
            let email: String = admin.get("email");
            let body = format!(
                "AletheDash system notice:\n\n\
                 Workspace \"{}\" has been suspended for over 360 days.\n\
                 It will be permanently deleted in 5 days.\n\n\
                 -- AletheDash",
                ws.name
            );
            let _ = crate::email::send_email(
                &email,
                &format!("AletheDash -- {} permanent deletion in 5 days", ws.name),
                &body,
            )
            .await;
        }
        let _ = crate::db::workspace_warnings::record_warning(
            &client,
            &ws.project_id,
            "hard_delete_5d",
        )
        .await;
    }

    // Hard delete workspaces suspended for 365+ days
    let to_delete = crate::db::projects::find_suspended_older_than(&client, 365)
        .await
        .unwrap_or_default();
    for ws in &to_delete {
        tracing::warn!(workspace = %ws.name, "Auto-deleting workspace (365 days suspended)");
        let _ = crate::db::projects::hard_delete_project(&client, &ws.project_id).await;
    }

    // Expire stale command approvals
    match crate::db::command_approvals::expire_stale(&client).await {
        Ok(n) if n > 0 => tracing::info!("Expired {n} stale command approvals"),
        _ => {}
    }
}

async fn run_health_checks(state: &Arc<AppState>) {
    let core_status = match state.db.get().await {
        Ok(client) => match client.query_one("SELECT 1 as ok", &[]).await {
            Ok(_) => ("green", None::<String>),
            Err(e) => {
                tracing::error!(error = %e, "Health check: core DB query failed");
                ("red", Some("Connection error".into()))
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "Health check: core DB pool error");
            ("red", Some("Pool unavailable".into()))
        }
    };

    let logs_status = match state.logs_db.get().await {
        Ok(client) => match client.query_one("SELECT 1 as ok", &[]).await {
            Ok(_) => ("green", None::<String>),
            Err(e) => {
                tracing::error!(error = %e, "Health check: logs DB query failed");
                ("red", Some("Connection error".into()))
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "Health check: logs DB pool error");
            ("red", Some("Pool unavailable".into()))
        }
    };

    let core_size: (&str, Option<String>, Option<String>) = match state.db.get().await {
        Ok(client) => {
            match client
                .query_one("SELECT pg_database_size(current_database()) as size", &[])
                .await
            {
                Ok(row) => {
                    let bytes: i64 = row.get("size");
                    let gb = bytes as f64 / 1_073_741_824.0;
                    let status = if gb > 5.0 {
                        "red"
                    } else if gb > 3.0 {
                        "yellow"
                    } else {
                        "green"
                    };
                    (status, Some(format!("{gb:.2} GB")), None)
                }
                Err(e) => {
                    tracing::error!(error = %e, "Health check: DB size query failed");
                    ("red", None, Some("Query error".into()))
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Health check: DB pool error");
            ("red", None, Some("Pool unavailable".into()))
        }
    };

    let logs_size: (&str, Option<String>, Option<String>) = match state.logs_db.get().await {
        Ok(client) => {
            match client
                .query_one("SELECT pg_database_size(current_database()) as size", &[])
                .await
            {
                Ok(row) => {
                    let bytes: i64 = row.get("size");
                    let gb = bytes as f64 / 1_073_741_824.0;
                    let status = if gb > 2.0 {
                        "red"
                    } else if gb > 1.0 {
                        "yellow"
                    } else {
                        "green"
                    };
                    (status, Some(format!("{gb:.2} GB")), None)
                }
                Err(e) => {
                    tracing::error!(error = %e, "Health check: logs DB size query failed");
                    ("red", None, Some("Query error".into()))
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Health check: logs DB pool error");
            ("red", None, Some("Pool unavailable".into()))
        }
    };

    let retention_status: (&str, Option<String>, Option<String>) = match state.logs_db.get().await {
        Ok(client) => {
            match client
                .query_opt("SELECT MIN(logged_at) as oldest FROM perf_log", &[])
                .await
            {
                Ok(Some(row)) => {
                    let oldest: Option<chrono::DateTime<chrono::Utc>> = row.get("oldest");
                    match oldest {
                        Some(ts) => {
                            let age_days = (chrono::Utc::now() - ts).num_days();
                            let status = if age_days > 8 {
                                "red"
                            } else if age_days > 7 {
                                "yellow"
                            } else {
                                "green"
                            };
                            (status, Some(format!("{age_days} days")), None)
                        }
                        None => ("green", Some("empty".into()), None),
                    }
                }
                Ok(None) => ("green", Some("empty".into()), None),
                Err(e) => {
                    tracing::error!(error = %e, "Health check: retention query failed");
                    ("red", None, Some("Query error".into()))
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Health check: logs DB pool error");
            ("red", None, Some("Pool unavailable".into()))
        }
    };

    if let Ok(client) = state.db.get().await {
        let checks: Vec<(&str, &str, Option<&str>, Option<&str>)> = vec![
            ("core_db", core_status.0, None, core_status.1.as_deref()),
            ("logs_db", logs_status.0, None, logs_status.1.as_deref()),
            (
                "core_db_size",
                core_size.0,
                core_size.1.as_deref(),
                core_size.2.as_deref(),
            ),
            (
                "logs_db_size",
                logs_size.0,
                logs_size.1.as_deref(),
                logs_size.2.as_deref(),
            ),
            (
                "logs_retention",
                retention_status.0,
                retention_status.1.as_deref(),
                retention_status.2.as_deref(),
            ),
        ];

        for (name, status, value, message) in &checks {
            if let Err(e) =
                crate::db::system_health::insert(&client, name, status, *value, *message, None)
                    .await
            {
                tracing::error!(error = %e, check = name, "Failed to persist health check");
            }
        }

        if let Err(e) = crate::db::system_health::cleanup(&client).await {
            tracing::error!(error = %e, "Failed to cleanup old health records");
        }
    }
}

/// How long a run may sit in `queued` before the watchdog gives up and
/// marks it `failed`. Keeps the UI truthful when no runner is ever able
/// to claim the run (all agents offline, agents crash-looping, etc.).
const QUEUED_CUTOFF_SECS: i64 = 300; // 5 minutes
/// Minimum age for the redispatcher to consider a queued run. Any younger
/// and we assume the inline dispatch in `launch_handler` is still in flight.
const QUEUED_MIN_AGE_SECS: i64 = 15;
/// Cap on how many queued runs a single redispatch pass will try. Bounds
/// the work each tick under pathological backlog.
const QUEUED_REDISPATCH_LIMIT: i64 = 50;

/// Fail test_runs stuck in "running" whose tester/agent is no longer online,
/// plus runs stuck in "queued" that no agent has claimed within the
/// `QUEUED_CUTOFF_SECS` window.
///
/// The `queued` arm closes a gap first observed in v0.28.1: when a user
/// launches a diagnostic probe (`EndpointRef::Network` with a user-entered
/// host and no deployed target) while no agent happens to be connected at
/// that exact millisecond, `best_effort_dispatch` logs "no online agent"
/// and returns — leaving the run queued forever with no retry and no user-
/// visible error. The `redispatch_queued_runs` tick handles the retry; this
/// watchdog handles the terminal case where retry still failed.
async fn reap_stale_assigned_jobs(state: &AppState) -> anyhow::Result<()> {
    use anyhow::Context;

    let client = state
        .db
        .get()
        .await
        .context("DB pool error in stale-run reaper")?;

    // ── Stale `running` runs (v0.27.27 behaviour) ───────────────────────
    let stale = crate::db::test_runs::find_stale_assigned(&client, 120)
        .await
        .context("Failed to query stale running test_runs")?;

    for (run_id, tester_id) in stale {
        // Tester id == agent id in the v0.28 model. If the agent is still
        // connected, the run may just be slow to heartbeat — skip it.
        if let Some(ref tid) = tester_id {
            if state.agents.is_agent_online(tid).await {
                continue;
            }
        }

        let error_msg = "Agent disconnected — tester may have been deleted or restarted";

        crate::db::test_runs::set_error(&client, &run_id, error_msg)
            .await
            .context("Failed to fail stale running test_run")?;

        // JobUpdate event kept for wire-compatibility with older dashboard
        // clients; job_id carries the run id in v0.28.
        let _ = state
            .events_tx
            .send(networker_common::messages::DashboardEvent::JobUpdate {
                job_id: run_id,
                status: "failed".into(),
                agent_id: tester_id,
                started_at: None,
                finished_at: Some(chrono::Utc::now()),
            });

        tracing::warn!(
            run_id = %run_id,
            tester_id = ?tester_id,
            "Reaped stale running test_run — agent offline"
        );
    }

    // ── Stale `queued` runs (v0.28.1+) ──────────────────────────────────
    let stuck = crate::db::test_runs::find_stale_queued(&client, QUEUED_CUTOFF_SECS)
        .await
        .context("Failed to query stale queued test_runs")?;

    for run_id in stuck {
        let error_msg = format!(
            "No runner claimed this job within {} minutes — check that at least one agent is online for this workspace",
            QUEUED_CUTOFF_SECS / 60
        );

        crate::db::test_runs::set_error(&client, &run_id, &error_msg)
            .await
            .context("Failed to fail stale queued test_run")?;

        let _ = state
            .events_tx
            .send(networker_common::messages::DashboardEvent::JobUpdate {
                job_id: run_id,
                status: "failed".into(),
                agent_id: None,
                started_at: None,
                finished_at: Some(chrono::Utc::now()),
            });

        tracing::warn!(
            run_id = %run_id,
            cutoff_secs = QUEUED_CUTOFF_SECS,
            "Reaped stale queued test_run — no runner claimed it"
        );
    }

    Ok(())
}

/// Retry dispatch for runs still stuck in `queued`. Covers three failure
/// modes of the inline launch-time dispatch:
///   1. Run created while no agent was registered in the hub.
///   2. Run created during a transient WS send failure (channel full/closed).
///   3. Provisioned run promoted to `queued` by the orchestrator but no
///      agent was online at that moment.
///
/// Each candidate run has its full `TestConfig` re-loaded and is fed through
/// the same `try_dispatch_run` path used at launch. Success flips the run
/// to `running` via the standard `AgentMessage::RunStarted` flow.
async fn redispatch_queued_runs(state: &Arc<AppState>) -> anyhow::Result<()> {
    use anyhow::Context;

    let client = state
        .db
        .get()
        .await
        .context("DB pool error in queued-run redispatcher")?;

    let candidates = crate::db::test_runs::list_unclaimed_queued(
        &client,
        QUEUED_MIN_AGE_SECS,
        QUEUED_REDISPATCH_LIMIT,
    )
    .await
    .context("Failed to query unclaimed queued test_runs")?;

    if candidates.is_empty() {
        return Ok(());
    }

    tracing::info!(
        count = candidates.len(),
        "Redispatching unclaimed queued test_runs"
    );

    for run in candidates {
        let cfg = match crate::db::test_configs::get(&client, &run.test_config_id).await? {
            Some(c) => c,
            None => {
                tracing::warn!(
                    run_id = %run.id,
                    config_id = %run.test_config_id,
                    "Queued run references missing test_config; failing it"
                );
                let _ = crate::db::test_runs::set_error(
                    &client,
                    &run.id,
                    "Test config was deleted before the run could be dispatched",
                )
                .await;
                continue;
            }
        };

        // Don't disturb runs whose config is still waiting on provisioning
        // — the provisioning orchestrator will hand them off once the
        // deployment completes.
        if matches!(cfg.endpoint, networker_common::EndpointRef::Pending { .. }) {
            continue;
        }

        let outcome = crate::provisioning::try_dispatch_run(&state.agents, &run, &cfg).await;
        match outcome {
            crate::provisioning::DispatchOutcome::Sent { agent_id } => {
                tracing::info!(
                    run_id = %run.id,
                    %agent_id,
                    "Redispatched previously-queued run"
                );
            }
            crate::provisioning::DispatchOutcome::NoAgent => {
                // Leave queued. The stale-queued watchdog will fail it if
                // this persists past `QUEUED_CUTOFF_SECS`.
            }
            crate::provisioning::DispatchOutcome::SendFailed { .. } => {
                // Ditto — retry next tick.
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::compute_next_run;

    mod cron_valid {
        use super::*;

        #[test]
        fn every_minute() {
            assert!(compute_next_run("0 * * * * *").is_some());
        }

        #[test]
        fn result_is_in_the_future() {
            let result = compute_next_run("0 * * * * *").expect("should parse");
            assert!(result > chrono::Utc::now());
        }

        #[test]
        fn step_on_hours() {
            assert!(compute_next_run("0 0 */6 * * *").is_some());
        }
    }

    mod cron_invalid {
        use super::*;

        #[test]
        fn empty() {
            assert!(compute_next_run("").is_none());
        }

        #[test]
        fn five_field_rejected() {
            assert!(compute_next_run("* * * * *").is_none());
        }

        #[test]
        fn garbage() {
            assert!(compute_next_run("every monday at noon").is_none());
        }

        #[test]
        fn out_of_range_second() {
            assert!(compute_next_run("60 * * * * *").is_none());
        }
    }

    mod cron_edge {
        use super::*;

        #[test]
        fn past_year_returns_none() {
            assert!(compute_next_run("0 0 0 1 1 * 2000").is_none());
        }
    }

    mod tick_flow {
        use super::*;

        #[test]
        fn next_run_never_in_past() {
            let exprs = [
                "0 * * * * *",
                "0 0 * * * *",
                "0 0 0 * * *",
                "0 0 9 * * Mon",
                "0 */15 * * * *",
                "0 0 8,17 * * *",
            ];
            let now = chrono::Utc::now();
            for expr in &exprs {
                let next =
                    compute_next_run(expr).unwrap_or_else(|| panic!("'{expr}' should be valid"));
                assert!(next > now, "'{expr}' next_run_at must be future");
            }
        }
    }
}
