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

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;

            if let Err(e) = tick(&state).await {
                tracing::error!(error = %e, "Scheduler tick failed");
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
            .unwrap_or(crate::auth::DEFAULT_PROJECT_ID);
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
            if let Ok(job_config) =
                serde_json::from_value::<networker_common::messages::JobConfig>(config.clone())
            {
                let msg = networker_common::messages::ControlMessage::JobAssign {
                    job_id,
                    config: job_config,
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

/// Start VMs associated with a deployment using cloud CLI tools.
async fn start_deployment_vm(
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
