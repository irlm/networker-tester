//! Provisioning dispatch helper.
//!
//! The entry point the REST launch handlers + scheduler call instead of
//! directly sending `ControlMessage::AssignRun`. If the config's endpoint is
//! `Pending`, this starts a deployment, marks the run `provisioning`, and
//! links the deployment id — the background orchestrator in
//! [`crate::benchmark_worker`] watches those runs and dispatches them once
//! the deployment completes.
//!
//! For all other endpoint kinds this function falls through to best-effort
//! immediate dispatch (same as the old inline code).

use std::sync::Arc;

use networker_common::messages::ControlMessage;
use networker_common::{EndpointRef, TestConfig, TestRun};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;

/// Dispatch `run` or kick it into provisioning. Returns `Ok(())` either way —
/// the caller doesn't need to know which path was taken.
pub async fn dispatch_or_provision(
    state: &Arc<AppState>,
    run: &TestRun,
    cfg: &TestConfig,
) -> anyhow::Result<()> {
    if let EndpointRef::Pending { .. } = &cfg.endpoint {
        kick_provisioning(state, run, cfg).await
    } else {
        best_effort_dispatch(state, run, cfg).await;
        Ok(())
    }
}

/// Start a deployment for this run's `Pending` endpoint and transition the
/// run to `provisioning`. The orchestrator takes over from here.
async fn kick_provisioning(
    state: &Arc<AppState>,
    run: &TestRun,
    cfg: &TestConfig,
) -> anyhow::Result<()> {
    let EndpointRef::Pending {
        cloud_account_id,
        region,
        vm_size,
        os,
        proxy_stack,
        topology: _,
        language,
    } = &cfg.endpoint
    else {
        // Caller already checked — this is defensive.
        anyhow::bail!("kick_provisioning called with non-Pending endpoint");
    };

    let deploy_json = build_deploy_json(
        cloud_account_id,
        region,
        vm_size,
        os,
        proxy_stack,
        language.as_deref(),
        &cfg.name,
    );
    let dep_name = format!("auto-{}-{}", cfg.name, short_id(&run.id));

    let client = state.db.get().await?;
    let deployment_id = crate::db::deployments::create(
        &client,
        &dep_name,
        &deploy_json,
        None,
        cfg.created_by.as_ref(),
        &cfg.project_id,
    )
    .await?;

    crate::db::test_runs::set_provisioning(&client, &run.id, &deployment_id).await?;

    tracing::info!(
        run_id = %run.id,
        deployment_id = %deployment_id,
        config_name = %cfg.name,
        proxy_stack,
        region,
        "Provisioning kicked off for Pending endpoint"
    );

    // Spawn the actual deploy (same pattern as `api::deployments::create`).
    let events_tx = state.events_tx.clone();
    let db_pool = Arc::new(state.db.clone());
    let deploy_json_moved = deploy_json.clone();
    tokio::spawn(async move {
        match crate::deploy::runner::run_deployment(
            deployment_id,
            &deploy_json_moved,
            events_tx,
            db_pool,
        )
        .await
        {
            Ok(ips) => tracing::info!(
                deployment_id = %deployment_id,
                endpoint_ips = ?ips,
                "Auto-provisioning deployment completed"
            ),
            Err(e) => tracing::error!(
                deployment_id = %deployment_id,
                error = %e,
                "Auto-provisioning deployment failed"
            ),
        }
    });

    Ok(())
}

/// Build the deploy.json document the runner will hand to `install.sh --deploy`.
///
/// Only the local/cloud provider keyed blocks are filled; we currently lean
/// entirely on the cloud account's metadata. Extend this when the install.sh
/// schema grows more required fields.
fn build_deploy_json(
    cloud_account_id: &Uuid,
    region: &str,
    vm_size: &str,
    os: &str,
    proxy_stack: &str,
    language: Option<&str>,
    cfg_name: &str,
) -> serde_json::Value {
    // Derive the provider from the vm_size prefix is fragile; instead,
    // `install.sh` supports an explicit `provider` per endpoint. We don't
    // know the provider here without a DB lookup, so we encode a neutral
    // shape and let the runner's validation resolve via cloud_account_id.
    //
    // TODO(irlm): inline the provider string by passing it through the
    // `EndpointRef::Pending` variant. Today the provider comes from the
    // cloud_account row; install.sh looks it up from `cloud_account_id`.
    let suffix = short_id_from_name(cfg_name);
    let vm_label = sanitize_vm_label(&format!("nwk-auto-{suffix}"));

    let mut endpoint = json!({
        // `provider: "auto"` tells install.sh to resolve the real
        // provider from cloud_account_id at deploy time.
        "provider": "auto",
        "label": cfg_name,
        "auto": {
            "region": region,
            "vm_size": vm_size,
            "os": os,
            "vm_name": vm_label,
        },
        "http_stacks": [proxy_stack],
    });
    if let Some(lang) = language {
        endpoint["languages"] = json!([lang]);
    }

    json!({
        "version": 1,
        "tester": { "provider": "local" },
        "cloud_account_id": cloud_account_id.to_string(),
        "endpoints": [endpoint],
        "tests": { "run_tests": false }
    })
}

fn short_id(id: &Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn short_id_from_name(name: &str) -> String {
    let slug: String = name
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c == ' ' || c == '-' || c == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect();
    slug.chars().take(8).collect::<String>()
}

fn sanitize_vm_label(raw: &str) -> String {
    // Windows NetBIOS (install.sh's strictest constraint): ≤15 chars,
    // alphanumeric + `-`.
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(15)
        .collect()
}

/// Non-Pending endpoint — send to any online agent. Same semantics as the
/// inline code that used to live in every launch handler.
async fn best_effort_dispatch(state: &Arc<AppState>, run: &TestRun, cfg: &TestConfig) {
    if let Some(agent_id) = state.agents.any_online_agent().await {
        let msg = ControlMessage::AssignRun {
            run: Box::new(run.clone()),
            config: Box::new(cfg.clone()),
        };
        let _ = state.agents.send_to_agent(&agent_id, &msg).await;
    } else {
        tracing::info!(
            run_id = %run.id,
            "No online agent — run remains queued for later dispatch"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_label_truncates_to_15() {
        let s = sanitize_vm_label("nwk-auto-abcdefghijklmnopqrs");
        assert!(s.len() <= 15, "got {}: {}", s.len(), s);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn vm_label_strips_invalid_chars() {
        let s = sanitize_vm_label("nwk auto/bad");
        assert_eq!(s, "nwkautobad");
    }

    #[test]
    fn short_id_from_name_lowercases_and_strips() {
        assert_eq!(short_id_from_name("My Test/Run!"), "my-test");
    }
}
