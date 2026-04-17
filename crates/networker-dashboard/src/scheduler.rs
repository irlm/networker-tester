use std::str::FromStr;
use std::sync::Arc;

use crate::AppState;

fn compute_next_run(cron_expr: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let schedule = cron::Schedule::from_str(cron_expr).ok()?;
    schedule.upcoming(chrono::Utc).next()
}

/// Background loop that checks for due schedules every 30s and creates jobs.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Wait for server startup
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        tracing::info!("Scheduler background task started");

        let mut last_invite_cleanup = std::time::Instant::now();
        let mut last_approval_cleanup = std::time::Instant::now();
        let mut last_inactivity_check: Option<std::time::Instant> = None;
        let mut last_health_check = std::time::Instant::now();
        let mut last_stale_job_check = std::time::Instant::now();

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;

            if let Err(e) = tick(&state).await {
                tracing::error!(error = %e, "Scheduler tick failed");
            }

            // Expire stale workspace invites hourly
            if last_invite_cleanup.elapsed() > std::time::Duration::from_secs(3600) {
                last_invite_cleanup = std::time::Instant::now();
                match state.db.get().await {
                    Ok(client) => match crate::db::invites::expire_stale_invites(&client).await {
                        Ok(count) if count > 0 => {
                            tracing::info!(count, "Expired stale workspace invites");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to expire stale invites");
                        }
                        _ => {}
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "DB pool error in invite cleanup");
                    }
                }
            }

            // Expire stale command approvals hourly
            if last_approval_cleanup.elapsed() > std::time::Duration::from_secs(3600) {
                last_approval_cleanup = std::time::Instant::now();
                match state.db.get().await {
                    Ok(client) => match crate::db::command_approvals::expire_stale(&client).await {
                        Ok(count) if count > 0 => {
                            tracing::info!(count, "Expired stale command approvals");
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to expire stale approvals");
                        }
                        _ => {}
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "DB pool error in approval cleanup");
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
    let due = crate::db::schedules::get_due(&client).await?;

    if due.is_empty() {
        return Ok(());
    }

    tracing::info!(count = due.len(), "Processing due schedules");

    for schedule in due {
        let schedule_id = schedule.schedule_id;
        let schedule_name = schedule.name.as_deref().unwrap_or("unnamed");

        // ── Benchmark schedule: clone config and queue it ──────────
        if let Some(ref bench_config_id) = schedule.benchmark_config_id {
            match clone_and_queue_benchmark(&client, bench_config_id, schedule_name).await {
                Ok(new_config_id) => {
                    tracing::info!(
                        schedule_id = %schedule_id,
                        schedule_name = %schedule_name,
                        template_config_id = %bench_config_id,
                        new_config_id = %new_config_id,
                        "Cloned benchmark config from schedule"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        schedule_id = %schedule_id,
                        error = %e,
                        "Failed to clone benchmark config from schedule"
                    );
                }
            }
            // Update last_run_at and compute next_run_at
            let next = compute_next_run(&schedule.cron_expr);
            crate::db::schedules::mark_run(&client, &schedule_id, next).await?;
            continue;
        }

        // ── Regular job schedule ───────────────────────────────────
        let config = match &schedule.config {
            Some(c) => c.clone(),
            None => {
                tracing::warn!(schedule_id = %schedule_id, "Schedule has no config, skipping");
                // Compute next run and move on
                let next = compute_next_run(&schedule.cron_expr);
                crate::db::schedules::mark_run(&client, &schedule_id, next).await?;
                continue;
            }
        };

        // If auto_start_vm is set, start the deployment's VM first
        if schedule.auto_start_vm {
            if let Some(dep_id) = &schedule.deployment_id {
                tracing::info!(
                    schedule_id = %schedule_id,
                    deployment_id = %dep_id,
                    "Auto-starting VM for scheduled test"
                );
                if let Err(e) = start_deployment_vm(&client, dep_id).await {
                    tracing::error!(
                        schedule_id = %schedule_id,
                        deployment_id = %dep_id,
                        error = %e,
                        "Failed to auto-start VM"
                    );
                }
            }
        }

        // Create job from schedule config
        let project_id = schedule
            .project_id
            .unwrap_or_else(|| crate::auth::default_project_id().to_string());
        let job_id = crate::db::jobs::create(
            &client,
            &config,
            schedule.agent_id.as_ref(),
            None,
            &project_id,
        )
        .await?;

        tracing::info!(
            schedule_id = %schedule_id,
            schedule_name = %schedule_name,
            job_id = %job_id,
            "Created job from schedule"
        );

        // Try to dispatch to agent
        let agent_id = match schedule.agent_id {
            Some(id) => Some(id),
            None => state.agents.any_online_agent().await,
        };

        if let Some(aid) = agent_id {
            if let Ok(mut job_config) =
                serde_json::from_value::<networker_common::messages::JobConfig>(config.clone())
            {
                job_config.project_id = Some(project_id.clone());
                let msg = networker_common::messages::ControlMessage::JobAssign {
                    job_id,
                    config: Box::new(job_config),
                };
                if state.agents.send_to_agent(&aid, &msg).await.is_ok() {
                    crate::db::jobs::update_status(&client, &job_id, "assigned")
                        .await
                        .ok();
                    let _ = state.events_tx.send(
                        networker_common::messages::DashboardEvent::JobUpdate {
                            job_id,
                            status: "assigned".into(),
                            agent_id: Some(aid),
                            started_at: None,
                            finished_at: None,
                        },
                    );
                    tracing::info!(
                        schedule_id = %schedule_id,
                        job_id = %job_id,
                        agent_id = %aid,
                        "Dispatched scheduled job to agent"
                    );
                }
            }
        } else {
            tracing::warn!(
                schedule_id = %schedule_id,
                job_id = %job_id,
                "No online agent — scheduled job queued as pending"
            );
        }

        // If auto_stop_vm, spawn a watcher that stops the VM when the job completes
        if schedule.auto_stop_vm {
            if let Some(dep_id) = schedule.deployment_id {
                let state_clone = state.clone();
                tokio::spawn(async move {
                    watch_job_and_stop_vm(state_clone, job_id, dep_id).await;
                });
            }
        }

        // Update last_run_at and compute next_run_at
        let next = compute_next_run(&schedule.cron_expr);
        crate::db::schedules::mark_run(&client, &schedule_id, next).await?;

        tracing::info!(
            schedule_id = %schedule_id,
            next_run_at = ?next,
            "Schedule run recorded"
        );
    }

    Ok(())
}

/// Clone a benchmark config template and insert as queued for the worker to pick up.
async fn clone_and_queue_benchmark(
    client: &tokio_postgres::Client,
    template_config_id: &uuid::Uuid,
    schedule_name: &str,
) -> anyhow::Result<uuid::Uuid> {
    // Load the template config
    let template = crate::db::benchmark_configs::get(client, template_config_id)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!("Benchmark config template not found: {template_config_id}")
        })?;

    // Create a new config cloned from the template
    let run_name = format!(
        "{} (scheduled {})",
        schedule_name,
        chrono::Utc::now().format("%Y-%m-%d %H:%M")
    );

    let new_config_id = crate::db::benchmark_configs::create(
        client,
        &template.project_id,
        &run_name,
        template.template.as_deref(),
        &template.config_json,
        template.created_by.as_ref(),
        template.max_duration_secs,
        template.baseline_run_id.as_ref(),
        &template.benchmark_type,
    )
    .await?;

    // Clone testbeds from the template
    let testbeds =
        crate::db::benchmark_testbeds::list_for_config(client, template_config_id).await?;
    for testbed in &testbeds {
        crate::db::benchmark_testbeds::create(
            client,
            &new_config_id,
            &testbed.cloud,
            &testbed.region,
            &testbed.topology,
            &testbed.languages,
            testbed.vm_size.as_deref(),
            testbed.os.as_str(),
            &testbed.proxies,
            &testbed.tester_os,
            testbed.endpoint_ip.as_deref(),
        )
        .await?;
    }

    // Set to queued so the benchmark worker picks it up
    crate::db::benchmark_configs::update_status(client, &new_config_id, "queued", None).await?;

    Ok(new_config_id)
}

