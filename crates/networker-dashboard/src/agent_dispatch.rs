//! Dashboard → agent command dispatch service.
//!
//! Flow:
//!   1. INSERT pending row into `agent_command`.
//!   2. Mint a per-command JWT (short-lived, verb-scoped).
//!   3. Push `ControlMessage::Command(envelope)` over the agent's WS.
//!   4. Return the envelope to the caller so it can poll or subscribe.
//!
//! The WS push and DB write are deliberately split from the outer
//! `dispatch_command` wrapper so unit tests can exercise the logic without
//! constructing a full `AppState`.
//!
//! Ingestion of `AgentMessage::CommandLog` and `AgentMessage::CommandResult`
//! lives in `handle_command_log` / `handle_command_result`. These are called
//! from `ws::agent_hub` when the agent socket receives the matching frame.
//!
//! `dispatch_command` and its plumbing are consumed by the REST layer in a
//! later task of the plan; silence the dead-code lint in the meantime.
#![allow(dead_code)]

use anyhow::{Context, Result};
use networker_common::messages::{
    AgentCommand, AgentCommandLog, AgentCommandResult, ControlMessage,
};
use std::sync::Arc;
use tokio_postgres::Client;
use uuid::Uuid;

use crate::auth::mint_command_token;
use crate::db::agent_commands;
use crate::AppState;

/// Minimum token lifetime, in seconds, regardless of the requested command
/// timeout. Keeps short-shot tokens above the validator's 60 s near-expiry
/// guard.
const MIN_TOKEN_LIFETIME_SECS: u64 = 300;
/// Seconds of slack added on top of `timeout_secs` so the agent has room to
/// respond (and the caller can retry mid-flight without minting a new token).
const TOKEN_LIFETIME_BUFFER_SECS: u64 = 60;

