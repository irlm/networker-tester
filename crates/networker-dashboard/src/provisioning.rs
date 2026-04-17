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
    let _ = try_dispatch_run(&state.agents, run, cfg).await;
}

/// Outcome of a single dispatch attempt. Used by the redispatcher to decide
/// whether to log progress or back off.
#[derive(Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// `AssignRun` was encoded and handed to an agent's outbound channel.
    Sent { agent_id: Uuid },
    /// No agents are currently registered in the hub. Run stays `queued`;
    /// the redispatcher will retry on the next tick.
    NoAgent,
    /// The hub found an agent but the send failed (channel full, closed, or
    /// encoding failure). Treated as non-fatal — the redispatcher retries.
    SendFailed { agent_id: Uuid, error: String },
}

/// Trait abstraction over `ws::agent_hub::AgentHub` so tests can exercise the
/// dispatch logic without spinning up a real hub / `AppState`.
///
/// Separate from `agent_dispatch::AgentSender` because we also need to query
/// the registry for any online agent — the existing trait only exposes
/// `send_to_agent`.
pub trait RunDispatcher: Send + Sync {
    fn any_online_agent(&self) -> impl std::future::Future<Output = Option<Uuid>> + Send;
    fn send_to_agent(
        &self,
        agent_id: &Uuid,
        msg: &ControlMessage,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

impl RunDispatcher for crate::ws::agent_hub::AgentHub {
    fn any_online_agent(&self) -> impl std::future::Future<Output = Option<Uuid>> + Send {
        crate::ws::agent_hub::AgentHub::any_online_agent(self)
    }
    fn send_to_agent(
        &self,
        agent_id: &Uuid,
        msg: &ControlMessage,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        crate::ws::agent_hub::AgentHub::send_to_agent(self, agent_id, msg)
    }
}

/// Core dispatch step: pick any online agent, send `AssignRun`, report
/// outcome. Shared by the inline launch path and the periodic queued-run
/// redispatcher in `scheduler.rs`.
///
/// Generic over `RunDispatcher` so unit tests can record every dispatch
/// attempt without constructing a full `AppState`.
pub async fn try_dispatch_run<D: RunDispatcher>(
    hub: &D,
    run: &TestRun,
    cfg: &TestConfig,
) -> DispatchOutcome {
    match hub.any_online_agent().await {
        None => {
            tracing::info!(
                run_id = %run.id,
                "No online agent — run remains queued for later dispatch"
            );
            DispatchOutcome::NoAgent
        }
        Some(agent_id) => {
            let msg = ControlMessage::AssignRun {
                run: Box::new(run.clone()),
                config: Box::new(cfg.clone()),
            };
            match hub.send_to_agent(&agent_id, &msg).await {
                Ok(()) => {
                    tracing::info!(
                        run_id = %run.id,
                        %agent_id,
                        endpoint_kind = cfg.endpoint_kind(),
                        "Dispatched run to agent"
                    );
                    DispatchOutcome::Sent { agent_id }
                }
                Err(e) => {
                    let error = e.to_string();
                    tracing::warn!(
                        run_id = %run.id,
                        %agent_id,
                        error = %error,
                        "Dispatch to agent failed — will retry"
                    );
                    DispatchOutcome::SendFailed { agent_id, error }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use networker_common::{
        CaptureMode, EndpointRef, Mode, RunStatus, TestConfig, TestRun, Workload,
    };
    use std::collections::HashMap;
    use tokio::sync::Mutex;

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

    // ─────────────────────────────────────────────────────────────────────
    // Mock dispatcher for `try_dispatch_run` unit tests.
    // ─────────────────────────────────────────────────────────────────────

    struct MockDispatcher {
        agent: Option<Uuid>,
        fail_send: bool,
        sent: Mutex<HashMap<Uuid, Vec<ControlMessage>>>,
    }

    impl MockDispatcher {
        fn with_agent() -> Self {
            Self {
                agent: Some(Uuid::new_v4()),
                fail_send: false,
                sent: Mutex::new(HashMap::new()),
            }
        }
        fn empty() -> Self {
            Self {
                agent: None,
                fail_send: false,
                sent: Mutex::new(HashMap::new()),
            }
        }
        fn with_broken_agent() -> Self {
            Self {
                agent: Some(Uuid::new_v4()),
                fail_send: true,
                sent: Mutex::new(HashMap::new()),
            }
        }
    }

    impl RunDispatcher for MockDispatcher {
        async fn any_online_agent(&self) -> Option<Uuid> {
            self.agent
        }
        async fn send_to_agent(&self, agent_id: &Uuid, msg: &ControlMessage) -> anyhow::Result<()> {
            if self.fail_send {
                anyhow::bail!("mock send failure");
            }
            self.sent
                .lock()
                .await
                .entry(*agent_id)
                .or_default()
                .push(msg.clone());
            Ok(())
        }
    }

    /// Build a probe-style `TestConfig` — the exact shape the DiagnosticsPage
    /// launches: `EndpointRef::Network` with a user-entered host and no
    /// deployed target.
    fn probe_config() -> TestConfig {
        TestConfig {
            id: Uuid::new_v4(),
            project_id: "projtestabc001".to_string(),
            name: "Diag: example.com".into(),
            description: None,
            endpoint: EndpointRef::Network {
                host: "example.com".into(),
                port: None,
            },
            workload: Workload {
                modes: vec![Mode::Dns, Mode::Tcp, Mode::Tls],
                runs: 1,
                concurrency: 1,
                timeout_ms: 5000,
                payload_sizes: vec![],
                capture_mode: CaptureMode::HeadersOnly,
            },
            methodology: None,
            baseline_run_id: None,
            max_duration_secs: 900,
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn queued_run(cfg: &TestConfig) -> TestRun {
        TestRun {
            id: Uuid::new_v4(),
            test_config_id: cfg.id,
            project_id: cfg.project_id.clone(),
            status: RunStatus::Queued,
            started_at: None,
            finished_at: None,
            success_count: 0,
            failure_count: 0,
            error_message: None,
            artifact_id: None,
            tester_id: None,
            worker_id: None,
            last_heartbeat: None,
            created_at: Utc::now(),
            comparison_group_id: None,
        }
    }

    /// Core regression for the prod bug: a probe-style TestConfig (Network
    /// endpoint without a deployed target) MUST be fanned out to any online
    /// agent, just like a regular network test. Previously only runs with a
    /// target_id were being dispatched; the diagnostic probe path was left
    /// stranded in `queued` forever.
    #[tokio::test]
    async fn probe_run_is_dispatched_when_an_agent_is_online() {
        let hub = MockDispatcher::with_agent();
        let cfg = probe_config();
        let run = queued_run(&cfg);

        let outcome = try_dispatch_run(&hub, &run, &cfg).await;

        // Must report Sent — not NoAgent / SendFailed.
        let agent_id = match outcome {
            DispatchOutcome::Sent { agent_id } => agent_id,
            other => panic!("expected Sent, got {other:?}"),
        };
        assert_eq!(Some(agent_id), hub.agent);

        // Exactly one AssignRun envelope must have been sent carrying the
        // probe run's id and the probe config's Network endpoint.
        let sent = hub.sent.lock().await;
        let envelopes = sent.get(&agent_id).expect("agent received envelope");
        assert_eq!(envelopes.len(), 1);
        match &envelopes[0] {
            ControlMessage::AssignRun { run: r, config: c } => {
                assert_eq!(r.id, run.id);
                assert_eq!(c.id, cfg.id);
                assert_eq!(c.endpoint_kind(), "network");
            }
            other => panic!("expected AssignRun, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_agent_leaves_run_queued_reported_as_noagent() {
        let hub = MockDispatcher::empty();
        let cfg = probe_config();
        let run = queued_run(&cfg);
        let outcome = try_dispatch_run(&hub, &run, &cfg).await;
        assert_eq!(outcome, DispatchOutcome::NoAgent);
        assert!(hub.sent.lock().await.is_empty());
    }

    #[tokio::test]
    async fn send_failure_is_non_fatal_and_reports_send_failed() {
        let hub = MockDispatcher::with_broken_agent();
        let cfg = probe_config();
        let run = queued_run(&cfg);
        let outcome = try_dispatch_run(&hub, &run, &cfg).await;
        match outcome {
            DispatchOutcome::SendFailed { agent_id, error } => {
                assert_eq!(Some(agent_id), hub.agent);
                assert!(error.contains("mock send failure"));
            }
            other => panic!("expected SendFailed, got {other:?}"),
        }
    }
}
