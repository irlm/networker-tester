use axum::{
    extract::{ws, Query, State, WebSocketUpgrade},
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

/// Registry of connected agents.
pub struct AgentHub {
    /// agent_id → channel sender for dispatching messages to the agent.
    agents: RwLock<HashMap<Uuid, mpsc::UnboundedSender<String>>>,
}

impl AgentHub {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, agent_id: Uuid, tx: mpsc::UnboundedSender<String>) {
        self.agents.write().await.insert(agent_id, tx);
    }

    pub async fn unregister(&self, agent_id: &Uuid) {
        self.agents.write().await.remove(agent_id);
    }

    pub fn send_to_agent(
        &self,
        agent_id: &Uuid,
        msg: &ControlMessage,
    ) -> Result<(), anyhow::Error> {
        let text = protocol::encode(msg)?;
        // Use try_read to avoid blocking; if locked, retry is acceptable
        if let Ok(agents) = self.agents.try_read() {
            if let Some(tx) = agents.get(agent_id) {
                tx.send(text).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }
        Ok(())
    }

    pub fn any_online_agent(&self) -> Option<Uuid> {
        if let Ok(agents) = self.agents.try_read() {
            agents.keys().next().copied()
        } else {
            None
        }
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
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_agent_socket(socket, state, q.key))
}

async fn handle_agent_socket(socket: ws::WebSocket, state: Arc<AppState>, api_key: String) {
    // Authenticate agent by API key
    let agent = {
        let client = match state.db.get().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("DB error during agent auth: {e}");
                return;
            }
        };
        match crate::db::agents::get_by_api_key(&client, &api_key).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                tracing::warn!("Agent connection rejected: invalid API key");
                return;
            }
            Err(e) => {
                tracing::error!("DB error during agent auth: {e}");
                return;
            }
        }
    };

    let agent_id = agent.agent_id;
    let agent_name = agent.name.clone();
    tracing::info!(agent_id = %agent_id, name = %agent_name, "Agent connected");

    // Update status to online
    if let Ok(client) = state.db.get().await {
        let _ = crate::db::agents::update_status(&client, &agent_id, "online").await;
    }
    let _ = state.events_tx.send(DashboardEvent::AgentStatus {
        agent_id,
        status: "online".into(),
        last_heartbeat: Some(chrono::Utc::now()),
    });

    // Set up channels
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    state.agents.register(agent_id, tx).await;

    // Send welcome
    let welcome = ControlMessage::Welcome {
        agent_id,
        agent_name,
    };
    if let Ok(text) = protocol::encode(&welcome) {
        let _ = ws_sink.send(ws::Message::Text(text.into())).await;
    }

    // Forward outbound messages (control → agent)
    let sink_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sink.send(ws::Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Process inbound messages (agent → control)
    let state2 = state.clone();
    while let Some(Ok(msg)) = ws_stream.next().await {
        if let ws::Message::Text(text) = msg {
            if let Ok(agent_msg) = protocol::decode::<AgentMessage>(&text) {
                handle_agent_message(&state2, agent_id, agent_msg).await;
            }
        }
    }

    // Cleanup on disconnect
    sink_task.abort();
    state.agents.unregister(&agent_id).await;
    if let Ok(client) = state.db.get().await {
        let _ = crate::db::agents::update_status(&client, &agent_id, "offline").await;
    }
    let _ = state.events_tx.send(DashboardEvent::AgentStatus {
        agent_id,
        status: "offline".into(),
        last_heartbeat: None,
    });
    tracing::info!(agent_id = %agent_id, "Agent disconnected");
}