/// Abstraction over the agent WS registry. The real implementation is
/// `ws::agent_hub::AgentHub`; tests provide a mock that records messages.
///
/// Uses native async-fn-in-trait (stable as of Rust 1.75). We only use this
/// trait with static generics (`impl AgentSender` / `H: AgentSender`), never
/// via `dyn`, so we don't need to worry about the `?Send` bound.
pub trait AgentSender: Send + Sync {
    fn send_to_agent(
        &self,
        agent_id: &Uuid,
        msg: &ControlMessage,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

impl AgentSender for crate::ws::agent_hub::AgentHub {
    fn send_to_agent(
        &self,
        agent_id: &Uuid,
        msg: &ControlMessage,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        crate::ws::agent_hub::AgentHub::send_to_agent(self, agent_id, msg)
    }
}

/// Public entry point: dispatch a typed command via `AppState`.
///
/// On error, the agent_command row's status is updated to `error` with
/// `error_message` set to the bail reason so REST consumers can still see
/// that a dispatch attempt happened.
pub async fn dispatch_command(
    state: &Arc<AppState>,
    agent_id: Uuid,
    verb: &str,
    args: serde_json::Value,
    config_id: Option<Uuid>,
    timeout_secs: u64,
    created_by: Option<Uuid>,
) -> Result<AgentCommand> {
    let client = state
        .db
        .get()
        .await
        .context("dispatch_command: acquire db client")?;
    dispatch_command_inner(
        &client,
        &state.agents,
        &state.jwt_secret,
        agent_id,
        verb,
        args,
        config_id,
        timeout_secs,
        created_by,
    )
    .await
}

/// Core dispatch logic — DB + hub are injected so tests can mock both.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_command_inner<H: AgentSender>(
    client: &Client,
    hub: &H,
    jwt_secret: &str,
    agent_id: Uuid,
    verb: &str,
    args: serde_json::Value,
    config_id: Option<Uuid>,
    timeout_secs: u64,
    created_by: Option<Uuid>,
) -> Result<AgentCommand> {
    let command_id = Uuid::new_v4();

    agent_commands::insert_pending(
        client,
        &command_id,
        &agent_id,
        config_id.as_ref(),
        verb,
        &args,
        created_by.as_ref(),
    )
    .await
    .context("insert agent_command row")?;

    let lifetime = (timeout_secs + TOKEN_LIFETIME_BUFFER_SECS).max(MIN_TOKEN_LIFETIME_SECS);
    let token = mint_command_token(
        jwt_secret,
        agent_id,
        config_id,
        &[verb.to_string()],
        lifetime,
    )
    .context("mint_command_token")?;

    let envelope = AgentCommand {
        command_id,
        config_id,
        token,
        verb: verb.to_string(),
        args,
        timeout_secs,
    };

    let msg = ControlMessage::Command(envelope.clone());
    if let Err(e) = hub.send_to_agent(&agent_id, &msg).await {
        let reason = format!("agent_not_connected: {e}");
        // Best-effort: record the dispatch failure on the row so the REST
        // layer can surface it. Any DB error here is swallowed because the
        // dispatch error is the primary signal.
        if let Err(db_err) = agent_commands::mark_dispatch_error(client, &command_id, &reason).await
        {
            tracing::warn!(
                %command_id,
                %agent_id,
                error = %db_err,
                "failed to stamp dispatch error on agent_command row"
            );
        }
        anyhow::bail!("dispatch to agent {agent_id} failed: {e}");
    }

    tracing::info!(%command_id, %agent_id, verb, "dispatched agent command");
    Ok(envelope)
}

/// Handle a `CommandLog` frame from an agent. Writes a line to `service_log`
/// and stamps `started_at` on the command row (lazy, idempotent) — the first
/// log line is the earliest point at which we know the command has actually
/// started running.
pub async fn handle_command_log(state: &Arc<AppState>, log: AgentCommandLog) -> Result<()> {
    // Logs DB write (best-effort — never block command processing).
    match state.logs_db.get().await {
        Ok(client) => {
            let stream_label = match log.stream {
                networker_common::messages::LogStream::Stdout => "stdout",
                networker_common::messages::LogStream::Stderr => "stderr",
            };
            let fields = serde_json::json!({
                "command_id": log.command_id,
                "stream": stream_label,
            });
            // level 3 = INFO (see networker-log level mapping).
            if let Err(e) = client
                .execute(
                    "INSERT INTO service_log (service, level, message, fields) \
                     VALUES ('agent-command', 3, $1, $2)",
                    &[
                        &log.line as &(dyn tokio_postgres::types::ToSql + Sync),
                        &fields,
                    ],
                )
                .await
            {
                tracing::warn!(command_id = %log.command_id, error = %e, "service_log insert failed");
            }
        }
        Err(e) => {
            tracing::warn!(
                command_id = %log.command_id,
                error = %e,
                "logs_db pool unavailable while ingesting command log"
            );
        }
    }

    // Lazy `started_at` stamp on the command row.
    let client = state
        .db
        .get()
        .await
        .context("handle_command_log: acquire db client")?;
    agent_commands::mark_started(&client, &log.command_id)
        .await
        .context("mark_started")?;

    Ok(())
}

/// Handle a `CommandResult` frame from an agent. Marks the row terminal and
/// fires a structured tracing event for ops dashboards.
pub async fn handle_command_result(
    state: &Arc<AppState>,
    result: AgentCommandResult,
) -> Result<()> {
    let status_str = agent_commands::command_status_str(&result.status);
    let client = state
        .db
        .get()
        .await
        .context("handle_command_result: acquire db client")?;
    agent_commands::mark_finished(
        &client,
        &result.command_id,
        status_str,
        result.result.as_ref(),
        result.error.as_deref(),
    )
    .await
    .context("mark_finished")?;

    tracing::info!(
        command_id = %result.command_id,
        status = status_str,
        duration_ms = result.duration_ms,
        "agent command completed"
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use networker_common::messages::ControlMessage;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    /// Minimal in-memory hub for unit tests. Records every send.
    struct MockHub {
        /// agent_id → Vec<ControlMessage>
        pub sent: Mutex<HashMap<Uuid, Vec<ControlMessage>>>,
        /// When true, every send attempt fails (simulating "agent not connected").
        pub fail: bool,
    }

    impl MockHub {
        fn new() -> Self {
            Self {
                sent: Mutex::new(HashMap::new()),
                fail: false,
            }
        }
        fn always_fail() -> Self {
            Self {
                sent: Mutex::new(HashMap::new()),
                fail: true,
            }
        }
    }

    impl AgentSender for MockHub {
        async fn send_to_agent(&self, agent_id: &Uuid, msg: &ControlMessage) -> Result<()> {
            if self.fail {
                anyhow::bail!("agent {agent_id} not connected (mock)");
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

    #[tokio::test]
    async fn mock_hub_records_sent_messages() {
        let hub = MockHub::new();
        let agent = Uuid::new_v4();
        let cmd = AgentCommand {
            command_id: Uuid::new_v4(),
            config_id: None,
            token: "t".into(),
            verb: "health".into(),
            args: serde_json::json!({}),
            timeout_secs: 30,
        };
        hub.send_to_agent(&agent, &ControlMessage::Command(cmd.clone()))
            .await
            .unwrap();
        let sent = hub.sent.lock().await;
        assert_eq!(sent.get(&agent).map(|v| v.len()), Some(1));
    }

    #[tokio::test]
    async fn mock_hub_propagates_send_failure() {
        let hub = MockHub::always_fail();
        let agent = Uuid::new_v4();
        let msg = ControlMessage::Welcome {
            agent_id: agent,
            agent_name: "x".into(),
        };
        assert!(hub.send_to_agent(&agent, &msg).await.is_err());
    }

    #[test]
    fn token_lifetime_respects_floor() {
        // Short timeouts still produce a >= MIN_TOKEN_LIFETIME_SECS token.
        let lifetime = (5u64 + TOKEN_LIFETIME_BUFFER_SECS).max(MIN_TOKEN_LIFETIME_SECS);
        assert_eq!(lifetime, MIN_TOKEN_LIFETIME_SECS);
        // Long timeouts extend past the floor.
        let lifetime = (600u64 + TOKEN_LIFETIME_BUFFER_SECS).max(MIN_TOKEN_LIFETIME_SECS);
        assert_eq!(lifetime, 660);
    }

    // ── DB-backed integration tests (opt-in) ─────────────────────────────
    //
    // These exercise the real INSERT/UPDATE paths against Postgres. They
    // require a live dashboard DB with the V033 migration applied; they
    // skip when `DASHBOARD_DB_URL` is unset so `cargo test --include-ignored`
    // in CI does not panic.

    #[cfg(test)]
    async fn connect_test_db() -> Option<(tokio_postgres::Client, Uuid)> {
        use tokio_postgres::NoTls;
        let url = std::env::var("DASHBOARD_DB_URL").ok()?;
        let (client, conn) = tokio_postgres::connect(&url, NoTls).await.ok()?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        // Find/create a project + agent so the FK constraint is satisfied.
        let project_id: String = match client
            .query_opt(
                "SELECT project_id FROM project ORDER BY created_at LIMIT 1",
                &[],
            )
            .await
            .ok()
            .flatten()
        {
            Some(row) => row.get(0),
            None => return None,
        };
        let agent_id = Uuid::new_v4();
        let api_key = format!("test-{}", agent_id);
        let name = format!("agent-dispatch-test-{}", &agent_id.to_string()[..8]);
        client
            .execute(
                "INSERT INTO agent (agent_id, project_id, name, api_key, status) \
                 VALUES ($1, $2, $3, $4, 'offline')",
                &[&agent_id, &project_id, &name, &api_key],
            )
            .await
            .ok()?;
        Some((client, agent_id))
    }

    #[tokio::test]
    #[ignore]
    async fn dispatch_inserts_pending_row_and_sends_envelope() -> anyhow::Result<()> {
        let Some((client, agent_id)) = connect_test_db().await else {
            eprintln!("SKIP: DASHBOARD_DB_URL not set or no project row present");
            return Ok(());
        };
        let hub = MockHub::new();
        let env = dispatch_command_inner(
            &client,
            &hub,
            "test-secret",
            agent_id,
            "health",
            serde_json::json!({}),
            None,
            30,
            None,
        )
        .await?;
        assert_eq!(env.verb, "health");

        let row = agent_commands::fetch_by_id(&client, &env.command_id)
            .await?
            .expect("row inserted");
        assert_eq!(row.status, "pending");
        assert_eq!(row.agent_id, agent_id);

        // Hub received exactly one Command envelope.
        let sent = hub.sent.lock().await;
        assert_eq!(sent.get(&agent_id).map(|v| v.len()), Some(1));
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn dispatch_marks_error_when_agent_not_connected() -> anyhow::Result<()> {
        let Some((client, agent_id)) = connect_test_db().await else {
            eprintln!("SKIP: DASHBOARD_DB_URL not set");
            return Ok(());
        };
        let hub = MockHub::always_fail();
        let err = dispatch_command_inner(
            &client,
            &hub,
            "test-secret",
            agent_id,
            "health",
            serde_json::json!({}),
            None,
            30,
            None,
        )
        .await;
        assert!(err.is_err());

        // The row should exist with status='error'.
        let row = client
            .query_opt(
                "SELECT status, error_message FROM agent_command \
                 WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
                &[&agent_id],
            )
            .await?
            .expect("row exists");
        let status: String = row.get("status");
        let err_msg: Option<String> = row.get("error_message");
        assert_eq!(status, "error");
        assert!(err_msg.unwrap_or_default().contains("agent_not_connected"));
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn mark_started_is_idempotent() -> anyhow::Result<()> {
        let Some((client, agent_id)) = connect_test_db().await else {
            eprintln!("SKIP: DASHBOARD_DB_URL not set");
            return Ok(());
        };
        let command_id = Uuid::new_v4();
        agent_commands::insert_pending(
            &client,
            &command_id,
            &agent_id,
            None,
            "health",
            &serde_json::json!({}),
            None,
        )
        .await?;
        agent_commands::mark_started(&client, &command_id).await?;
        let row1 = agent_commands::fetch_by_id(&client, &command_id)
            .await?
            .unwrap();
        let first = row1.started_at.expect("started_at stamped");

        // Second call must not move started_at.
        agent_commands::mark_started(&client, &command_id).await?;
        let row2 = agent_commands::fetch_by_id(&client, &command_id)
            .await?
            .unwrap();
        assert_eq!(row2.started_at, Some(first));
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn mark_finished_sets_terminal_fields() -> anyhow::Result<()> {
        let Some((client, agent_id)) = connect_test_db().await else {
            eprintln!("SKIP: DASHBOARD_DB_URL not set");
            return Ok(());
        };
        let command_id = Uuid::new_v4();
        agent_commands::insert_pending(
            &client,
            &command_id,
            &agent_id,
            None,
            "health",
            &serde_json::json!({}),
            None,
        )
        .await?;
        let payload = serde_json::json!({"ok": true});
        agent_commands::mark_finished(&client, &command_id, "ok", Some(&payload), None).await?;
        let row = agent_commands::fetch_by_id(&client, &command_id)
            .await?
            .unwrap();
        assert_eq!(row.status, "ok");
        assert!(row.finished_at.is_some());
        assert!(row.started_at.is_some(), "started_at back-filled");
        assert_eq!(row.result, Some(payload));
        Ok(())
    }
}
