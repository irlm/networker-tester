use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
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
    /// "ssh" = deploy via SSH (only remote agents supported)
    #[serde(default = "default_location")]
    pub location: String,
    /// SSH connection details (required when location = "ssh")
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
}

fn default_location() -> String {
    "ssh".into()
}

#[derive(Serialize)]
pub struct CreateAgentResponse {
    pub agent_id: Uuid,
    pub api_key: String,
    pub name: String,
    pub status: String,
}

/// Validates an SSH hostname: non-empty, alphanumeric plus `.`, `-`, `_`.
fn is_valid_ssh_host(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || ".-_".contains(c))
}

/// Validates an SSH username: alphanumeric plus `.`, `_`, `-`.
fn is_valid_ssh_user(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn list_agents_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<AgentListResponse>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_agents_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let agents = crate::db::agents::list(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list agents from DB");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(AgentListResponse { agents }))
}

async fn create_agent_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<CreateAgentResponse>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let create_req: CreateAgentRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let api_key = format!("agent-{}", Uuid::new_v4());
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_agent_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let provider = if create_req.location == "local" {
        Some("local")
    } else {
        create_req.provider.as_deref()
    };

    let agent_id = crate::db::agents::create(
        &client,
        &create_req.name,
        &api_key,
        create_req.region.as_deref(),
        provider,
        &ctx.project_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create tester");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        agent_id = %agent_id,
        name = %create_req.name,
        location = %create_req.location,
        project_id = %ctx.project_id,
        created_by = %user.email,
        "Tester created (project-scoped)"
    );

    let dashboard_port = state.dashboard_port;

    if create_req.location == "ssh" {
        let ssh_host = create_req.ssh_host.clone().unwrap_or_default();
        let ssh_user = create_req.ssh_user.clone().unwrap_or_else(|| "root".into());
        let ssh_port = create_req.ssh_port.unwrap_or(22);
        if !is_valid_ssh_host(&ssh_host) || !is_valid_ssh_user(&ssh_user) || ssh_port == 0 {
            return Err(StatusCode::BAD_REQUEST);
        }
        let api_key_clone = api_key.clone();
        let name_clone = create_req.name.clone();
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

    Ok(Json(CreateAgentResponse {
        agent_id,
        api_key: api_key.clone(),
        name: create_req.name,
        status: "provisioning".into(),
    }))
}

async fn delete_agent_scoped(
    State(state): State<Arc<AppState>>,
    Path((_, agent_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)?;

    let pid = state.tester_processes.write().await.remove(&agent_id);
    if let Some(pid) = pid {
        tracing::info!(agent_id = %agent_id, pid, "Killing tester process");
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .output()
                .await;
        }
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_agent_scoped");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

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

    tracing::info!(agent_id = %agent_id, "Tester deleted (project-scoped)");
    Ok(Json(serde_json::json!({"deleted": true})))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/agents", get(list_agents_scoped).post(create_agent_scoped))
        .route(
            "/agents/:agent_id",
            get(delete_agent_scoped).delete(delete_agent_scoped),
        )
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
