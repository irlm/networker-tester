use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_role, AuthUser, ProjectContext, ProjectRole, Role, DEFAULT_PROJECT_ID};
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
    let agents = crate::db::agents::list(&client, &DEFAULT_PROJECT_ID)
        .await
        .map_err(|e| {
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
        &DEFAULT_PROJECT_ID,
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

#[derive(Deserialize)]
pub struct DeployVmRequest {
    pub name: String,
    pub provider: String,
    pub region: String,
    pub vm_size: String,
}

#[derive(Serialize)]
pub struct DeployVmResponse {
    pub agent_id: Uuid,
    pub status: String,
}

/// Allowed Azure regions for VM deployment.
const ALLOWED_REGIONS: &[&str] = &[
    "eastus",
    "eastus2",
    "westus2",
    "westus3",
    "northeurope",
    "westeurope",
    "southeastasia",
    "australiaeast",
    "uksouth",
    "centralus",
];

/// Allowed Azure VM sizes.
const ALLOWED_VM_SIZES: &[&str] = &[
    "Standard_B1s",
    "Standard_B2s",
    "Standard_D2s_v3",
    "Standard_D2s_v5",
];

/// Validates a VM name: alphanumeric plus `-`, 1-64 chars.
fn is_valid_vm_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

async fn deploy_vm(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<DeployVmRequest>,
) -> Result<Json<DeployVmResponse>, StatusCode> {
    require_role(&user, Role::Operator)?;

    // Validate provider (only Azure for now)
    if req.provider != "azure" {
        tracing::warn!(provider = %req.provider, "Unsupported cloud provider for VM deploy");
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate inputs
    if !is_valid_vm_name(&req.name) {
        tracing::warn!(name = %req.name, "Invalid VM name");
        return Err(StatusCode::BAD_REQUEST);
    }
    if !ALLOWED_REGIONS.contains(&req.region.as_str()) {
        tracing::warn!(region = %req.region, "Invalid region");
        return Err(StatusCode::BAD_REQUEST);
    }
    if !ALLOWED_VM_SIZES.contains(&req.vm_size.as_str()) {
        tracing::warn!(vm_size = %req.vm_size, "Invalid VM size");
        return Err(StatusCode::BAD_REQUEST);
    }

    // Generate API key and create agent record
    let api_key = format!("agent-{}", Uuid::new_v4());

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in deploy_vm");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let agent_id = crate::db::agents::create(
        &client,
        &req.name,
        &api_key,
        Some(&req.region),
        Some("azure"),
        &DEFAULT_PROJECT_ID,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create agent for VM deploy");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Set status to deploying
    crate::db::agents::update_status(&client, &agent_id, "deploying")
        .await
        .ok();

    tracing::info!(
        agent_id = %agent_id,
        name = %req.name,
        region = %req.region,
        vm_size = %req.vm_size,
        "Starting cloud VM deployment"
    );

    // Spawn background deployment task
    let state_clone = state.clone();
    let name = req.name.clone();
    let region = req.region.clone();
    let vm_size = req.vm_size.clone();
    tokio::spawn(async move {
        run_vm_deployment(state_clone, agent_id, name, region, vm_size, api_key).await;
    });

    Ok(Json(DeployVmResponse {
        agent_id,
        status: "deploying".into(),
    }))
}

/// Background task that creates an Azure VM, installs the tester agent, and starts it.
async fn run_vm_deployment(
    state: Arc<AppState>,
    agent_id: Uuid,
    name: String,
    region: String,
    vm_size: String,
    api_key: String,
) {
    let events_tx = state.events_tx.clone();
    let rg = format!("networker-testers-{region}-rg");

    let send_log = |msg: String| {
        let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
            deployment_id: agent_id, // Re-use deployment_id field for agent deploy logs
            line: msg,
            stream: "stdout".into(),
        });
    };

    let set_failed = |state: &Arc<AppState>, agent_id: Uuid, msg: &str| {
        let state = state.clone();
        let msg = msg.to_string();
        async move {
            tracing::error!(agent_id = %agent_id, error = %msg, "VM deployment failed");
            if let Ok(client) = state.db.get().await {
                let _ = crate::db::agents::update_status(&client, &agent_id, "failed").await;
            }
        }
    };

    // Step 1: Ensure resource group exists
    send_log(format!("Creating resource group '{rg}' in {region}..."));
    let rg_result = tokio::process::Command::new("az")
        .args([
            "group",
            "create",
            "--name",
            &rg,
            "--location",
            &region,
            "--output",
            "none",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match rg_result {
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            send_log(format!("Failed to create resource group: {stderr}"));
            set_failed(
                &state,
                agent_id,
                &format!("Resource group creation failed: {stderr}"),
            )
            .await;
            return;
        }
        Err(e) => {
            send_log(format!("Failed to run az CLI: {e}"));
            set_failed(&state, agent_id, &format!("az CLI error: {e}")).await;
            return;
        }
        _ => {}
    }

    // Step 2: Create VM
    send_log(format!(
        "Creating VM '{name}' ({vm_size}) in {region}... (~2 min)"
    ));
    let vm_result = tokio::process::Command::new("az")
        .args([
            "vm",
            "create",
            "--resource-group",
            &rg,
            "--name",
            &name,
            "--image",
            "Ubuntu2404",
            "--size",
            &vm_size,
            "--admin-username",
            "azureuser",
            "--generate-ssh-keys",
            "--public-ip-sku",
            "Standard",
            "--location",
            &region,
            "--output",
            "none",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match vm_result {
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            send_log(format!("VM creation failed: {stderr}"));
            set_failed(&state, agent_id, &format!("VM creation failed: {stderr}")).await;
            return;
        }
        Err(e) => {
            send_log(format!("Failed to run az vm create: {e}"));
            set_failed(&state, agent_id, &format!("az vm create error: {e}")).await;
            return;
        }
        _ => {}
    }
    send_log("VM created successfully.".into());

    // Step 3: Get public IP
    send_log("Retrieving public IP...".into());
    let ip_result = tokio::process::Command::new("az")
        .args([
            "vm",
            "show",
            "-g",
            &rg,
            "-n",
            &name,
            "-d",
            "--query",
            "publicIps",
            "-o",
            "tsv",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    let public_ip = match ip_result {
        Ok(out) if out.status.success() => {
            let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if ip.is_empty() {
                send_log("Could not retrieve public IP.".into());
                set_failed(&state, agent_id, "No public IP returned").await;
                return;
            }
            send_log(format!("Public IP: {ip}"));
            ip
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            send_log(format!("Failed to get public IP: {stderr}"));
            set_failed(&state, agent_id, &format!("IP query failed: {stderr}")).await;
            return;
        }
        Err(e) => {
            send_log(format!("az vm show error: {e}"));
            set_failed(&state, agent_id, &format!("az vm show error: {e}")).await;
            return;
        }
    };

    // Step 4: Open NSG port 8443 for endpoint (best-effort)
    send_log("Opening NSG port 8443...".into());
    let _ = tokio::process::Command::new("az")
        .args([
            "vm",
            "open-port",
            "--resource-group",
            &rg,
            "--name",
            &name,
            "--port",
            "8443",
            "--priority",
            "1100",
            "--output",
            "none",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    // Step 5: Build the dashboard URL for the agent
    let dashboard_url = if state.public_url.starts_with("https://") {
        let host = state.public_url.trim_start_matches("https://");
        format!("wss://{host}/ws/agent")
    } else if state.public_url.starts_with("http://") {
        let host = state.public_url.trim_start_matches("http://");
        format!("ws://{host}/ws/agent")
    } else {
        format!("wss://{}/ws/agent", state.public_url)
    };

    // Step 6: Install tester and agent via run-command
    send_log("Installing tester agent on VM... (~30s)".into());
    let install_script = format!(
        r#"
cd /tmp

# Install Chrome for browser test modes
apt-get update -qq < /dev/null
apt-get install -y wget gnupg < /dev/null
wget -q -O - https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" > /etc/apt/sources.list.d/google-chrome.list
apt-get update -qq < /dev/null
apt-get install -y google-chrome-stable < /dev/null || apt-get install -y chromium-browser < /dev/null || true

# Install tester and agent binaries
curl -sL https://github.com/irlm/networker-tester/releases/download/v{version}/networker-agent-x86_64-unknown-linux-musl.tar.gz | tar xz
curl -sL https://github.com/irlm/networker-tester/releases/download/v{version}/networker-tester-x86_64-unknown-linux-musl.tar.gz | tar xz
mkdir -p /opt/networker
cp networker-agent networker-tester /opt/networker/
chmod +x /opt/networker/*
ln -sf /opt/networker/networker-tester /usr/local/bin/

cat > /etc/systemd/system/networker-agent.service << SVCEOF
[Unit]
Description=Networker Tester Agent
After=network.target

[Service]
Type=simple
ExecStart=/opt/networker/networker-agent
Restart=always
RestartSec=5
Environment=AGENT_DASHBOARD_URL={dashboard_url}
Environment=AGENT_API_KEY={api_key}

[Install]
WantedBy=multi-user.target
SVCEOF

systemctl daemon-reload
systemctl enable networker-agent
systemctl start networker-agent
echo "Agent installed and started"
"#,
        version = env!("CARGO_PKG_VERSION"),
        dashboard_url = dashboard_url,
        api_key = api_key,
    );

    let install_result = tokio::process::Command::new("az")
        .args([
            "vm",
            "run-command",
            "invoke",
            "--resource-group",
            &rg,
            "--name",
            &name,
            "--command-id",
            "RunShellScript",
            "--scripts",
            &install_script,
            "--output",
            "none",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match install_result {
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            send_log(format!("Install command failed: {stderr}"));
            set_failed(&state, agent_id, &format!("Install failed: {stderr}")).await;
            return;
        }
        Err(e) => {
            send_log(format!("az vm run-command error: {e}"));
            set_failed(&state, agent_id, &format!("run-command error: {e}")).await;
            return;
        }
        _ => {}
    }

    send_log(format!(
        "Tester agent installed on {name} ({public_ip}). Waiting for connection..."
    ));

    // Step 7: Wait for agent to connect (poll for up to 60 seconds)
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut connected = false;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if let Ok(client) = state.db.get().await {
            if let Ok(Some(agent)) = crate::db::agents::get_by_id(&client, &agent_id).await {
                if agent.status == "online" {
                    connected = true;
                    break;
                }
            }
        }
    }

    if connected {
        send_log(format!("Tester '{name}' is online at {public_ip}"));
        let _ = events_tx.send(networker_common::messages::DashboardEvent::AgentStatus {
            agent_id,
            status: "online".into(),
            last_heartbeat: Some(chrono::Utc::now()),
        });
    } else {
        // Agent may still connect later — set status to 'waiting'
        send_log(format!(
            "Tester '{name}' deployed but not yet connected. It may take a moment."
        ));
        if let Ok(client) = state.db.get().await {
            // Only update if still deploying (agent may have connected and set itself online)
            let _ = client
                .execute(
                    "UPDATE agent SET status = 'waiting' WHERE agent_id = $1 AND status = 'deploying'",
                    &[&agent_id],
                )
                .await;
        }
    }
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
        .route("/agents/deploy-vm", post(deploy_vm))
        .route("/agents/:agent_id", get(delete_agent).delete(delete_agent))
        .with_state(state)
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

    match create_req.location.as_str() {
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
        _ => {}
    }

    Ok(Json(CreateAgentResponse {
        agent_id,
        api_key: api_key.clone(),
        name: create_req.name,
        status: if create_req.location == "local" {
            "starting".into()
        } else {
            "provisioning".into()
        },
    }))
}

async fn delete_agent_scoped(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
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
    use super::{is_valid_ssh_host, is_valid_ssh_user, is_valid_vm_name};

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

    /// Tests for VM name validation.
    mod vm_name_validation {
        use super::*;

        #[test]
        fn valid_vm_names() {
            assert!(is_valid_vm_name("tester-eastus-1"));
            assert!(is_valid_vm_name("my-vm"));
            assert!(is_valid_vm_name("a"));
            assert!(is_valid_vm_name("test123"));
        }

        #[test]
        fn empty_name_rejected() {
            assert!(!is_valid_vm_name(""));
        }

        #[test]
        fn name_with_special_chars_rejected() {
            assert!(!is_valid_vm_name("vm;drop table"));
            assert!(!is_valid_vm_name("vm name"));
            assert!(!is_valid_vm_name("vm$HOME"));
            assert!(!is_valid_vm_name("vm`id`"));
        }

        #[test]
        fn name_with_leading_trailing_dash_rejected() {
            assert!(!is_valid_vm_name("-vm"));
            assert!(!is_valid_vm_name("vm-"));
        }

        #[test]
        fn name_too_long_rejected() {
            let long_name = "a".repeat(65);
            assert!(!is_valid_vm_name(&long_name));
            // 64 is ok
            let ok_name = "a".repeat(64);
            assert!(is_valid_vm_name(&ok_name));
        }
    }

    /// Tests for DeployVmRequest deserialization.
    mod deploy_vm_request {
        use super::super::DeployVmRequest;

        #[test]
        fn valid_request() {
            let json = r#"{
                "name": "us-east-tester",
                "provider": "azure",
                "region": "eastus",
                "vm_size": "Standard_B1s"
            }"#;
            let req: DeployVmRequest = serde_json::from_str(json).unwrap();
            assert_eq!(req.name, "us-east-tester");
            assert_eq!(req.provider, "azure");
            assert_eq!(req.region, "eastus");
            assert_eq!(req.vm_size, "Standard_B1s");
        }

        #[test]
        fn missing_field_rejected() {
            let json = r#"{"name": "test", "provider": "azure"}"#;
            assert!(serde_json::from_str::<DeployVmRequest>(json).is_err());
        }
    }

    /// Tests for allowed regions/sizes constants.
    mod allowed_values {
        use super::super::{ALLOWED_REGIONS, ALLOWED_VM_SIZES};

        #[test]
        fn common_regions_present() {
            assert!(ALLOWED_REGIONS.contains(&"eastus"));
            assert!(ALLOWED_REGIONS.contains(&"westeurope"));
            assert!(!ALLOWED_REGIONS.contains(&"invalid-region"));
        }

        #[test]
        fn common_sizes_present() {
            assert!(ALLOWED_VM_SIZES.contains(&"Standard_B1s"));
            assert!(ALLOWED_VM_SIZES.contains(&"Standard_D2s_v3"));
            assert!(!ALLOWED_VM_SIZES.contains(&"Standard_E64s_v5"));
        }
    }
}