/// Start VMs associated with a deployment using cloud CLI tools.
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
                // Need instance ID — try to find from deployment IPs or name
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

/// Watch a job until completion, then stop the VM.
async fn watch_job_and_stop_vm(
    state: Arc<AppState>,
    job_id: uuid::Uuid,
    deployment_id: uuid::Uuid,
) {
    // Poll job status every 15s for up to 30 minutes
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;

        let client = match state.db.get().await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let job = match crate::db::jobs::get(&client, &job_id).await {
            Ok(Some(j)) => j,
            _ => continue,
        };

        match job.status.as_str() {
            "completed" | "failed" | "cancelled" => {
                tracing::info!(
                    job_id = %job_id,
                    deployment_id = %deployment_id,
                    status = %job.status,
                    "Job finished — stopping VM (auto_stop_vm)"
                );
                if let Err(e) = stop_deployment_vm(&client, &deployment_id).await {
                    tracing::error!(error = %e, "Failed to auto-stop VM");
                }
                return;
            }
            _ => {}
        }
    }

    tracing::warn!(
        job_id = %job_id,
        "Job did not complete within 30 minutes — stopping VM anyway"
    );
    if let Ok(client) = state.db.get().await {
        let _ = stop_deployment_vm(&client, &deployment_id).await;
    }
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

    // 1. Expire stale invites
    match crate::db::invites::expire_stale_invites(&client).await {
        Ok(n) if n > 0 => tracing::info!("Expired {n} stale workspace invites"),
        _ => {}
    }

    // 2. Find inactive workspaces (90 days, not protected, not suspended)
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
            continue; // Already warned
        }
        // Send warning email to all members
        if let Ok(members) = crate::db::projects::list_members(&client, &ws.project_id).await {
            for member in &members {
                let body = format!(
                    "Hi,\n\n\
                     Your workspace \"{}\" on AletheDash has had no activity for 90 days.\n\n\
                     It will be suspended in 30 days if no one logs in.\n\n\
                     Log in to keep your workspace active:\n{}\n\n\
                     — AletheDash",
                    ws.name, state.public_url
                );
                let _ = crate::email::send_email(
                    &member.email,
                    &format!("AletheDash — {} workspace will be suspended", ws.name),
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

    // 3. Find workspaces warned 30+ days ago -> auto-suspend
    let warned_ids =
        crate::db::workspace_warnings::warnings_older_than(&client, "inactivity_90d", 30)
            .await
            .unwrap_or_default();

    for pid in &warned_ids {
        // Check still inactive (no recent activity since warning)
        let still_inactive = crate::db::projects::find_inactive_workspaces(&client, 90)
            .await
            .map(|list| list.iter().any(|p| p.project_id == *pid))
            .unwrap_or(false);
        if still_inactive {
            let _ = crate::db::projects::suspend_project(&client, pid).await;
            tracing::info!(project_id = %pid, "Auto-suspended workspace due to inactivity");
        }
    }

    // 4. Warn system admins about workspaces approaching hard delete (360 days suspended)
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
        // Find system admins and email them
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
                 To prevent deletion, restore the workspace from the System Dashboard.\n\n\
                 — AletheDash",
                ws.name
            );
            let _ = crate::email::send_email(
                &email,
                &format!("AletheDash — {} permanent deletion in 5 days", ws.name),
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
        tracing::info!(workspace = %ws.name, "Sent hard-delete warning to system admins");
    }

    // 5. Hard delete workspaces suspended for 365+ days
    let to_delete = crate::db::projects::find_suspended_older_than(&client, 365)
        .await
        .unwrap_or_default();
    for ws in &to_delete {
        tracing::warn!(
            workspace = %ws.name,
            project_id = %ws.project_id,
            "Auto-deleting workspace (365 days suspended)"
        );
        let _ = crate::db::projects::hard_delete_project(&client, &ws.project_id).await;
    }

    // 6. Expire stale command approvals
    match crate::db::command_approvals::expire_stale(&client).await {
        Ok(n) if n > 0 => tracing::info!("Expired {n} stale command approvals"),
        _ => {}
    }
}

async fn run_health_checks(state: &Arc<AppState>) {
    // Check core DB connectivity
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

    // Check logs DB connectivity
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

    // Check core DB size
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
                    tracing::error!(error = %e, "Health check: DB query failed");
                    ("red", None, Some("Query error".into()))
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Health check: DB pool error");
            ("red", None, Some("Pool unavailable".into()))
        }
    };

    // Check logs DB size
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
                    tracing::error!(error = %e, "Health check: DB query failed");
                    ("red", None, Some("Query error".into()))
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Health check: DB pool error");
            ("red", None, Some("Pool unavailable".into()))
        }
    };

    // Check logs retention (oldest row in perf_log)
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
                    tracing::error!(error = %e, "Health check: DB query failed");
                    ("red", None, Some("Query error".into()))
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Health check: DB pool error");
            ("red", None, Some("Pool unavailable".into()))
        }
    };

    // Persist results to core DB
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

    if core_status.0 == "red" {
        tracing::warn!("Health check FAILED: core_db — {:?}", core_status.1);
    }
    if logs_status.0 == "red" {
        tracing::warn!("Health check FAILED: logs_db — {:?}", logs_status.1);
    }
}

