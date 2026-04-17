//! WebSocket v2 agent hub.
//!
//! Accepts agent connections, speaks protocol v2 (TestConfig / TestRun model).
//! Legacy v1 agent message variants (`JobAck`, `JobComplete`, `JobError`,
//! `JobLog`, `AttemptResult`, `TlsProfileComplete`) are ignored — all agents
//! must run v2. See `.critique/refactor/03-spec.md` §4.

use axum::{
    extract::{ws, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::AppState;
use networker_common::messages::{AgentMessage, ControlMessage, DashboardEvent};
use networker_common::protocol;
use networker_common::RunStatus;

/// Bounded channel capacity for agent WebSocket outbound messages.
const AGENT_CHANNEL_CAPACITY: usize = 256;

/// Registry of connected agents.
pub struct AgentHub {
    agents: RwLock<HashMap<Uuid, mpsc::Sender<String>>>,
}

impl AgentHub {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, agent_id: Uuid, tx: mpsc::Sender<String>) {
        self.agents.write().await.insert(agent_id, tx);
    }

    pub async fn unregister(&self, agent_id: &Uuid) {
        self.agents.write().await.remove(agent_id);
    }

    pub async fn send_to_agent(
        &self,
        agent_id: &Uuid,
        msg: &ControlMessage,
    ) -> Result<(), anyhow::Error> {
        let text = protocol::encode(msg)?;
        let agents = self.agents.read().await;
        match agents.get(agent_id) {
            Some(tx) => tx
                .try_send(text)
                .map_err(|e| anyhow::anyhow!("agent channel error: {e}")),
            None => Err(anyhow::anyhow!("agent {agent_id} not connected")),
        }
    }

    pub async fn is_agent_online(&self, agent_id: &Uuid) -> bool {
        self.agents.read().await.contains_key(agent_id)
    }

    pub async fn any_online_agent(&self) -> Option<Uuid> {
        let agents = self.agents.read().await;
        agents.keys().next().copied()
    }
}

#[derive(Deserialize)]
pub struct AgentWsQuery {
    key: String,
}

async fn agent_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(q): Query<AgentWsQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let agent = crate::db::agents::get_by_api_key(&client, &q.key)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    Ok(ws
        .max_message_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| handle_agent_socket(socket, state, agent)))
}

