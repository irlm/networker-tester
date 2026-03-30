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

const MAX_TLS_PROFILE_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_TLS_PROFILE_FINDINGS: usize = 2048;
const MAX_TLS_PROFILE_STRINGS: usize = 4096;

fn validate_tls_profile_size(
    profile: &networker_tester::tls_profile::TlsEndpointProfile,
) -> anyhow::Result<()> {
    let json = serde_json::to_vec(profile)?;
    if json.len() > MAX_TLS_PROFILE_JSON_BYTES {
        anyhow::bail!(
            "TLS profile JSON exceeds {} bytes",
            MAX_TLS_PROFILE_JSON_BYTES
        );
    }
    if profile.findings.len() > MAX_TLS_PROFILE_FINDINGS {
        anyhow::bail!(
            "TLS profile findings exceed {} entries",
            MAX_TLS_PROFILE_FINDINGS
        );
    }
    let stringish = profile.unsupported_checks.len()
        + profile.limitations.len()
        + profile.target.resolved_ips.len()
        + profile.path_characteristics.evidence.len()
        + profile.trust.issues.len()
        + profile.trust.revocation.notes.len()
        + profile.resumption.notes.len();
    if stringish > MAX_TLS_PROFILE_STRINGS {
        anyhow::bail!(
            "TLS profile string list fields exceed {} entries",
            MAX_TLS_PROFILE_STRINGS
        );
    }
    Ok(())
}

async fn job_assigned_to_agent(
    state: &AppState,
    job_id: &Uuid,
    agent_id: Uuid,
) -> anyhow::Result<bool> {
    let client = state.db.get().await?;
    Ok(crate::db::jobs::get(&client, job_id)
        .await?
        .and_then(|job| job.agent_id)
        == Some(agent_id))
}

/// Bounded channel capacity for agent WebSocket outbound messages.
const AGENT_CHANNEL_CAPACITY: usize = 256;

/// Registry of connected agents.
pub struct AgentHub {
    /// agent_id → channel sender for dispatching messages to the agent.
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
    // Validate API key BEFORE WebSocket upgrade to reject unauthorized connections
    // at the HTTP layer rather than after the upgrade completes.
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let agent = crate::db::agents::get_by_api_key(&client, &q.key)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Large test runs (27 modes × 3 runs = 255 attempts) produce multi-MB JSON.
    // Default axum WS limit is 64KB which silently drops the JobComplete message.
    Ok(ws
        .max_message_size(64 * 1024 * 1024) // 64 MB
        .on_upgrade(move |socket| handle_agent_socket(socket, state, agent)))
}

