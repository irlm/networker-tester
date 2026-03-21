use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_role, AuthUser, Role};
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
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_agents");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let agents = crate::db::agents::list(&client).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list agents from DB");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(AgentListResponse { agents }))
}

async fn create_agent(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateAgentRequest>,
) -> Result<Json<CreateAgentResponse>, StatusCode> {
    require_role(&user, Role::Operator)?;
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
            if !is_valid_ssh_host(&ssh_host) || !is_valid_ssh_user(&ssh_user) || ssh_port == 0 {
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
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Operator)?;
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

/// Validates an SSH hostname: non-empty, alphanumeric plus `.`, `-`, `_`.
fn is_valid_ssh_host(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || ".-_".contains(c))
}

/// Validates an SSH username: alphanumeric plus `.`, `_`, `-`.
fn is_valid_ssh_user(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/:agent_id", get(delete_agent).delete(delete_agent))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{is_valid_ssh_host, is_valid_ssh_user};

    /// Tests for SSH host/user validation (command injection prevention).
    mod ssh_validation {
        use super::*;

        #[test]
        fn valid_hostname_accepted() {
            assert!(is_valid_ssh_host("my-host.example.com"));
            assert!(is_valid_ssh_host("10.0.0.1"));
            assert!(is_valid_ssh_host("host_name"));
            assert!(is_valid_ssh_host("a"));
        }

        #[test]
        fn empty_hostname_rejected() {
            assert!(!is_valid_ssh_host(""));
        }

        #[test]
        fn hostname_with_semicolon_rejected() {
            assert!(!is_valid_ssh_host("host; rm -rf /"));
        }

        #[test]
        fn hostname_with_backtick_rejected() {
            assert!(!is_valid_ssh_host("host`whoami`"));
        }

        #[test]
        fn hostname_with_space_rejected() {
            assert!(!is_valid_ssh_host("host name"));
        }

        #[test]
        fn hostname_with_dollar_rejected() {
            assert!(!is_valid_ssh_host("$HOME"));
        }

        #[test]
        fn hostname_with_newline_rejected() {
            assert!(!is_valid_ssh_host("host\n-o ProxyCommand=evil"));
        }

        #[test]
        fn valid_username_accepted() {
            assert!(is_valid_ssh_user("root"));
            assert!(is_valid_ssh_user("deploy-user"));
            assert!(is_valid_ssh_user("user_name"));
            assert!(is_valid_ssh_user("user.name"));
        }

        #[test]
        fn empty_username_rejected() {
            assert!(!is_valid_ssh_user(""));
        }

        #[test]
        fn username_with_shell_chars_rejected() {
            assert!(!is_valid_ssh_user("root;id"));
            assert!(!is_valid_ssh_user("user$(whoami)"));
            assert!(!is_valid_ssh_user("root && cat /etc/passwd"));
        }
    }

    /// Tests for the CreateAgentRequest deserialization defaults.
    mod request_defaults {
        use super::super::CreateAgentRequest;

        #[test]
        fn location_defaults_to_local() {
            let json = r#"{"name": "test-agent"}"#;
            let req: CreateAgentRequest = serde_json::from_str(json).unwrap();
            assert_eq!(req.location, "local");
        }

        #[test]
        fn optional_fields_default_to_none() {
            let json = r#"{"name": "test-agent"}"#;
            let req: CreateAgentRequest = serde_json::from_str(json).unwrap();
            assert!(req.region.is_none());
            assert!(req.provider.is_none());
            assert!(req.ssh_host.is_none());
            assert!(req.ssh_user.is_none());
            assert!(req.ssh_port.is_none());
        }

        #[test]
        fn all_fields_populated() {
            let json = r#"{
                "name": "remote-1",
                "region": "us-east-1",
                "provider": "aws",
                "location": "ssh",
                "ssh_host": "10.0.0.5",
                "ssh_user": "deploy",
                "ssh_port": 2222
            }"#;
            let req: CreateAgentRequest = serde_json::from_str(json).unwrap();
            assert_eq!(req.name, "remote-1");
            assert_eq!(req.location, "ssh");
            assert_eq!(req.ssh_port, Some(2222));
        }
    }
}
