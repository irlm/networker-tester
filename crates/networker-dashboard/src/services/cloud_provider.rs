//! Provider-agnostic cloud VM lifecycle abstraction.
//!
//! Each cloud backend (currently Azure only) implements the same six
//! operations: create, start, stop, delete, get-state, and tag.
//! The `CloudProvider` enum dispatches to the concrete provider based on the
//! `cloud_connection.provider` column value and the `config` JSONB payload.
//!
//! All Azure operations shell out to the `az` CLI with explicit
//! `--subscription` and `--resource-group` flags — no ambient defaults.

use anyhow::{anyhow, Context};
use std::collections::HashMap;

// ── Data types ──────────────────────────────────────────────────────────────

/// Configuration for creating a new VM.
#[derive(Debug, Clone)]
pub struct VmConfig {
    pub name: String,
    pub region: String,
    pub vm_size: String,
    pub ssh_user: String,
    pub image: String,
    pub tags: HashMap<String, String>,
}

/// Information about an existing VM.
#[derive(Debug, Clone)]
pub struct VmInfo {
    pub resource_id: String,
    pub public_ip: String,
    pub vm_name: String,
    pub power_state: String,
}

// ── Provider enum ───────────────────────────────────────────────────────────

/// Provider-agnostic VM lifecycle dispatcher.
#[derive(Debug, Clone)]
pub enum CloudProvider {
    Azure(AzureProvider),
}

impl CloudProvider {
    /// Build a provider from the `cloud_connection` row's `provider` string
    /// and `config` JSONB value.
    pub fn from_connection(
        conn_provider: &str,
        conn_config: &serde_json::Value,
    ) -> anyhow::Result<Self> {
        match conn_provider {
            "azure" => Ok(CloudProvider::Azure(AzureProvider::from_config(
                conn_config,
            )?)),
            other => Err(anyhow!("unsupported cloud provider: {other}")),
        }
    }

    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo> {
        match self {
            CloudProvider::Azure(az) => az.create_vm(config).await,
        }
    }

    pub async fn start_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.start_vm(resource_id).await,
        }
    }

    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.stop_vm(resource_id).await,
        }
    }

    pub async fn delete_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.delete_vm(resource_id).await,
        }
    }

    pub async fn get_vm_state(&self, resource_id: &str) -> anyhow::Result<VmInfo> {
        match self {
            CloudProvider::Azure(az) => az.get_vm_state(resource_id).await,
        }
    }

    pub async fn tag_vm(
        &self,
        resource_id: &str,
        tags: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.tag_vm(resource_id, tags).await,
        }
    }
}

// ── Azure provider ──────────────────────────────────────────────────────────

/// Azure VM lifecycle backed by the `az` CLI.
///
/// Every command includes explicit `--subscription` and `--resource-group`
/// flags — we never rely on the CLI's ambient account/subscription context.
#[derive(Debug, Clone)]
pub struct AzureProvider {
    pub subscription_id: String,
    pub resource_group: String,
    pub identity_type: String,
    /// Service principal credentials (used when identity_type == "service_principal")
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub tenant_id: Option<String>,
}