/// Fail jobs stuck in "assigned" status whose agent is no longer online.
///
/// When a tester VM is deleted and recreated it gets a new agent_id, but jobs
/// assigned to the OLD agent_id stay "assigned" forever.  This watchdog marks
/// them failed so the UI reflects reality and the scheduler can re-queue work.
async fn reap_stale_assigned_jobs(state: &AppState) -> anyhow::Result<()> {
    use anyhow::Context;

    let client = state
        .db
        .get()
        .await
        .context("DB pool error in stale-job reaper")?;

    let stale = crate::db::jobs::find_stale_assigned(&client, 120)
        .await
        .context("Failed to query stale assigned jobs")?;

    for (job_id, agent_id) in stale {
        // If the agent is still connected, the job may just be slow to ACK — skip it.
        if let Some(ref aid) = agent_id {
            if state.agents.is_agent_online(aid).await {
                continue;
            }
        }

        let error_msg = "Agent disconnected — tester may have been deleted or restarted";

        crate::db::jobs::set_error(&client, &job_id, error_msg)
            .await
            .context("Failed to fail stale assigned job")?;

        let _ = state
            .events_tx
            .send(networker_common::messages::DashboardEvent::JobUpdate {
                job_id,
                status: "failed".into(),
                agent_id,
                started_at: None,
                finished_at: Some(chrono::Utc::now()),
            });

        tracing::warn!(
            job_id = %job_id,
            agent_id = ?agent_id,
            "Reaped stale assigned job — agent offline"
        );
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::compute_next_run;

    /// Valid cron expressions produce a future timestamp.
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

    /// Invalid or malformed expressions return None.
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

    /// Edge cases: valid syntax but no future occurrences.
    mod cron_edge {
        use super::*;

        #[test]
        fn past_year_returns_none() {
            assert!(compute_next_run("0 0 0 1 1 * 2000").is_none());
        }
    }

    /// Scheduler tick flow: next_run_at is always in the future after mark_run.
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

/// Stop VMs associated with a deployment.
async fn stop_deployment_vm(
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
                tracing::info!(provider = "azure", vm = %vm_name, "Stopping VM (auto_stop_vm)");
                let _ = tokio::process::Command::new("az")
                    .args([
                        "vm",
                        "deallocate",
                        "--resource-group",
                        rg,
                        "--name",
                        &vm_name,
                        "--no-wait",
                    ])
                    .output()
                    .await;
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
                    tracing::info!(provider = "aws", instance_id = %instance_id, "Stopping VM");
                    let _ = tokio::process::Command::new("aws")
                        .args([
                            "ec2",
                            "stop-instances",
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
                    tracing::info!(provider = "gcp", vm = %vm_name, "Stopping VM");
                    let _ = tokio::process::Command::new("gcloud")
                        .args(["compute", "instances", "stop", &vm_name, "--zone", zone])
                        .output()
                        .await;
                }
            }
            _ => {}
        }
    }

    Ok(())
}
