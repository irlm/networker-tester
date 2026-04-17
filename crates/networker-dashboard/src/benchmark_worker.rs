//! Provisioning orchestrator (v0.28.1).
//!
//! The old `benchmark_worker` polled `benchmark_config` rows and spawned
//! orchestrator processes locally. In v0.28.0 all test execution moved onto
//! agent dispatch and the worker became a no-op stub.
//!
//! v0.28.1 reuses this task for a different job: driving `EndpointRef::Pending`
//! configs through their provisioning lifecycle. When the REST launch handlers
//! encounter a `Pending` endpoint they kick off a deployment, mark the run
//! `provisioning`, and link `provisioning_deployment_id` — this loop watches
//! those runs, rewrites the config's endpoint once the deployment succeeds,
//! and hands the run off to the normal dispatch path.
//!
//! Kept under the `benchmark_worker` name so `main.rs` doesn't need rewiring.

use std::sync::Arc;
use std::time::Duration;

use networker_common::messages::ControlMessage;
use networker_common::test_config::proxy_https_port;
use networker_common::{EndpointRef, RunStatus, TestRun};

use crate::AppState;

/// Spawn the orchestrator. Ticks every 5s; one tick handles all currently
/// provisioning runs in FIFO-ish order.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Small startup delay to let migrations finish and the DB pool warm up.
        tokio::time::sleep(Duration::from_secs(2)).await;
        tracing::info!("Provisioning orchestrator started");

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Err(e) = tick(&state).await {
                tracing::error!(error = %e, "Provisioning orchestrator tick failed");
            }
        }
    });
}

async fn tick(state: &Arc<AppState>) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let pairs = crate::db::test_runs::list_provisioning(&client).await?;
    if pairs.is_empty() {
        return Ok(());
    }

    for (run, deployment_id) in pairs {
        if let Err(e) = handle_run(state, &run, &deployment_id).await {
            tracing::error!(
                run_id = %run.id,
                deployment_id = %deployment_id,
                error = %e,
                "Orchestrator failed to handle provisioning run"
            );
        }
    }
    Ok(())
}

async fn handle_run(
    state: &Arc<AppState>,
    run: &TestRun,
    deployment_id: &uuid::Uuid,
) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let deployment = crate::db::deployments::get(&client, deployment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("deployment {deployment_id} vanished"))?;

    match deployment.status.as_str() {
        "completed" => promote(state, run, &deployment).await,
        "failed" => {
            let msg = deployment
                .error_message
                .as_deref()
                .unwrap_or("deployment failed");
            crate::db::test_runs::set_error(
                &client,
                &run.id,
                &format!("Provisioning failed: {msg}"),
            )
            .await?;
            tracing::warn!(
                run_id = %run.id,
                deployment_id = %deployment_id,
                error = %msg,
                "Run transitioned to failed: provisioning deployment failed"
            );
            Ok(())
        }
        // Still running / pending — leave it alone; we'll check again next tick.
        _ => Ok(()),
    }
}

/// Deployment completed — rewrite the config's endpoint from `Pending` to
/// `Network{host, port}` and transition the run into the normal dispatch path.
async fn promote(
    state: &Arc<AppState>,
    run: &TestRun,
    deployment: &crate::db::deployments::DeploymentRow,
) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let cfg = crate::db::test_configs::get(&client, &run.test_config_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("test_config {} vanished", run.test_config_id))?;

    let EndpointRef::Pending { proxy_stack, .. } = &cfg.endpoint else {
        // Someone already rewrote it — just move the run along.
        crate::db::test_runs::update_status(&client, &run.id, RunStatus::Queued).await?;
        return Ok(());
    };

    let host = first_endpoint_host(&deployment.endpoint_ips).ok_or_else(|| {
        anyhow::anyhow!(
            "deployment {} completed but no endpoint IPs were captured",
            deployment.deployment_id
        )
    })?;
    let port = proxy_https_port(proxy_stack);

    let new_endpoint = EndpointRef::Network {
        host: host.clone(),
        port: Some(port),
    };

    crate::db::test_configs::update_endpoint(&client, &cfg.id, &new_endpoint).await?;
    crate::db::test_runs::update_status(&client, &run.id, RunStatus::Queued).await?;

    tracing::info!(
        run_id = %run.id,
        deployment_id = %deployment.deployment_id,
        host,
        port,
        proxy_stack,
        "Provisioning complete — endpoint rewritten, run queued"
    );

    // Best-effort immediate dispatch. The scheduler also polls queued runs, so
    // if no agent is online now the run stays queued and gets picked up later.
    if let Some(agent_id) = state.agents.any_online_agent().await {
        // Re-read the updated config so the agent gets the rewritten endpoint.
        let updated_cfg = crate::db::test_configs::get(&client, &cfg.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("config vanished post-rewrite"))?;
        let updated_run = crate::db::test_runs::get(&client, &run.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("run vanished post-status-update"))?;
        let msg = ControlMessage::AssignRun {
            run: Box::new(updated_run),
            config: Box::new(updated_cfg),
        };
        if state.agents.send_to_agent(&agent_id, &msg).await.is_ok() {
            tracing::info!(
                run_id = %run.id,
                agent_id = %agent_id,
                "Dispatched provisioned run"
            );
        }
    }

    Ok(())
}

/// Pull the first usable host (FQDN preferred, bare IP otherwise) out of the
/// deployment's captured endpoint list. The deploy runner stores either form.
fn first_endpoint_host(endpoint_ips: &Option<serde_json::Value>) -> Option<String> {
    let arr = endpoint_ips.as_ref()?.as_array()?;
    for v in arr {
        if let Some(s) = v.as_str() {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_first_host_from_jsonb_array() {
        let v = Some(json!(["first.example.com", "second.example.com"]));
        assert_eq!(
            first_endpoint_host(&v).as_deref(),
            Some("first.example.com")
        );
    }

    #[test]
    fn empty_or_missing_returns_none() {
        assert_eq!(first_endpoint_host(&None), None);
        assert_eq!(first_endpoint_host(&Some(json!([]))), None);
        assert_eq!(first_endpoint_host(&Some(json!(["   "]))), None);
    }
}