impl AzureProvider {
    /// Parse the JSONB config from a `cloud_connection` row.
    ///
    /// Expected shape:
    /// ```json
    /// {
    ///   "tenant_id": "...",
    ///   "subscription_id": "...",
    ///   "resource_group": "...",
    ///   "identity_type": "managed_identity"
    /// }
    /// ```
    pub fn from_config(config: &serde_json::Value) -> anyhow::Result<Self> {
        let subscription_id = config
            .get("subscription_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("azure config: missing subscription_id"))?
            .to_string();
        let resource_group = config
            .get("resource_group")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("azure config: missing resource_group"))?
            .to_string();
        let identity_type = config
            .get("identity_type")
            .and_then(|v| v.as_str())
            .unwrap_or("managed_identity")
            .to_string();

        let client_id = config
            .get("client_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let client_secret = config
            .get("client_secret")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let tenant_id_opt = config
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        Ok(Self {
            subscription_id,
            resource_group,
            identity_type,
            client_id,
            client_secret,
            tenant_id: tenant_id_opt,
        })
    }

    /// If service principal credentials are available, login to an isolated
    /// az CLI config dir. Returns the config dir path (set as AZURE_CONFIG_DIR
    /// on subsequent commands). Returns None for managed identity (uses ambient session).
    async fn ensure_sp_login(&self) -> anyhow::Result<Option<String>> {
        let az = Self::az_bin();
        let (cid, csec, tid) = match (&self.client_id, &self.client_secret, &self.tenant_id) {
            (Some(c), Some(s), Some(t)) if self.identity_type == "service_principal" => {
                tracing::info!(az_bin = %az, "SP login: using service principal credentials");
                (c, s, t)
            }
            _ => {
                tracing::info!(
                    az_bin = %az,
                    identity_type = %self.identity_type,
                    has_client_id = self.client_id.is_some(),
                    has_client_secret = self.client_secret.is_some(),
                    has_tenant_id = self.tenant_id.is_some(),
                    "SP login: skipping (no SP credentials or wrong identity_type)"
                );
                return Ok(None);
            }
        };

        let config_dir = format!("/tmp/az-sp-{}", uuid::Uuid::new_v4().simple());
        std::fs::create_dir_all(&config_dir).ok();

        let output = tokio::process::Command::new(Self::az_bin())
            .arg("login")
            .arg("--service-principal")
            .arg("-u")
            .arg(cid)
            .arg("-p")
            .arg(csec)
            .arg("--tenant")
            .arg(tid)
            .arg("--output")
            .arg("none")
            .env("AZURE_CONFIG_DIR", &config_dir)
            .output()
            .await
            .context("failed to spawn az login")?;

        if !output.status.success() {
            let _ = std::fs::remove_dir_all(&config_dir);
            anyhow::bail!(
                "az login --service-principal failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(Some(config_dir))
    }

    /// Resolve the `az` binary path. Checks (in order):
    /// 1. `AZ_CMD` env var
    /// 2. `/tmp/az-cmd-override` file (for dev — contains a path)
    /// 3. Default: `az` on PATH
    fn az_bin() -> String {
        if let Ok(v) = std::env::var("AZ_CMD") {
            if !v.is_empty() {
                return v;
            }
        }
        if let Ok(path) = std::fs::read_to_string("/tmp/az-cmd-override") {
            let path = path.trim();
            if !path.is_empty() && std::path::Path::new(path).exists() {
                return path.to_string();
            }
        }
        "az".to_string()
    }

    /// Build an `az` command with the correct auth context.
    /// Sets PYTHONWARNINGS=ignore to suppress Python SyntaxWarnings that
    /// pollute stderr/stdout and break JSON parsing.
    async fn az_cmd(&self, config_dir: &Option<String>) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(Self::az_bin());
        cmd.env("PYTHONWARNINGS", "ignore");
        if let Some(dir) = config_dir {
            cmd.env("AZURE_CONFIG_DIR", dir);
        }
        cmd
    }

    /// Clean up the SP login session.
    fn cleanup_sp_session(config_dir: &Option<String>) {
        if let Some(dir) = config_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Create a new Azure VM via `az vm create`.
    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo> {
        tracing::info!(
            subscription = %self.subscription_id,
            resource_group = %self.resource_group,
            identity_type = %self.identity_type,
            has_client_id = self.client_id.is_some(),
            vm_name = %config.name,
            region = %config.region,
            vm_size = %config.vm_size,
            "AzureProvider::create_vm"
        );
        let sp_dir = self.ensure_sp_login().await?;
        let mut cmd = self.az_cmd(&sp_dir).await;
        cmd.arg("vm")
            .arg("create")
            .arg("--subscription")
            .arg(&self.subscription_id)
            .arg("--resource-group")
            .arg(&self.resource_group)
            .arg("--name")
            .arg(&config.name)
            .arg("--location")
            .arg(&config.region)
            .arg("--image")
            .arg(&config.image)
            .arg("--size")
            .arg(&config.vm_size)
            .arg("--public-ip-sku")
            .arg("Standard")
            .arg("--admin-username")
            .arg(&config.ssh_user)
            .arg("--generate-ssh-keys")
            .arg("--output")
            .arg("json");

        // Append tags as `key=value` pairs.
        if !config.tags.is_empty() {
            cmd.arg("--tags");
            for (k, v) in &config.tags {
                cmd.arg(format!("{k}={v}"));
            }
        }

        let output = cmd
            .output()
            .await
            .context("failed to spawn `az vm create`")?;
        Self::cleanup_sp_session(&sp_dir);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::error!(
                %stderr,
                %stdout,
                status = ?output.status.code(),
                "az vm create failed"
            );
            anyhow::bail!("az vm create failed: {stderr}");
        }

        // Strip any non-JSON prefix (az CLI may print warnings before JSON)
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let json_start = stdout_str.find('{').unwrap_or(0);
        let v: serde_json::Value = serde_json::from_str(&stdout_str[json_start..])
            .context("az vm create produced non-JSON output")?;

        let public_ip = v
            .get("publicIpAddress")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("az vm create: missing publicIpAddress"))?
            .to_string();
        let resource_id = v
            .get("id")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("az vm create: missing id"))?
            .to_string();

        Ok(VmInfo {
            resource_id,
            public_ip,
            vm_name: config.name.clone(),
            power_state: "running".to_string(),
        })
    }