async fn handle_agent_message(state: &Arc<AppState>, agent_id: Uuid, msg: AgentMessage) {
    match msg {
        AgentMessage::Heartbeat { .. } => {
            tracing::trace!(agent_id = %agent_id, "Heartbeat received");
            if let Ok(client) = state.db.get().await {
                let _ = crate::db::agents::update_heartbeat(&client, &agent_id).await;
            }
        }
        AgentMessage::JobAck { job_id } => {
            let correlation_id = job_id.to_string();
            tracing::info!(correlation_id, agent_id = %agent_id, "Job ACK — setting status to running");
            if let Ok(client) = state.db.get().await {
                if let Err(e) = crate::db::jobs::update_status(&client, &job_id, "running").await {
                    tracing::error!(correlation_id, error = %e, "Failed to update job status to running");
                }
            }
            let _ = state.events_tx.send(DashboardEvent::JobUpdate {
                job_id,
                status: "running".into(),
                agent_id: Some(agent_id),
                started_at: Some(chrono::Utc::now()),
                finished_at: None,
            });
        }
        AgentMessage::AttemptResult { job_id, attempt } => {
            let correlation_id = job_id.to_string();
            tracing::info!(
                correlation_id,
                seq = attempt.sequence_num,
                protocol = %attempt.protocol,
                success = attempt.success,
                "Attempt result received — broadcasting to browsers"
            );
            let _ = state
                .events_tx
                .send(DashboardEvent::AttemptResult { job_id, attempt });
        }
        AgentMessage::JobComplete { job_id, run } => {
            let correlation_id = job_id.to_string();
            let run_id = run.run_id;
            let success_count = run.success_count();
            let failure_count = run.failure_count();

            tracing::info!(
                correlation_id,
                run_id = %run_id,
                success = success_count,
                failures = failure_count,
                attempts = run.attempts.len(),
                "Job complete — persisting results"
            );

            // Update job status
            if let Ok(client) = state.db.get().await {
                if let Err(e) = crate::db::jobs::update_status(&client, &job_id, "completed").await
                {
                    tracing::error!(correlation_id, error = %e, "Failed to update job status to completed");
                }
                if let Err(e) = crate::db::jobs::set_run_id(&client, &job_id, &run_id).await {
                    tracing::error!(correlation_id, error = %e, "Failed to set run_id on job");
                }
            }

            // Persist the TestRun via networker-tester's DB layer
            let db_url = std::env::var("DASHBOARD_DB_URL").unwrap_or_default();
            if !db_url.is_empty() {
                tracing::info!(
                    correlation_id,
                    "Saving TestRun to database via networker-tester backend"
                );
                match networker_tester::output::db::connect(&db_url).await {
                    Ok(backend) => {
                        // Run migration to ensure TestRun table exists
                        if let Err(e) = backend.migrate().await {
                            tracing::error!(correlation_id, error = %e, "DB migration failed");
                        }
                        if let Err(e) = backend.save(&run).await {
                            tracing::error!(correlation_id, error = %e, "Failed to save TestRun to database");
                        } else {
                            tracing::info!(correlation_id, run_id = %run_id, "TestRun saved to database");
                        }
                    }
                    Err(e) => {
                        tracing::error!(correlation_id, error = %e, "Failed to connect to DB for run save")
                    }
                }
            } else {
                tracing::warn!(
                    correlation_id,
                    "DASHBOARD_DB_URL not set — skipping TestRun persistence"
                );
            }

            let _ = state.events_tx.send(DashboardEvent::JobComplete {
                job_id,
                run_id,
                success_count,
                failure_count,
            });
            tracing::info!(correlation_id, "JobComplete event broadcast to browsers");
        }
        AgentMessage::JobError { job_id, message } => {
            let correlation_id = job_id.to_string();
            tracing::error!(correlation_id, error = %message, "Job error received from agent");
            if let Ok(client) = state.db.get().await {
                if let Err(e) = crate::db::jobs::set_error(&client, &job_id, &message).await {
                    tracing::error!(correlation_id, error = %e, "Failed to set error on job");
                }
            }
            let _ = state.events_tx.send(DashboardEvent::JobUpdate {
                job_id,
                status: "failed".into(),
                agent_id: Some(agent_id),
                started_at: None,
                finished_at: Some(chrono::Utc::now()),
            });
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws/agent", get(agent_ws_handler))
        .with_state(state)
}
