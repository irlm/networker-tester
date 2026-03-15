use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize)]
struct AgentListResponse {
    agents: Vec<crate::db::agents::AgentRow>,
}

#[derive(Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub region: Option<String>,
    pub provider: Option<String>,
    /// "local" = spawn on this machine, "ssh" = deploy via SSH
    #[serde(default = "default_location")]
    pub location: String,
    /// SSH connection details (required when location = "ssh")
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
}

fn default_location() -> String {
    "local".into()
}

#[derive(Serialize)]
pub struct CreateAgentResponse {
    pub agent_id: Uuid,
    pub api_key: String,
    pub name: String,
    pub status: String,
}

async fn list_agents(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AgentListResponse>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let agents = crate::db::agents::list(&client)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(AgentListResponse { agents }))
}

async fn create_agent(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateAgentRequest>,
) -> Result<Json<CreateAgentResponse>, StatusCode> {
    let api_key = format!("agent-{}", Uuid::new_v4());

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_agent");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Set provider to "local" for locally-spawned testers
    let provider = if req.location == "local" {
        Some("local")
    } else {
        req.provider.as_deref()
    };

    let agent_id = crate::db::agents::create(
        &client,
        &req.name,
        &api_key,
        req.region.as_deref(),
        provider,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create tester");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(agent_id = %agent_id, name = %req.name, location = %req.location, "Tester created");

    let dashboard_port = state.dashboard_port;

    match req.location.as_str() {
        "local" => {
            let api_key_clone = api_key.clone();
            let dashboard_url = format!("ws://127.0.0.1:{dashboard_port}/ws/agent");
            let state_clone = state.clone();
            tokio::spawn(async move {
                if let Some(pid) = crate::deploy::agent_provisioner::spawn_local_agent(
                    &api_key_clone,
                    &dashboard_url,
                )
                .await
                {
                    state_clone
                        .tester_processes
                        .write()
                        .await
                        .insert(agent_id, pid);
                }
            });
        }
        "ssh" => {
            let ssh_host = req.ssh_host.clone().unwrap_or_default();
            let ssh_user = req.ssh_user.clone().unwrap_or_else(|| "root".into());
            let ssh_port = req.ssh_port.unwrap_or(22);
            // Validate SSH inputs to prevent command injection
            let valid_host = |s: &str| {
                !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || ".-_".contains(c))
            };
            let valid_user = |s: &str| s.chars().all(|c| c.is_alphanumeric() || "._-".contains(c));
            if !valid_host(&ssh_host) || !valid_user(&ssh_user) || ssh_port == 0 {
                return Err(StatusCode::BAD_REQUEST);
            }
            let api_key_clone = api_key.clone();
            let name_clone = req.name.clone();
            let dashboard_url = format!("ws://{{DASHBOARD_HOST}}:{dashboard_port}/ws/agent");
            let events_tx = state.events_tx.clone();
            tokio::spawn(async move {
                crate::deploy::agent_provisioner::provision_remote_agent(
                    &name_clone,
                    &api_key_clone,
                    &dashboard_url,
                    &ssh_host,
                    &ssh_user,
                    ssh_port,
                    events_tx,
                )
                .await;
            });
        }
        _ => {}
    }

    Ok(Json(CreateAgentResponse {
        agent_id,
        api_key: api_key.clone(),
        name: req.name,
        status: if req.location == "local" {
            "starting".into()
        } else {
            "provisioning".into()
        },
    }))
}

async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Kill the tester process if we spawned it
    let pid = state.tester_processes.write().await.remove(&agent_id);
    if let Some(pid) = pid {
        tracing::info!(agent_id = %agent_id, pid, "Killing tester process");
        #[cfg(unix)]
        {
            // SAFETY: PID was stored by us when we spawned the process. TOCTOU race
            // is acceptable here — worst case we signal a recycled PID which ignores SIGTERM.
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            // On non-unix, try taskkill
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .output()
                .await;
        }
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_agent");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Clear FK references before deleting
    client
        .execute(
            "UPDATE job SET agent_id = NULL WHERE agent_id = $1",
            &[&agent_id],
        )
        .await
        .ok();
    client
        .execute(
            "UPDATE deployment SET agent_id = NULL WHERE agent_id = $1",
            &[&agent_id],
        )
        .await
        .ok();
    client
        .execute("DELETE FROM agent WHERE agent_id = $1", &[&agent_id])
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete tester");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(agent_id = %agent_id, "Tester deleted");
    Ok(Json(serde_json::json!({"deleted": true})))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/:agent_id", get(delete_agent).delete(delete_agent))
        .with_state(state)
}
