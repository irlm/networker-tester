//! VM lifecycle: tester lock release guard, Azure start/lookup helpers,
//! VM resolution/validation, and teardown.

use super::status::log_callback;
use crate::callback::CallbackClient;
use crate::config::TestbedConfig;
use crate::provisioner::{self, VmInfo};
use crate::ssh;
use crate::tester_state;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio_postgres::Client as PgClient;
use uuid::Uuid;

/// Drop-safe guard that releases the tester lock on scope exit. The preferred
/// path is `release_now().await` which synchronously releases and marks the
/// guard as consumed. If a panic or early `return` skips that call, the `Drop`
/// impl spawns a background task to release the lock — best-effort; if the
/// tokio runtime is shutting down the release may be lost and the dashboard's
/// crash-recovery sweep (Task 12) will reclaim the lock.
pub(super) struct ReleaseGuard {
    client: Arc<PgClient>,
    tester_id: Uuid,
    config_id: Uuid,
    released: bool,
}

impl ReleaseGuard {
    pub(super) fn new(client: Arc<PgClient>, tester_id: Uuid, config_id: Uuid) -> Self {
        Self {
            client,
            tester_id,
            config_id,
            released: false,
        }
    }

    pub(super) async fn release_now(mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if let Err(e) = tester_state::release(&self.client, &self.tester_id, &self.config_id).await
        {
            tracing::error!(
                tester_id = %self.tester_id,
                config_id = %self.config_id,
                "failed to release tester lock: {e:#}"
            );
        }
    }
}

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let client = self.client.clone();
        let tid = self.tester_id;
        let cid = self.config_id;
        // Best-effort: spawn on the current runtime if one is available.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = tester_state::release(&client, &tid, &cid).await {
                    tracing::error!(
                        tester_id = %tid,
                        config_id = %cid,
                        "Drop release failed: {e:#}"
                    );
                }
            });
        } else {
            tracing::warn!(
                tester_id = %tid,
                config_id = %cid,
                "ReleaseGuard dropped without tokio runtime — crash recovery must reclaim lock"
            );
        }
    }
}