    /// Start a stopped (deallocated) VM.
    pub async fn start_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let sp_dir = self.ensure_sp_login().await?;
        let output = self
            .az_cmd(&sp_dir)
            .await
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
        Self::cleanup_sp_session(&sp_dir);
        if !output.status.success() {
            anyhow::bail!(
                "az vm start failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Deallocate (stop-billing) a running VM.
    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let sp_dir = self.ensure_sp_login().await?;
        let output = self
            .az_cmd(&sp_dir)
            .await
            .arg("vm")
            .arg("deallocate")
            .arg("--subscription")
            .arg(&self.subscription_id)
            .arg("--resource-group")
            .arg(&self.resource_group)
            .arg("--ids")
            .arg(resource_id)
            .output()
            .await
            .context("failed to spawn `az vm deallocate`")?;
        Self::cleanup_sp_session(&sp_dir);
        if !output.status.success() {
            anyhow::bail!(
                "az vm deallocate failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Permanently delete a VM and its associated resources.
    pub async fn delete_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let sp_dir = self.ensure_sp_login().await?;
        let output = self
            .az_cmd(&sp_dir)
            .await
            .arg("vm")
            .arg("delete")
            .arg("--subscription")
            .arg(&self.subscription_id)
            .arg("--resource-group")
            .arg(&self.resource_group)
            .arg("--ids")
            .arg(resource_id)
            .arg("--yes")
            .output()
            .await
            .context("failed to spawn `az vm delete`")?;
        Self::cleanup_sp_session(&sp_dir);
        if !output.status.success() {
            anyhow::bail!(
                "az vm delete failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Query the current power state and public IP of a VM.
    pub async fn get_vm_state(&self, resource_id: &str) -> anyhow::Result<VmInfo> {
        let sp_dir = self.ensure_sp_login().await?;
        let output = self
            .az_cmd(&sp_dir)
            .await
            .arg("vm")
            .arg("show")
            .arg("--subscription")
            .arg(&self.subscription_id)
            .arg("--resource-group")
            .arg(&self.resource_group)
            .arg("--ids")
            .arg(resource_id)
            .arg("--show-details")
            .arg("--output")
            .arg("json")
            .output()
            .await
            .context("failed to spawn `az vm show`")?;
        Self::cleanup_sp_session(&sp_dir);

        if !output.status.success() {
            anyhow::bail!(
                "az vm show failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let v: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("az vm show produced non-JSON output")?;

        let vm_name = v
            .get("name")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let public_ip = v
            .get("publicIps")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let power_state = v
            .get("powerState")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();
        let resource_id_out = v
            .get("id")
            .and_then(|s| s.as_str())
            .unwrap_or(resource_id)
            .to_string();

        Ok(VmInfo {
            resource_id: resource_id_out,
            public_ip,
            vm_name,
            power_state,
        })
    }

    /// Set or update tags on an existing VM.
    pub async fn tag_vm(
        &self,
        resource_id: &str,
        tags: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        if tags.is_empty() {
            return Ok(());
        }

        let sp_dir = self.ensure_sp_login().await?;
        let mut cmd = self.az_cmd(&sp_dir).await;
        cmd.arg("resource")
            .arg("tag")
            .arg("--subscription")
            .arg(&self.subscription_id)
            .arg("--resource-group")
            .arg(&self.resource_group)
            .arg("--ids")
            .arg(resource_id)
            .arg("--tags");
        for (k, v) in tags {
            cmd.arg(format!("{k}={v}"));
        }

        let output = cmd
            .output()
            .await
            .context("failed to spawn `az resource tag`")?;
        Self::cleanup_sp_session(&sp_dir);
        if !output.status.success() {
            anyhow::bail!(
                "az resource tag failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}

// ── Legacy fallback ────────────────────────────────────────────────────────

/// Build a `CloudProvider::Azure` from the legacy env-var convention used by
/// testers created before `cloud_connection_id` was added to `project_tester`.
/// This keeps existing v0.25.x testers working until the migration (Task 4)
/// backfills the FK and the API (Task 5) requires it on creation.
pub fn legacy_azure_provider() -> anyhow::Result<CloudProvider> {
    let sub = std::env::var("AZURE_SUBSCRIPTION_ID")
        .or_else(|_| std::env::var("DASHBOARD_AZURE_SUBSCRIPTION"))
        .unwrap_or_default();
    if sub.is_empty() {
        anyhow::bail!(
            "No Azure subscription configured. Either:\n\
             1. Add a Cloud Account (Settings > Cloud > Add Account) with Azure credentials, or\n\
             2. Add a Cloud Connection (Settings > Cloud Connections) with managed identity config, or\n\
             3. Set AZURE_SUBSCRIPTION_ID environment variable on the dashboard"
        );
    }
    let rg =
        std::env::var("DASHBOARD_AZURE_RG").unwrap_or_else(|_| "networker-testers".to_string());
    let config = serde_json::json!({
        "tenant_id": "",
        "subscription_id": sub,
        "resource_group": rg,
        "identity_type": "managed_identity"
    });
    CloudProvider::from_connection("azure", &config)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a short, DNS-safe VM name: `tester-{region}-{5 hex chars}`.
pub fn generate_vm_name(region: &str) -> String {
    let suffix: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(5)
        .collect();
    format!("tester-{region}-{suffix}")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_provider_from_valid_config() {
        let config = serde_json::json!({
            "tenant_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "subscription_id": "11111111-2222-3333-4444-555555555555",
            "resource_group": "my-rg",
            "identity_type": "managed_identity"
        });

        let provider = AzureProvider::from_config(&config).unwrap();
        assert_eq!(
            provider.subscription_id,
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(provider.resource_group, "my-rg");
        assert_eq!(provider.identity_type, "managed_identity");
    }

    #[test]
    fn azure_provider_rejects_missing_subscription() {
        let config = serde_json::json!({
            "tenant_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "resource_group": "my-rg"
        });

        let err = AzureProvider::from_config(&config).unwrap_err();
        assert!(
            err.to_string().contains("subscription_id"),
            "expected error about subscription_id, got: {err}"
        );
    }

    #[test]
    fn from_connection_rejects_unknown_provider() {
        let config = serde_json::json!({});

        let err = CloudProvider::from_connection("aws", &config).unwrap_err();
        assert!(
            err.to_string().contains("unsupported cloud provider"),
            "expected 'unsupported cloud provider', got: {err}"
        );

        let err = CloudProvider::from_connection("gcp", &config).unwrap_err();
        assert!(
            err.to_string().contains("unsupported cloud provider"),
            "expected 'unsupported cloud provider', got: {err}"
        );
    }

    #[test]
    fn generate_vm_name_contains_region() {
        let name = generate_vm_name("eastus");
        assert!(name.starts_with("tester-eastus-"));
        assert!(name.len() > "tester-eastus-".len());
    }

    /// Recursively collect all `.rs` files under a directory.
    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_rs_files(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
    }

    #[test]
    fn cloud_provider_never_touches_stored_credentials() {
        // Walk services/ for forbidden patterns. The cloud_provider module
        // itself must never reference stored credentials — it receives
        // config values, not encrypted blobs.
        //
        // Note: api/testers.rs is excluded because provider_for_tester()
        // legitimately decrypts cloud_account credentials to build a
        // CloudProvider config. The FIC principle applies to the provider
        // abstraction layer, not the orchestration layer above it.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // Build patterns at runtime so this test file doesn't match itself.
        let forbidden = [
            format!("credentials{}", "_enc"),
            format!("credentials{}", "_nonce"),
            format!("crypto::{}", "decrypt"),
        ];
        let mut violations = Vec::new();

        let mut files = Vec::new();
        collect_rs_files(&root.join("services"), &mut files);

        for path in &files {
            let content = std::fs::read_to_string(path).unwrap();
            for pattern in &forbidden {
                if content.contains(pattern.as_str()) {
                    violations.push(format!("{}:{}", path.display(), pattern));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "FIC violation: cloud provider services reference stored credentials: {:?}",
            violations
        );
    }

    #[test]
    fn azure_provider_defaults_identity_type() {
        let config = serde_json::json!({
            "subscription_id": "sub-123",
            "resource_group": "rg-test"
        });

        let provider = AzureProvider::from_config(&config).unwrap();
        assert_eq!(provider.identity_type, "managed_identity");
    }
}