async fn handle_agent_socket(
    socket: ws::WebSocket,
    state: Arc<AppState>,
    agent: crate::db::agents::AgentRow,
) {
    let agent_id = agent.agent_id;
    let agent_name = agent.name.clone();
    tracing::info!(agent_id = %agent_id, name = %agent_name, "Tester connected");

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
    let (tx, mut rx) = mpsc::channel::<String>(AGENT_CHANNEL_CAPACITY);

    state.agents.register(agent_id, tx).await;

    // Send welcome
    let welcome = ControlMessage::Welcome {
        agent_id,
        agent_name,
    };
    if let Ok(text) = protocol::encode(&welcome) {
        let _ = ws_sink.send(ws::Message::Text(text)).await;
    }

    // Forward outbound messages (control → agent)
    let sink_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sink.send(ws::Message::Text(msg)).await.is_err() {
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
        // Fail any jobs that were running/assigned on this agent (orphan cleanup)
        match client
            .execute(
                "UPDATE job SET status = 'failed',
                        error_message = 'Agent disconnected during execution',
                        finished_at = now()
                 WHERE agent_id = $1 AND status IN ('running', 'assigned')",
                &[&agent_id],
            )
            .await
        {
            Ok(count) if count > 0 => {
                tracing::warn!(
                    agent_id = %agent_id,
                    orphaned_jobs = count,
                    "Failed orphaned jobs due to agent disconnect"
                );
            }
            Err(e) => {
                tracing::error!(agent_id = %agent_id, error = %e, "Failed to clean up orphaned jobs");
            }
            _ => {}
        }
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
        AgentMessage::Heartbeat { .. } => {
            tracing::trace!(agent_id = %agent_id, "Heartbeat received");
            if let Ok(client) = state.db.get().await {
                let _ = crate::db::agents::update_heartbeat(&client, &agent_id).await;
            }
        }
        AgentMessage::JobAck { job_id } => {
            let correlation_id = job_id.to_string();
            tracing::info!(correlation_id, agent_id = %agent_id, "Job ACK — setting status to running");
            match job_assigned_to_agent(state, &job_id, agent_id).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, "Rejecting JobAck for unassigned job");
                    return;
                }
                Err(e) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, error = %e, "Failed to authorize JobAck");
                    return;
                }
            }
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
            match job_assigned_to_agent(state, &job_id, agent_id).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, "Rejecting AttemptResult for unassigned job");
                    return;
                }
                Err(e) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, error = %e, "Failed to authorize AttemptResult");
                    return;
                }
            }
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

            match job_assigned_to_agent(state, &job_id, agent_id).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, "Rejecting JobComplete for unassigned job");
                    return;
                }
                Err(e) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, error = %e, "Failed to authorize JobComplete");
                    return;
                }
            }

            tracing::info!(
                correlation_id,
                run_id = %run_id,
                success = success_count,
                failures = failure_count,
                attempts = run.attempts.len(),
                "Job complete — persisting results"
            );

            // Set run_id BEFORE status so the frontend never sees completed+null run_id
            if let Ok(client) = state.db.get().await {
                if let Err(e) = crate::db::jobs::set_run_id(&client, &job_id, &run_id).await {
                    tracing::error!(correlation_id, error = %e, "Failed to set run_id on job");
                }
                if let Err(e) = crate::db::jobs::update_status(&client, &job_id, "completed").await
                {
                    tracing::error!(correlation_id, error = %e, "Failed to update job status to completed");
                }
            }

            // Persist the TestRun via networker-tester's DB layer
            let db_url = &state.database_url;
            if !db_url.is_empty() {
                tracing::info!(
                    correlation_id,
                    "Saving TestRun to database via networker-tester backend"
                );
                match networker_tester::output::db::connect(db_url).await {
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
        AgentMessage::TlsProfileComplete { job_id, profile } => {
            let correlation_id = job_id.to_string();

            tracing::info!(correlation_id, agent_id = %agent_id, host = %profile.target.host, port = profile.target.port, "TLS profile complete — persisting result");

            let mut project_id = None;
            let mut authorized = false;
            let pooled_client = match state.db.get().await {
                Ok(client) => Some(client),
                Err(e) => {
                    tracing::warn!(correlation_id, agent_id = %agent_id, error = %e, "DB pool unavailable in TlsProfileComplete — project linkage/status updates may be lost");
                    None
                }
            };
            if let Some(client) = pooled_client.as_ref() {
                if let Err(e) = crate::db::jobs::update_status(client, &job_id, "completed").await {
                    tracing::error!(correlation_id, error = %e, "Failed to update TLS profile job status to completed");
                }
                if let Some(job) = crate::db::jobs::get(client, &job_id).await.ok().flatten() {
                    project_id = job.project_id;
                    authorized = job.agent_id == Some(agent_id);
                }
            }

            if !authorized {
                tracing::error!(correlation_id, agent_id = %agent_id, "Rejecting TLS profile completion for unassigned job");
                return;
            }

            if let Err(e) = validate_tls_profile_size(&profile) {
                tracing::error!(correlation_id, agent_id = %agent_id, error = %e, "Rejecting oversized TLS profile payload");
                return;
            }

            let db_url = &state.database_url;
            if !db_url.is_empty() {
                match networker_tester::output::db::connect(db_url).await {
                    Ok(backend) => {
                        let mut migrated = state.tls_profile_db_migrated.lock().await;
                        if !*migrated {
                            if let Err(e) = backend.migrate().await {
                                tracing::error!(correlation_id, error = %e, "DB migration failed");
                            } else {
                                *migrated = true;
                            }
                        }
                        drop(migrated);
                        match backend
                            .save_tls_profile(&profile, project_id.as_ref())
                            .await
                        {
                            Ok(tls_profile_run_id) => {
                                if let Some(client) = pooled_client.as_ref() {
                                    if let Err(e) = crate::db::jobs::set_tls_profile_run_id(
                                        client,
                                        &job_id,
                                        &tls_profile_run_id,
                                    )
                                    .await
                                    {
                                        tracing::error!(correlation_id, error = %e, "Failed to link TLS profile run to job");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(correlation_id, error = %e, "Failed to save TLS profile to database");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(correlation_id, error = %e, "Failed to connect to DB for TLS profile save")
                    }
                }
            }

            let _ = state.events_tx.send(DashboardEvent::JobUpdate {
                job_id,
                status: "completed".into(),
                agent_id: Some(agent_id),
                started_at: None,
                finished_at: Some(chrono::Utc::now()),
            });
        }
        AgentMessage::JobLog {
            job_id,
            line,
            level,
        } => {
            // Forward tester log lines to browsers
            let _ = state.events_tx.send(DashboardEvent::JobLog {
                job_id,
                line,
                level,
            });
        }
        AgentMessage::JobError { job_id, message } => {
            let correlation_id = job_id.to_string();
            match job_assigned_to_agent(state, &job_id, agent_id).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, "Rejecting JobError for unassigned job");
                    return;
                }
                Err(e) => {
                    tracing::error!(correlation_id, agent_id = %agent_id, error = %e, "Failed to authorize JobError");
                    return;
                }
            }
            tracing::error!(correlation_id, error = %message, "Job error received from tester");
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