/// Start a stopped tester VM via the cloud provider and wait for SSH to come up.
///
/// Uses `OrchestratorCloudProvider` (Option A per FIC plan) -- a minimal
/// duplicate of the dashboard's `CloudProvider`. A future PR can extract
/// both into `networker-common`.
pub(super) async fn ensure_running_via_azure(
    tester: &ProjectTesterRow,
    db: &tokio_postgres::Client,
) -> Result<()> {
    let resource_id = tester
        .vm_resource_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("tester {} has no vm_resource_id", tester.tester_id))?;
    let ip = tester
        .public_ip
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("tester {} has no public_ip", tester.tester_id))?;

    tracing::info!(
        tester_id = %tester.tester_id,
        %resource_id,
        "starting stopped tester VM via cloud provider"
    );

    let provider = load_cloud_provider(db, &tester.cloud_connection_id).await?;
    provider.start_vm(resource_id).await?;

    // Poll SSH up to ~5 minutes.
    for attempt in 1..=30u32 {
        match ssh::ssh_exec(ip, "echo ok").await {
            Ok(_) => {
                tracing::info!(tester_id = %tester.tester_id, attempt, "SSH ready after VM start");
                return Ok(());
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
    anyhow::bail!("SSH not available on {ip} within 5 minutes after VM start")
}

// ── Minimal cloud provider (Option A duplication) ─────────────────────────

/// Minimal cloud provider for the orchestrator -- duplicated from the dashboard
/// crate's `CloudProvider` (Option A per FIC plan). A future PR can extract a
/// shared crate. Currently only supports Azure `az` CLI operations.
#[derive(Debug, Clone)]
struct OrchestratorCloudProvider {
    subscription_id: String,
    resource_group: String,
}

impl OrchestratorCloudProvider {
    /// Parse the JSONB `config` column from `cloud_connection`.
    fn from_config(config: &serde_json::Value) -> Result<Self> {
        let subscription_id = config
            .get("subscription_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("azure config: missing subscription_id"))?
            .to_string();
        let resource_group = config
            .get("resource_group")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("azure config: missing resource_group"))?
            .to_string();
        Ok(Self {
            subscription_id,
            resource_group,
        })
    }

    /// Build from legacy env vars (testers without a `cloud_connection_id`).
    fn legacy_fallback() -> Result<Self> {
        let sub = std::env::var("AZURE_SUBSCRIPTION_ID")
            .or_else(|_| std::env::var("DASHBOARD_AZURE_SUBSCRIPTION"))
            .unwrap_or_default();
        let rg =
            std::env::var("DASHBOARD_AZURE_RG").unwrap_or_else(|_| "networker-testers".to_string());
        Ok(Self {
            subscription_id: sub,
            resource_group: rg,
        })
    }

    /// Start a stopped (deallocated) VM via `az vm start`.
    async fn start_vm(&self, resource_id: &str) -> Result<()> {
        let output = tokio::process::Command::new("az")
            .arg("vm")
            .arg("start")
            .arg("--subscription")
            .arg(&self.subscription_id)
            .arg("--resource-group")
            .arg(&self.resource_group)
            .arg("--ids")
            .arg(resource_id)
            .output()
            .await
            .context("failed to spawn `az vm start`")?;
        if !output.status.success() {
            anyhow::bail!(
                "az vm start failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}

/// Load the cloud provider for a tester. If the tester has a
/// `cloud_connection_id`, fetch the connection row and construct from its
/// config. Otherwise fall back to legacy env vars.
async fn load_cloud_provider(
    client: &tokio_postgres::Client,
    cloud_connection_id: &Option<Uuid>,
) -> Result<OrchestratorCloudProvider> {
    if let Some(conn_id) = cloud_connection_id {
        let row = client
            .query_one(
                "SELECT provider, config FROM cloud_connection WHERE connection_id = $1",
                &[conn_id],
            )
            .await
            .with_context(|| format!("loading cloud_connection {conn_id}"))?;
        let provider: String = row.get("provider");
        if provider != "azure" {
            anyhow::bail!("orchestrator only supports azure provider, got: {provider}");
        }
        let config: serde_json::Value = row.get("config");
        OrchestratorCloudProvider::from_config(&config)
    } else {
        OrchestratorCloudProvider::legacy_fallback()
    }
}

/// Subset of the dashboard's `project_tester` row that the orchestrator needs
/// when executing an application benchmark against a persistent tester.
///
/// This is defined locally (rather than imported from `networker-dashboard`)
/// because the orchestrator is a standalone crate that talks to Postgres
/// directly via tokio-postgres. Only the columns consumed by the executor
/// are included — extend as needed.
///
/// `dead_code` is allowed because Task 23 (the `execute_testbed_application`
/// rewrite) is the first caller; this helper is committed independently so
/// Task 23 lands as a pure swap.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectTesterRow {
    pub tester_id: Uuid,
    pub project_id: String,
    pub name: String,
    pub public_ip: Option<String>,
    pub ssh_user: String,
    pub vm_name: Option<String>,
    pub vm_resource_id: Option<String>,
    pub power_state: String,
    pub allocation: String,
    pub installer_version: Option<String>,
    pub cloud_connection_id: Option<Uuid>,
}

/// Look up the persistent tester associated with a given benchmark config.
///
/// Joins `benchmark_config` and `project_tester` on `benchmark_config.tester_id`.
/// Returns an error if the config has no tester (`tester_id IS NULL`) — for
/// application-mode benchmarks the V027 SQL CHECK constraint should make this
/// impossible, but we defend against it so a malformed row fails loudly
/// instead of silently skipping the tester-lock flow.
///
/// Task 23 (`execute_testbed_application` rewrite) is the primary caller.
#[allow(dead_code)]
pub async fn lookup_tester(
    client: &tokio_postgres::Client,
    config_id: &Uuid,
) -> Result<ProjectTesterRow> {
    // `public_ip::text` casts INET → TEXT so tokio-postgres can decode it
    // as `Option<String>` without needing the `with-cidr` feature.
    let row = client
        .query_opt(
            r#"
            SELECT t.tester_id,
                   t.project_id,
                   t.name,
                   t.public_ip::text,
                   t.ssh_user,
                   t.vm_name,
                   t.vm_resource_id,
                   t.power_state,
                   t.allocation,
                   t.installer_version,
                   t.cloud_connection_id
              FROM benchmark_config c
              JOIN project_tester   t ON t.tester_id = c.tester_id
             WHERE c.config_id = $1
            "#,
            &[config_id],
        )
        .await
        .with_context(|| format!("lookup_tester query failed for config {config_id}"))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "benchmark_config {} has no tester_id (or the referenced tester no longer exists)",
                config_id
            )
        })?;

    Ok(ProjectTesterRow {
        tester_id: row.get(0),
        project_id: row.get(1),
        name: row.get(2),
        public_ip: row.get::<_, Option<String>>(3),
        ssh_user: row.get(4),
        vm_name: row.get(5),
        vm_resource_id: row.get(6),
        power_state: row.get(7),
        allocation: row.get(8),
        installer_version: row.get(9),
        cloud_connection_id: row.get(10),
    })
}

/// Validate an IPv4 address string: must be 4 octets 0-255, no shell metacharacters.
/// For cloud-provisioned VMs, blocks link-local (169.254.x.x) and localhost (127.x.x.x).
fn validate_ip(ip: &str, is_cloud: bool) -> Result<()> {
    // Reject any shell metacharacters
    if ip.chars().any(|c| !c.is_ascii_digit() && c != '.') {
        anyhow::bail!("IP address contains invalid characters: {ip}");
    }
    let octets: Vec<&str> = ip.split('.').collect();
    if octets.len() != 4 {
        anyhow::bail!("IP address must have exactly 4 octets: {ip}");
    }
    for octet in &octets {
        let val: u16 = octet
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid octet in IP address: {ip}"))?;
        if val > 255 {
            anyhow::bail!("Octet out of range in IP address: {ip}");
        }
    }
    if is_cloud {
        let first: u8 = octets[0].parse().unwrap();
        let second: u8 = octets[1].parse().unwrap();
        if first == 127 {
            anyhow::bail!("Localhost address not allowed for cloud VMs: {ip}");
        }
        if first == 169 && second == 254 {
            anyhow::bail!("Link-local address not allowed for cloud VMs: {ip}");
        }
        if first == 10 {
            anyhow::bail!("RFC1918 private address not allowed for cloud VMs: {ip}");
        }
        if first == 172 && (16..=31).contains(&second) {
            anyhow::bail!("RFC1918 private address not allowed for cloud VMs: {ip}");
        }
        if first == 192 && second == 168 {
            anyhow::bail!("RFC1918 private address not allowed for cloud VMs: {ip}");
        }
        if first == 0 {
            anyhow::bail!("Invalid address not allowed for cloud VMs: {ip}");
        }
    }
    Ok(())
}

/// Resolve the VM for a testbed: use existing IP or provision a new one.
pub(super) async fn resolve_vm(testbed: &TestbedConfig) -> Result<(VmInfo, bool)> {
    if let Some(ip) = &testbed.existing_vm_ip {
        let is_cloud = ["azure", "aws", "gcp"].contains(&testbed.cloud.to_lowercase().as_str());
        validate_ip(ip, is_cloud).with_context(|| {
            format!("Invalid existing_vm_ip for testbed {}", testbed.testbed_id)
        })?;
        tracing::info!(
            "Using existing VM at {} for testbed {}",
            ip,
            testbed.testbed_id
        );
        // SSH user differs by cloud: azure=azureuser (or root on some images),
        // aws/gcp Ubuntu=ubuntu, aws/gcp Debian=admin. Default based on cloud
        // since we don't know the OS distro for existing VMs without probing.
        let ssh_user = match testbed.cloud.to_lowercase().as_str() {
            "azure" => "azureuser",
            "aws" | "gcp" => "ubuntu",
            _ => "ubuntu",
        }
        .to_string();
        let vm = VmInfo {
            name: format!(
                "existing-{}",
                &testbed.testbed_id[..8.min(testbed.testbed_id.len())]
            ),
            ip: ip.clone(),
            cloud: testbed.cloud.clone(),
            region: testbed.region.clone(),
            os: "ubuntu".to_string(),
            vm_size: testbed.vm_size.clone(),
            resource_group: String::new(),
            ssh_user,
        };
        Ok((vm, false))
    } else {
        // For now, auto-provisioning requires cloud CLI tools (az/aws/gcloud).
        // If none are available, fail fast with a helpful message.
        let vm_name = format!(
            "ab-{}-{}",
            &testbed.testbed_id[..8.min(testbed.testbed_id.len())],
            testbed.region
        );

        // Check if VM already exists.
        if let Some(existing) = provisioner::find_existing_vm(&vm_name).await? {
            if !existing.ip.is_empty() {
                tracing::info!("Reusing existing VM {} at {}", existing.name, existing.ip);
                return Ok((existing, false));
            }
        }

        tracing::info!(
            "Provisioning new VM: name={}, cloud={}, region={}, size={}",
            vm_name,
            testbed.cloud,
            testbed.region,
            testbed.vm_size,
        );

        let cloud_lower = testbed.cloud.to_lowercase();
        let size_lower = testbed.vm_size.to_lowercase();
        let resolved_size = crate::vm_tiers::resolve_vm_size(&cloud_lower, &size_lower);
        let vm = provisioner::provision_vm(
            &testbed.cloud,
            &testbed.region,
            "ubuntu",
            resolved_size,
            &vm_name,
        )
        .await?;
        Ok((vm, true))
    }
}

/// Tear down a provisioned VM for a testbed.
pub(super) async fn teardown_testbed(testbed: &TestbedConfig, callback: &Arc<CallbackClient>) {
    let vm_name = format!(
        "ab-{}-{}",
        &testbed.testbed_id[..8.min(testbed.testbed_id.len())],
        testbed.region
    );

    log_callback(
        callback,
        &testbed.testbed_id,
        vec![format!("Tearing down VM {vm_name}...")],
    )
    .await;

    // Find and destroy the VM.
    match provisioner::find_existing_vm(&vm_name).await {
        Ok(Some(vm)) => {
            if let Err(e) = provisioner::destroy_vm(&vm).await {
                tracing::error!("Failed to destroy VM {}: {e:#}", vm_name);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Teardown failed for {vm_name}: {e:#}")],
                )
                .await;
            } else {
                tracing::info!("VM {} destroyed", vm_name);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("VM {vm_name} destroyed")],
                )
                .await;
            }
        }
        Ok(None) => {
            tracing::debug!("VM {} not found, nothing to tear down", vm_name);
        }
        Err(e) => {
            tracing::warn!("Failed to look up VM {} for teardown: {e}", vm_name);
        }
    }
}