async fn handle_agent_socket(
    socket: ws::WebSocket,
    state: Arc<AppState>,
    agent: crate::db::agents::AgentRow,
) {
    let agent_id = agent.agent_id;
    let agent_name = agent.name.clone();
    tracing::info!(agent_id = %agent_id, name = %agent_name, "Tester connected (v2)");

    // Mark online
    if let Ok(client) = state.db.get().await {
        let _ = crate::db::agents::update_status(&client, &agent_id, "online").await;
    }
    let _ = state.events_tx.send(DashboardEvent::AgentStatus {
        agent_id,
        status: "online".into(),
        last_heartbeat: Some(chrono::Utc::now()),
    });

    let (mut ws_sink, mut ws_stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(AGENT_CHANNEL_CAPACITY);

    state.agents.register(agent_id, tx).await;

    // Send welcome
    let welcome = ControlMessage::Welcome {
        agent_id,
        agent_name,
    };
    if let Ok(text) = protocol::encode(&welcome) {
        let _ = ws_sink.send(ws::Message::Text(text.into())).await;
    }

    // Outbound pump
    let sink_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sink.send(ws::Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Inbound pump
    let state2 = state.clone();
    while let Some(Ok(msg)) = ws_stream.next().await {
        if let ws::Message::Text(text) = msg {
            if let Ok(agent_msg) = protocol::decode::<AgentMessage>(&text) {
                handle_agent_message(&state2, agent_id, agent_msg).await;
            }
        }
    }

    // Cleanup
    sink_task.abort();
    state.agents.unregister(&agent_id).await;
    if let Ok(client) = state.db.get().await {
        let _ = crate::db::agents::update_status(&client, &agent_id, "offline").await;
        // Fail orphaned runs that were running on this agent.
        let _ = client
            .execute(
                "UPDATE test_run SET status = 'failed',
                        error_message = 'Agent disconnected during execution',
                        finished_at = now()
                 WHERE tester_id = $1 AND status IN ('running', 'queued')",
                &[&agent_id],
            )
            .await;
    }
    let _ = state.events_tx.send(DashboardEvent::AgentStatus {
        agent_id,
        status: "offline".into(),
        last_heartbeat: None,
    });
    tracing::info!(agent_id = %agent_id, "Tester disconnected");
}

async fn handle_agent_message(state: &Arc<AppState>, agent_id: Uuid, msg: AgentMessage) {
    match msg {
        // ── v2 message handlers ─────────────────────────────────────────
        AgentMessage::Heartbeat { version, .. } => {
            tracing::trace!(agent_id = %agent_id, ?version, "v2 heartbeat");
            if let Ok(client) = state.db.get().await {
                let _ = crate::db::agents::update_heartbeat(&client, &agent_id, version.as_deref())
                    .await;
            }
        }
        AgentMessage::RunStarted { run_id, started_at } => {
            tracing::info!(run_id = %run_id, agent_id = %agent_id, "RunStarted");
            if let Ok(client) = state.db.get().await {
                let _ =
                    crate::db::test_runs::update_status(&client, &run_id, RunStatus::Running).await;
            }
            let _ = state.events_tx.send(DashboardEvent::JobUpdate {
                job_id: run_id,
                status: "running".into(),
                agent_id: Some(agent_id),
                started_at: Some(started_at),
                finished_at: None,
            });
        }
        AgentMessage::RunProgress {
            run_id,
            success,
            failure,
        } => {
            tracing::trace!(run_id = %run_id, success, failure, "RunProgress");
            if let Ok(client) = state.db.get().await {
                let _ =
                    crate::db::test_runs::update_counts(&client, &run_id, success, failure).await;
            }
        }
        AgentMessage::AttemptEvent { run_id, attempt } => {
            tracing::trace!(
                run_id = %run_id,
                seq = attempt.sequence_num,
                "AttemptEvent"
            );
            let _ = state.events_tx.send(DashboardEvent::AttemptResult {
                job_id: run_id,
                attempt,
            });
        }
        AgentMessage::RunFinished {
            run_id,
            status,
            artifact,
        } => {
            tracing::info!(run_id = %run_id, ?status, has_artifact = artifact.is_some(), "RunFinished");
            if let Ok(client) = state.db.get().await {
                let _ = crate::db::test_runs::update_status(&client, &run_id, status).await;

                // Persist the benchmark artifact if present.
                if let Some(art) = artifact {
                    let new_art = crate::db::benchmark_artifacts::NewBenchmarkArtifact {
                        test_run_id: &run_id,
                        environment: &art.environment,
                        methodology: &art.methodology,
                        launches: &art.launches,
                        cases: &art.cases,
                        samples: art.samples.as_ref(),
                        summaries: &art.summaries,
                        data_quality: &art.data_quality,
                    };
                    match crate::db::benchmark_artifacts::create(&client, &new_art).await {
                        Ok(saved) => {
                            tracing::info!(run_id = %run_id, artifact_id = %saved.id, "artifact persisted");
                        }
                        Err(e) => {
                            tracing::error!(run_id = %run_id, error = %e, "failed to persist artifact");
                        }
                    }
                }
            }

            // Read the final run state for the complete event.
            let (success_count, failure_count) = if let Ok(client) = state.db.get().await {
                if let Ok(Some(r)) = crate::db::test_runs::get(&client, &run_id).await {
                    (r.success_count as usize, r.failure_count as usize)
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            };
            let _ = state.events_tx.send(DashboardEvent::JobComplete {
                job_id: run_id,
                run_id,
                success_count,
                failure_count,
            });
        }
        AgentMessage::Error { run_id, message } => {
            tracing::error!(?run_id, error = %message, "Agent error");
            if let (Some(rid), Ok(client)) = (run_id, state.db.get().await) {
                let _ = crate::db::test_runs::set_error(&client, &rid, &message).await;
                let _ = state.events_tx.send(DashboardEvent::JobUpdate {
                    job_id: rid,
                    status: "failed".into(),
                    agent_id: Some(agent_id),
                    started_at: None,
                    finished_at: Some(chrono::Utc::now()),
                });
            }
        }

        // ── command dispatch (unchanged) ────────────────────────────────
        AgentMessage::CommandLog(log) => {
            if let Err(e) = crate::agent_dispatch::handle_command_log(state, log).await {
                tracing::error!(agent_id = %agent_id, error = %e, "command log ingestion failed");
            }
        }
        AgentMessage::CommandResult(result) => {
            if let Err(e) = crate::agent_dispatch::handle_command_result(state, result).await {
                tracing::error!(agent_id = %agent_id, error = %e, "command result ingestion failed");
            }
        }

        // ── Legacy v1 variants — ignored ────────────────────────────────
        // These are kept in the `AgentMessage` enum for compile compat
        // with the Agent C transition period. Dashboard no longer handles them.
        _ => {
            tracing::trace!(agent_id = %agent_id, "Ignored legacy v1 agent message");
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws/agent", get(agent_ws_handler))
        .with_state(state)
}
