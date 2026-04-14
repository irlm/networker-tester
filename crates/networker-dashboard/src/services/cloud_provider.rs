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
    /// Optional cloud-init / user-data script that the provider should inject
    /// at instance creation time. For AWS this maps to `--user-data`; for GCP
    /// to `--metadata-from-file startup-script=`; for Azure to `--custom-data`.
    pub bootstrap_script: Option<String>,
}

/// Resolve the image reference for a given (cloud, os, variant) triple.
/// Returns the provider-specific image reference to pass as VmConfig.image.
pub fn resolve_image(cloud: &str, os: &str, variant: &str) -> String {
    match (cloud, os, variant) {
        // Azure — URN format: Publisher:Offer:Sku:Version
        // Note: Azure Ubuntu Desktop is not a published image; fall back to server.
        ("azure", "ubuntu-24.04", "server" | "desktop") => {
            "Canonical:ubuntu-24_04-lts:server:latest".into()
        }
        ("azure", "ubuntu-22.04", "server") => {
            "Canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest".into()
        }
        ("azure", "windows-2022", "server") => {
            "MicrosoftWindowsServer:WindowsServer:2022-datacenter-azure-edition:latest".into()
        }
        ("azure", "windows-11", "desktop") => {
            "MicrosoftWindowsDesktop:windows-11:win11-24h2-pro:latest".into()
        }
        ("azure", "debian-12", "server") => "Debian:debian-12:12:latest".into(),

        // AWS — pass a marker; create_vm will query SSM/describe-images
        ("aws", "ubuntu-24.04", "server") => "aws:ubuntu-24.04-server".into(),
        ("aws", "ubuntu-22.04", "server") => "aws:ubuntu-22.04-server".into(),
        ("aws", "windows-2022", "server") => "aws:windows-2022-server".into(),
        ("aws", "debian-12", "server") => "aws:debian-12-server".into(),

        // GCP — image family
        ("gcp", "ubuntu-24.04", "server") => "ubuntu-2404-lts-amd64".into(),
        ("gcp", "ubuntu-22.04", "server") => "ubuntu-2204-lts".into(),
        ("gcp", "debian-12", "server") => "debian-12".into(),
        ("gcp", "windows-2022", "server") => "windows-2022".into(),

        // Fallback: Ubuntu 24.04 Server
        ("azure", _, _) => "Canonical:ubuntu-24_04-lts:server:latest".into(),
        ("aws", _, _) => "aws:ubuntu-24.04-server".into(),
        ("gcp", _, _) => "ubuntu-2404-lts-amd64".into(),
        _ => "ubuntu-24.04-server".into(),
    }
}

/// Derive a Windows-safe NetBIOS computer name from a descriptive VM name.
/// - Max 15 chars
/// - Only letters, digits, hyphens; punctuation → hyphens
/// - Must not be entirely numeric (prefixed with "w" if it would be)
/// - Trailing hyphens stripped
pub fn azure_windows_computer_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    if s.len() > 15 {
        s.truncate(15);
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() || s.chars().all(|c| c.is_ascii_digit()) {
        s = format!("w{s}");
        if s.len() > 15 {
            s.truncate(15);
        }
    }
    s
}

/// Default SSH user for a given OS.
pub fn default_ssh_user(cloud: &str, os: &str) -> &'static str {
    if os.starts_with("windows") {
        return "azureadmin"; // "Administrator" is reserved on Azure Windows images
    }
    match (cloud, os) {
        // Azure disallows "admin" as username — use "azureuser" for all Azure Linux
        ("azure", _) => "azureuser",
        // AWS/GCP Debian uses "admin", Ubuntu uses "ubuntu"
        ("aws", "debian-12") => "admin",
        ("gcp", "debian-12") => "admin",
        ("aws", _) => "ubuntu",
        ("gcp", _) => "ubuntu",
        _ => "ubuntu",
    }
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
    Aws(AwsProvider),
    Gcp(GcpProvider),
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
            "aws" => Ok(CloudProvider::Aws(AwsProvider::from_config(conn_config)?)),
            "gcp" => Ok(CloudProvider::Gcp(GcpProvider::from_config(conn_config)?)),
            other => Err(anyhow!("unsupported cloud provider: {other}")),
        }
    }

    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo> {
        match self {
            CloudProvider::Azure(az) => az.create_vm(config).await,
            CloudProvider::Aws(aws) => aws.create_vm(config).await,
            CloudProvider::Gcp(gcp) => gcp.create_vm(config).await,
        }
    }

    pub async fn start_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.start_vm(resource_id).await,
            CloudProvider::Aws(aws) => aws.start_vm(resource_id).await,
            CloudProvider::Gcp(gcp) => gcp.start_vm(resource_id).await,
        }
    }

    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.stop_vm(resource_id).await,
            CloudProvider::Aws(aws) => aws.stop_vm(resource_id).await,
            CloudProvider::Gcp(gcp) => gcp.stop_vm(resource_id).await,
        }
    }

    pub async fn delete_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.delete_vm(resource_id).await,
            CloudProvider::Aws(aws) => aws.delete_vm(resource_id).await,
            CloudProvider::Gcp(gcp) => gcp.delete_vm(resource_id).await,
        }
    }

    pub async fn get_vm_state(&self, resource_id: &str) -> anyhow::Result<VmInfo> {
        match self {
            CloudProvider::Azure(az) => az.get_vm_state(resource_id).await,
            CloudProvider::Aws(aws) => aws.get_vm_state(resource_id).await,
            CloudProvider::Gcp(gcp) => gcp.get_vm_state(resource_id).await,
        }
    }

    pub async fn tag_vm(
        &self,
        resource_id: &str,
        tags: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        match self {
            CloudProvider::Azure(az) => az.tag_vm(resource_id, tags).await,
            CloudProvider::Aws(aws) => aws.tag_vm(resource_id, tags).await,
            CloudProvider::Gcp(gcp) => gcp.tag_vm(resource_id, tags).await,
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
        let is_windows = config.image.to_lowercase().contains("windows");
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
            .arg(&config.ssh_user);

        // Windows NetBIOS computer name is limited to 15 chars and may not be
        // all-numeric or contain punctuation besides `-`. The Azure resource
        // name (config.name) can stay descriptive; only the OS-level computer
        // name is constrained. Derive a safe 15-char slug for Windows.
        if is_windows {
            let safe = azure_windows_computer_name(&config.name);
            cmd.arg("--computer-name").arg(&safe);
        }

        // Windows VMs require a password; Linux VMs use SSH keys.
        let win_password = if is_windows {
            // Azure password rules: 12-72 chars, 3 of {upper, lower, digit, special}
            let pw = format!(
                "Nx!{}{}aZ9",
                uuid::Uuid::new_v4().simple(),
                &config.name.chars().take(4).collect::<String>()
            );
            cmd.arg("--admin-password").arg(&pw);
            Some(pw)
        } else {
            cmd.arg("--generate-ssh-keys");
            None
        };
        let _ = win_password; // currently not surfaced; logged below for ops.

        cmd.arg("--output").arg("json");

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
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Idempotent: VM already gone is the desired end-state.
            if stderr.contains("ResourceNotFound") || stderr.contains("could not be found") {
                tracing::info!(resource_id, "Azure VM already deleted; treating as success");
                return Ok(());
            }
            anyhow::bail!("az vm delete failed: {stderr}");
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

// ── AWS provider ───────────────────────────────────────────────────────────

/// AWS EC2 VM lifecycle backed by the `aws` CLI.
#[derive(Debug, Clone)]
pub struct AwsProvider {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
}

impl AwsProvider {
    pub fn from_config(config: &serde_json::Value) -> anyhow::Result<Self> {
        let access_key_id = config
            .get("access_key_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let secret_access_key = config
            .get("secret_access_key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let session_token = config
            .get("session_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let region = config
            .get("region")
            .and_then(|v| v.as_str())
            .unwrap_or("us-east-1")
            .to_string();

        if access_key_id.is_empty() || secret_access_key.is_empty() {
            anyhow::bail!("aws config: missing access_key_id or secret_access_key");
        }

        Ok(Self {
            access_key_id,
            secret_access_key,
            session_token,
            region,
        })
    }

    fn aws_cmd(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("aws");
        cmd.env("AWS_ACCESS_KEY_ID", &self.access_key_id)
            .env("AWS_SECRET_ACCESS_KEY", &self.secret_access_key)
            .env("AWS_DEFAULT_REGION", &self.region);
        if let Some(ref token) = self.session_token {
            cmd.env("AWS_SESSION_TOKEN", token);
        }
        cmd
    }

    /// Ensure a key pair named `alethedash-tester` exists in the region.
    /// If the local ~/.ssh/id_rsa.pub exists, imports it. Otherwise creates
    /// a new key and saves the private key to ~/.ssh/alethedash-tester.pem.
    async fn ensure_key_pair(&self, region: &str) -> anyhow::Result<String> {
        let key_name = "alethedash-tester";

        // Check if key pair exists
        let check = self
            .aws_cmd()
            .arg("ec2")
            .arg("describe-key-pairs")
            .arg("--key-names")
            .arg(key_name)
            .arg("--region")
            .arg(region)
            .arg("--output")
            .arg("json")
            .output()
            .await
            .context("failed to spawn aws ec2 describe-key-pairs")?;

        if check.status.success() {
            tracing::info!(key_name, "Key pair already exists");
            return Ok(key_name.to_string());
        }

        // Try to import local public key
        let home = std::env::var("HOME").unwrap_or_default();
        let pub_key_path = format!("{home}/.ssh/id_rsa.pub");
        if std::path::Path::new(&pub_key_path).exists() {
            tracing::info!(key_name, %pub_key_path, "Importing local SSH public key");
            let pub_key = std::fs::read_to_string(&pub_key_path)?;
            let import = self
                .aws_cmd()
                .arg("ec2")
                .arg("import-key-pair")
                .arg("--key-name")
                .arg(key_name)
                .arg("--public-key-material")
                .arg(format!("fileb://{pub_key_path}"))
                .arg("--region")
                .arg(region)
                .output()
                .await
                .context("failed to spawn aws ec2 import-key-pair")?;
            if import.status.success() {
                tracing::info!(key_name, "Key pair imported");
                return Ok(key_name.to_string());
            }
            tracing::warn!(
                stderr = %String::from_utf8_lossy(&import.stderr),
                "Key import failed, will try create-key-pair"
            );
            drop(pub_key);
        }

        // Create new key pair
        let create = self
            .aws_cmd()
            .arg("ec2")
            .arg("create-key-pair")
            .arg("--key-name")
            .arg(key_name)
            .arg("--query")
            .arg("KeyMaterial")
            .arg("--region")
            .arg(region)
            .arg("--output")
            .arg("text")
            .output()
            .await
            .context("failed to spawn aws ec2 create-key-pair")?;

        if !create.status.success() {
            anyhow::bail!(
                "create-key-pair failed: {}",
                String::from_utf8_lossy(&create.stderr)
            );
        }
        let pem_path = format!("{home}/.ssh/{key_name}.pem");
        std::fs::write(&pem_path, create.stdout)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&pem_path, std::fs::Permissions::from_mode(0o600));
        }
        tracing::info!(key_name, %pem_path, "Created new key pair");
        Ok(key_name.to_string())
    }

    /// Ensure a security group named `alethedash-tester` exists allowing
    /// SSH (22), networker-endpoint (8080/8443), and probe ports (8443 UDP, 9998-9999 UDP).
    async fn ensure_security_group(&self, region: &str) -> anyhow::Result<String> {
        let sg_name = "alethedash-tester";

        // Check if SG exists
        let check = self
            .aws_cmd()
            .arg("ec2")
            .arg("describe-security-groups")
            .arg("--group-names")
            .arg(sg_name)
            .arg("--query")
            .arg("SecurityGroups[0].GroupId")
            .arg("--region")
            .arg(region)
            .arg("--output")
            .arg("text")
            .output()
            .await
            .context("failed to spawn aws ec2 describe-security-groups")?;

        if check.status.success() {
            let sg_id = String::from_utf8_lossy(&check.stdout).trim().to_string();
            if !sg_id.is_empty() && sg_id != "None" {
                tracing::info!(%sg_id, "Security group already exists");
                return Ok(sg_id);
            }
        }

        // Create the security group
        let create = self
            .aws_cmd()
            .arg("ec2")
            .arg("create-security-group")
            .arg("--group-name")
            .arg(sg_name)
            .arg("--description")
            .arg("AletheDash tester (SSH + diagnostic ports)")
            .arg("--query")
            .arg("GroupId")
            .arg("--region")
            .arg(region)
            .arg("--output")
            .arg("text")
            .output()
            .await
            .context("failed to spawn aws ec2 create-security-group")?;

        if !create.status.success() {
            anyhow::bail!(
                "create-security-group failed: {}",
                String::from_utf8_lossy(&create.stderr)
            );
        }
        let sg_id = String::from_utf8_lossy(&create.stdout).trim().to_string();

        // Add ingress rules: SSH (22), HTTP/S diagnostic ports (8080, 8443), UDP probes (8443, 9998, 9999)
        for (proto, port) in &[
            ("tcp", "22"),
            ("tcp", "8080"),
            ("tcp", "8443"),
            ("udp", "8443"),
            ("udp", "9998"),
            ("udp", "9999"),
        ] {
            let _ = self
                .aws_cmd()
                .arg("ec2")
                .arg("authorize-security-group-ingress")
                .arg("--group-id")
                .arg(&sg_id)
                .arg("--protocol")
                .arg(proto)
                .arg("--port")
                .arg(port)
                .arg("--cidr")
                .arg("0.0.0.0/0")
                .arg("--region")
                .arg(region)
                .output()
                .await;
        }

        tracing::info!(%sg_id, "Created security group with ingress rules");
        Ok(sg_id)
    }

    /// Resolve a marker like "aws:ubuntu-24.04-server" into an AMI ID for the given region.
    async fn resolve_ami(&self, marker: &str, region: &str) -> anyhow::Result<String> {
        let (owner, name_filter) = match marker.strip_prefix("aws:").unwrap_or(marker) {
            "ubuntu-24.04-server" => (
                "099720109477",
                "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*",
            ),
            "ubuntu-22.04-server" => (
                "099720109477",
                "ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*",
            ),
            "debian-12-server" => ("136693071363", "debian-12-amd64-*"),
            "windows-2022-server" => ("801119661308", "Windows_Server-2022-English-Full-Base-*"),
            _ => (
                "099720109477",
                "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*",
            ),
        };

        let output = self
            .aws_cmd()
            .arg("ec2")
            .arg("describe-images")
            .arg("--owners")
            .arg(owner)
            .arg("--filters")
            .arg(format!("Name=name,Values={name_filter}"))
            .arg("Name=state,Values=available")
            .arg("--query")
            .arg("sort_by(Images, &CreationDate)[-1].ImageId")
            .arg("--region")
            .arg(region)
            .arg("--output")
            .arg("text")
            .output()
            .await
            .context("aws ec2 describe-images")?;
        if !output.status.success() {
            anyhow::bail!(
                "aws ec2 describe-images failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let ami = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ami.is_empty() || ami == "None" {
            anyhow::bail!("No AMI found for '{marker}' in region {region}");
        }
        Ok(ami)
    }

    /// Poll for the instance's public IP for up to 60s.
    async fn wait_for_public_ip(&self, instance_id: &str, region: &str) -> anyhow::Result<String> {
        for _ in 0..30u32 {
            let output = self
                .aws_cmd()
                .arg("ec2")
                .arg("describe-instances")
                .arg("--instance-ids")
                .arg(instance_id)
                .arg("--query")
                .arg("Reservations[0].Instances[0].PublicIpAddress")
                .arg("--region")
                .arg(region)
                .arg("--output")
                .arg("text")
                .output()
                .await
                .context("failed to spawn aws ec2 describe-instances")?;
            if output.status.success() {
                let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !ip.is_empty() && ip != "None" {
                    tracing::info!(%instance_id, %ip, "Public IP assigned");
                    return Ok(ip);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        anyhow::bail!("Public IP not assigned to {instance_id} after 60s")
    }

    /// Build the argv for `aws ec2 run-instances` from a `VmConfig` plus
    /// already-resolved `ami_id`, `key_name`, and `sg_id`. Pure (no IO).
    ///
    /// When `user_data_path` is `Some(path)`, appends `--user-data file://<path>`
    /// so cloud-init runs on first boot.
    pub fn build_run_instances_args(
        config: &VmConfig,
        ami_id: &str,
        key_name: &str,
        sg_id: &str,
        user_data_path: Option<&std::path::Path>,
    ) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "ec2".into(),
            "run-instances".into(),
            "--image-id".into(),
            ami_id.to_string(),
            "--instance-type".into(),
            config.vm_size.clone(),
            "--region".into(),
            config.region.clone(),
            "--key-name".into(),
            key_name.to_string(),
            "--security-group-ids".into(),
            sg_id.to_string(),
            "--associate-public-ip-address".into(),
            "--tag-specifications".into(),
            format!(
                "ResourceType=instance,Tags=[{{Key=Name,Value={}}}]",
                config.name
            ),
            "--query".into(),
            "Instances[0]".into(),
            "--output".into(),
            "json".into(),
        ];
        if let Some(path) = user_data_path {
            args.push("--user-data".into());
            args.push(format!("file://{}", path.display()));
        }
        args
    }

    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo> {
        tracing::info!(
            region = %self.region,
            vm_size = %config.vm_size,
            vm_name = %config.name,
            "AwsProvider::create_vm"
        );

        // Resolve AMI by marker (image field is "aws:<os-variant>")
        let ami_id = self.resolve_ami(&config.image, &config.region).await?;
        tracing::info!(ami_id = %ami_id, image_marker = %config.image, "Resolved AMI");

        // Ensure key pair exists (uses local ~/.ssh/id_rsa.pub if available, else creates new)
        let key_name = self.ensure_key_pair(&config.region).await?;

        // Ensure security group exists with SSH + dashboard ports open
        let sg_id = self.ensure_security_group(&config.region).await?;

        // If a bootstrap script is requested, write it to a tempfile and pass
        // `--user-data file://<path>`. The NamedTempFile is kept alive for the
        // duration of the aws call, then dropped (auto-deleted) on return.
        let user_data_tmp = if let Some(script) = config.bootstrap_script.as_deref() {
            use std::io::Write;
            let mut tmp = tempfile::NamedTempFile::new()
                .context("failed to create tempfile for --user-data")?;
            tmp.write_all(script.as_bytes())
                .context("failed to write --user-data script to tempfile")?;
            tmp.flush().ok();
            Some(tmp)
        } else {
            None
        };

        let args = Self::build_run_instances_args(
            config,
            &ami_id,
            &key_name,
            &sg_id,
            user_data_tmp.as_ref().map(|t| t.path()),
        );

        let output = self
            .aws_cmd()
            .args(&args)
            .output()
            .await
            .context("failed to spawn aws ec2 run-instances")?;
        // Keep tempfile alive until after the aws invocation completed.
        drop(user_data_tmp);

        if !output.status.success() {
            anyhow::bail!(
                "aws ec2 run-instances failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let v: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("aws ec2 run-instances produced non-JSON output")?;

        let instance_id = v
            .get("InstanceId")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("missing InstanceId"))?
            .to_string();

        // Public IP isn't available immediately — poll for up to 60s
        let public_ip = self
            .wait_for_public_ip(&instance_id, &config.region)
            .await
            .unwrap_or_default();

        Ok(VmInfo {
            resource_id: instance_id,
            public_ip,
            vm_name: config.name.clone(),
            power_state: "running".to_string(),
        })
    }

    pub async fn start_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let output = self
            .aws_cmd()
            .arg("ec2")
            .arg("start-instances")
            .arg("--instance-ids")
            .arg(resource_id)
            .output()
            .await
            .context("failed to spawn aws ec2 start-instances")?;
        if !output.status.success() {
            anyhow::bail!(
                "aws ec2 start-instances failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let output = self
            .aws_cmd()
            .arg("ec2")
            .arg("stop-instances")
            .arg("--instance-ids")
            .arg(resource_id)
            .output()
            .await
            .context("failed to spawn aws ec2 stop-instances")?;
        if !output.status.success() {
            anyhow::bail!(
                "aws ec2 stop-instances failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    pub async fn delete_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let output = self
            .aws_cmd()
            .arg("ec2")
            .arg("terminate-instances")
            .arg("--instance-ids")
            .arg(resource_id)
            .output()
            .await
            .context("failed to spawn aws ec2 terminate-instances")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Idempotent: instance already gone is the desired end-state.
            if stderr.contains("InvalidInstanceID.NotFound") || stderr.contains("does not exist") {
                tracing::info!(
                    resource_id,
                    "AWS instance already terminated; treating as success"
                );
                return Ok(());
            }
            anyhow::bail!("aws ec2 terminate-instances failed: {stderr}");
        }
        Ok(())
    }

    pub async fn get_vm_state(&self, resource_id: &str) -> anyhow::Result<VmInfo> {
        let output = self
            .aws_cmd()
            .arg("ec2")
            .arg("describe-instances")
            .arg("--instance-ids")
            .arg(resource_id)
            .arg("--query")
            .arg("Reservations[0].Instances[0]")
            .arg("--output")
            .arg("json")
            .output()
            .await
            .context("failed to spawn aws ec2 describe-instances")?;

        if !output.status.success() {
            anyhow::bail!(
                "aws ec2 describe-instances failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let v: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("aws ec2 describe-instances non-JSON")?;

        let state = v
            .get("State")
            .and_then(|s| s.get("Name"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();
        let public_ip = v
            .get("PublicIpAddress")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let name = v
            .get("Tags")
            .and_then(|t| t.as_array())
            .and_then(|tags| {
                tags.iter()
                    .find(|t| t.get("Key").and_then(|k| k.as_str()) == Some("Name"))
                    .and_then(|t| t.get("Value").and_then(|v| v.as_str()))
            })
            .unwrap_or("")
            .to_string();

        Ok(VmInfo {
            resource_id: resource_id.to_string(),
            public_ip,
            vm_name: name,
            power_state: state,
        })
    }

    pub async fn tag_vm(
        &self,
        resource_id: &str,
        tags: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        if tags.is_empty() {
            return Ok(());
        }
        let tag_args: Vec<String> = tags
            .iter()
            .map(|(k, v)| format!("Key={k},Value={v}"))
            .collect();
        let mut cmd = self.aws_cmd();
        cmd.arg("ec2")
            .arg("create-tags")
            .arg("--resources")
            .arg(resource_id)
            .arg("--tags");
        for t in &tag_args {
            cmd.arg(t);
        }
        let output = cmd
            .output()
            .await
            .context("failed to spawn aws ec2 create-tags")?;
        if !output.status.success() {
            anyhow::bail!(
                "aws ec2 create-tags failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}

// ── GCP provider ───────────────────────────────────────────────────────────

/// GCP Compute Engine VM lifecycle backed by the `gcloud` CLI.
#[derive(Debug, Clone)]
pub struct GcpProvider {
    /// Service account JSON key (full file content)
    pub service_account_json: String,
    pub project_id: String,
    pub region: String,
}

impl GcpProvider {
    pub fn from_config(config: &serde_json::Value) -> anyhow::Result<Self> {
        let json_key = config
            .get("json_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("gcp config: missing json_key"))?
            .to_string();

        // Parse the JSON key to extract project_id
        let key_parsed: serde_json::Value =
            serde_json::from_str(&json_key).context("gcp config: json_key is not valid JSON")?;
        let project_id = key_parsed
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("gcp json_key: missing project_id"))?
            .to_string();

        let region = config
            .get("region")
            .and_then(|v| v.as_str())
            .unwrap_or("us-central1")
            .to_string();

        Ok(Self {
            service_account_json: json_key,
            project_id,
            region,
        })
    }

    /// Write the service account JSON to a temp file and return the path.
    /// Caller should delete the file after use.
    fn write_key_file(&self) -> anyhow::Result<String> {
        let path = format!("/tmp/gcp-key-{}.json", uuid::Uuid::new_v4().simple());
        std::fs::write(&path, &self.service_account_json)?;
        Ok(path)
    }

    fn gcloud_cmd(&self, key_file: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("gcloud");
        cmd.env("GOOGLE_APPLICATION_CREDENTIALS", key_file)
            .env("CLOUDSDK_CORE_PROJECT", &self.project_id);
        cmd
    }

    /// Build the argv for `gcloud compute instances create` from a `VmConfig`
    /// plus already-computed `zone`, optional `ssh_metadata_path` (for the
    /// `ssh-keys=` metadata file), and optional `startup_script_path`. Pure
    /// (no IO).
    ///
    /// When `startup_script_path` is `Some(path)`, appends
    /// `--metadata-from-file startup-script=<path>` so cloud-init-style
    /// bootstrap runs on first boot.
    pub fn build_create_args(
        config: &VmConfig,
        zone: &str,
        ssh_metadata_path: Option<&std::path::Path>,
        startup_script_path: Option<&std::path::Path>,
    ) -> Vec<String> {
        let image_project = match config.image.as_str() {
            s if s.starts_with("ubuntu") => "ubuntu-os-cloud",
            s if s.starts_with("debian") => "debian-cloud",
            s if s.starts_with("windows") => "windows-cloud",
            _ => "ubuntu-os-cloud",
        };

        let mut args: Vec<String> = vec![
            "compute".into(),
            "instances".into(),
            "create".into(),
            config.name.clone(),
            "--zone".into(),
            zone.to_string(),
            "--machine-type".into(),
            config.vm_size.clone(),
            "--image-family".into(),
            config.image.clone(),
            "--image-project".into(),
            image_project.to_string(),
            "--tags".into(),
            "alethedash-tester".into(),
            "--format".into(),
            "json".into(),
        ];

        if let Some(md) = ssh_metadata_path {
            args.push("--metadata-from-file".into());
            args.push(format!("ssh-keys={}", md.display()));
        }

        if let Some(path) = startup_script_path {
            args.push("--metadata-from-file".into());
            args.push(format!("startup-script={}", path.display()));
        }

        args
    }

    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo> {
        tracing::info!(
            project = %self.project_id,
            region = %self.region,
            vm_name = %config.name,
            vm_size = %config.vm_size,
            "GcpProvider::create_vm"
        );

        let key_file = self.write_key_file()?;

        // GCP needs a zone, not just a region. Use the first zone in the region.
        let zone = format!("{}-a", config.region);

        // Inject SSH public key so the dashboard can SSH in after creation.
        // GCP uses instance metadata `ssh-keys=user:pubkey` for this.
        // Read the dashboard's local ~/.ssh/id_rsa.pub.
        let home = std::env::var("HOME").unwrap_or_default();
        let pub_key_path = format!("{home}/.ssh/id_rsa.pub");
        let ssh_metadata_file = if std::path::Path::new(&pub_key_path).exists() {
            let pub_key = std::fs::read_to_string(&pub_key_path)?;
            let user = &config.ssh_user;
            let metadata_path = format!("/tmp/gcp-ssh-keys-{}.txt", uuid::Uuid::new_v4().simple());
            std::fs::write(&metadata_path, format!("{user}:{}", pub_key.trim()))?;
            Some(metadata_path)
        } else {
            tracing::warn!("No ~/.ssh/id_rsa.pub found — GCP SSH may not work");
            None
        };

        // If a bootstrap script is requested, write it to a tempfile and pass
        // `--metadata-from-file startup-script=<path>`. The NamedTempFile is
        // kept alive for the duration of the gcloud call, then dropped
        // (auto-deleted) on return.
        let startup_script_tmp = if let Some(script) = config.bootstrap_script.as_deref() {
            use std::io::Write;
            let mut tmp = tempfile::NamedTempFile::new()
                .context("failed to create tempfile for startup-script")?;
            tmp.write_all(script.as_bytes())
                .context("failed to write startup-script to tempfile")?;
            tmp.flush().ok();
            Some(tmp)
        } else {
            None
        };

        let ssh_metadata_path = ssh_metadata_file.as_ref().map(std::path::Path::new);
        let args = Self::build_create_args(
            config,
            &zone,
            ssh_metadata_path,
            startup_script_tmp.as_ref().map(|t| t.path()),
        );

        let output = self
            .gcloud_cmd(&key_file)
            .args(&args)
            .output()
            .await
            .context("failed to spawn gcloud compute instances create")?;
        // Keep tempfile alive until after the gcloud invocation completed.
        drop(startup_script_tmp);

        if let Some(md) = &ssh_metadata_file {
            let _ = std::fs::remove_file(md);
        }

        let _ = std::fs::remove_file(&key_file);

        if !output.status.success() {
            anyhow::bail!(
                "gcloud compute instances create failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let v: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("gcloud create produced non-JSON output")?;

        // GCP returns an array
        let inst = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
        let resource_id = inst
            .get("selfLink")
            .and_then(|s| s.as_str())
            .or_else(|| inst.get("id").and_then(|s| s.as_str()))
            .ok_or_else(|| anyhow!("missing instance id/selfLink"))?
            .to_string();

        let public_ip = inst
            .get("networkInterfaces")
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.first())
            .and_then(|n| n.get("accessConfigs"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("natIP"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();

        Ok(VmInfo {
            resource_id,
            public_ip,
            vm_name: config.name.clone(),
            power_state: "running".to_string(),
        })
    }

    pub async fn start_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let key_file = self.write_key_file()?;
        let (name, zone) = parse_gcp_resource_id(resource_id);
        let output = self
            .gcloud_cmd(&key_file)
            .arg("compute")
            .arg("instances")
            .arg("start")
            .arg(&name)
            .arg("--zone")
            .arg(&zone)
            .output()
            .await
            .context("failed to spawn gcloud start")?;
        let _ = std::fs::remove_file(&key_file);
        if !output.status.success() {
            anyhow::bail!(
                "gcloud start failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let key_file = self.write_key_file()?;
        let (name, zone) = parse_gcp_resource_id(resource_id);
        let output = self
            .gcloud_cmd(&key_file)
            .arg("compute")
            .arg("instances")
            .arg("stop")
            .arg(&name)
            .arg("--zone")
            .arg(&zone)
            .output()
            .await
            .context("failed to spawn gcloud stop")?;
        let _ = std::fs::remove_file(&key_file);
        if !output.status.success() {
            anyhow::bail!(
                "gcloud stop failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    pub async fn delete_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let key_file = self.write_key_file()?;
        let (name, zone) = parse_gcp_resource_id(resource_id);
        let output = self
            .gcloud_cmd(&key_file)
            .arg("compute")
            .arg("instances")
            .arg("delete")
            .arg(&name)
            .arg("--zone")
            .arg(&zone)
            .arg("--quiet")
            .output()
            .await
            .context("failed to spawn gcloud delete")?;
        let _ = std::fs::remove_file(&key_file);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Treat "already gone" as success — the desired end-state is the
            // same. Lets the dashboard reap orphan rows whose cloud resource
            // was removed out-of-band.
            if stderr.contains("was not found") || stderr.contains("404") {
                tracing::info!(%name, %zone, "GCP VM already deleted; treating as success");
                return Ok(());
            }
            anyhow::bail!("gcloud delete failed: {stderr}");
        }
        Ok(())
    }

    pub async fn get_vm_state(&self, resource_id: &str) -> anyhow::Result<VmInfo> {
        let key_file = self.write_key_file()?;
        let (name, zone) = parse_gcp_resource_id(resource_id);
        let output = self
            .gcloud_cmd(&key_file)
            .arg("compute")
            .arg("instances")
            .arg("describe")
            .arg(&name)
            .arg("--zone")
            .arg(&zone)
            .arg("--format")
            .arg("json")
            .output()
            .await
            .context("failed to spawn gcloud describe")?;
        let _ = std::fs::remove_file(&key_file);
        if !output.status.success() {
            anyhow::bail!(
                "gcloud describe failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let v: serde_json::Value =
            serde_json::from_slice(&output.stdout).context("gcloud describe non-JSON")?;

        let status = v
            .get("status")
            .and_then(|s| s.as_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "unknown".to_string());
        let public_ip = v
            .get("networkInterfaces")
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.first())
            .and_then(|n| n.get("accessConfigs"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("natIP"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();

        Ok(VmInfo {
            resource_id: resource_id.to_string(),
            public_ip,
            vm_name: name,
            power_state: status,
        })
    }

    pub async fn tag_vm(
        &self,
        _resource_id: &str,
        _tags: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        // GCP uses "labels" instead of tags. Implement if needed.
        Ok(())
    }
}

/// Parse a GCP resource ID (selfLink) into (name, zone).
/// Format: https://www.googleapis.com/compute/v1/projects/PROJECT/zones/ZONE/instances/NAME
fn parse_gcp_resource_id(resource_id: &str) -> (String, String) {
    let parts: Vec<&str> = resource_id.split('/').collect();
    let name = parts.last().unwrap_or(&"").to_string();
    let zone = parts
        .iter()
        .position(|&p| p == "zones")
        .and_then(|i| parts.get(i + 1))
        .map(|s| s.to_string())
        .unwrap_or_default();
    (name, zone)
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
    fn windows_computer_name_truncates_long_names() {
        assert_eq!(
            azure_windows_computer_name("bm-azure-windows-2022"),
            "bm-azure-window"
        );
        assert_eq!(
            azure_windows_computer_name("bm-azure-win11"),
            "bm-azure-win11"
        );
    }

    #[test]
    fn windows_computer_name_replaces_invalid_chars_and_trims_hyphens() {
        assert_eq!(
            azure_windows_computer_name("bm_azure.win 11"),
            "bm-azure-win-11"
        );
        assert_eq!(azure_windows_computer_name("bm-azure-"), "bm-azure");
        assert_eq!(
            azure_windows_computer_name("bm--azure---win"),
            "bm-azure-win"
        );
    }

    #[test]
    fn windows_computer_name_rejects_all_numeric() {
        assert_eq!(azure_windows_computer_name("2022"), "w2022");
        assert_eq!(
            azure_windows_computer_name("12345678901234567890"),
            "w12345678901234"
        );
    }

    fn aws_vm_config_fixture(bootstrap: Option<&str>) -> VmConfig {
        VmConfig {
            name: "bm-aws-test".to_string(),
            region: "us-east-1".to_string(),
            vm_size: "t3.small".to_string(),
            ssh_user: "ubuntu".to_string(),
            image: "aws:ubuntu-24.04-server".to_string(),
            tags: HashMap::new(),
            bootstrap_script: bootstrap.map(|s| s.to_string()),
        }
    }

    #[test]
    fn aws_create_vm_args_include_user_data_when_bootstrap_set() {
        use std::io::Write;

        let config = aws_vm_config_fixture(Some("#!/bin/bash\necho hi\n"));

        // Write the script to a real tempfile (matching the runtime path).
        let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile for bootstrap script");
        tmp.write_all(config.bootstrap_script.as_deref().unwrap().as_bytes())
            .expect("write bootstrap script");
        tmp.flush().ok();

        let args = AwsProvider::build_run_instances_args(
            &config,
            "ami-123456",
            "alethedash-tester",
            "sg-abcdef",
            Some(tmp.path()),
        );

        // --user-data must be present and followed by a file:// reference.
        let idx = args
            .iter()
            .position(|a| a == "--user-data")
            .expect("--user-data arg present");
        let val = args
            .get(idx + 1)
            .expect("value arg after --user-data")
            .clone();
        assert!(
            val.starts_with("file://"),
            "user-data value should be file:// reference, got {val}"
        );

        // The referenced file should contain the script body we wrote.
        let path = val.trim_start_matches("file://");
        let contents = std::fs::read_to_string(path).expect("read back tempfile");
        assert!(
            contents.contains("echo hi"),
            "tempfile should contain bootstrap script body, got: {contents}"
        );
    }

    #[test]
    fn aws_create_vm_args_omit_user_data_when_bootstrap_none() {
        let config = aws_vm_config_fixture(None);
        let args = AwsProvider::build_run_instances_args(
            &config,
            "ami-123456",
            "alethedash-tester",
            "sg-abcdef",
            None,
        );
        assert!(
            !args.iter().any(|a| a == "--user-data"),
            "no --user-data flag when bootstrap_script is None; got args = {args:?}"
        );
    }

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

        let err = CloudProvider::from_connection("digitalocean", &config).unwrap_err();
        assert!(
            err.to_string().contains("unsupported cloud provider"),
            "expected 'unsupported cloud provider', got: {err}"
        );

        let err = CloudProvider::from_connection("oracle", &config).unwrap_err();
        assert!(
            err.to_string().contains("unsupported cloud provider"),
            "expected 'unsupported cloud provider', got: {err}"
        );
    }

    #[test]
    fn aws_provider_from_valid_config() {
        let config = serde_json::json!({
            "access_key_id": "AKIA1234567890",
            "secret_access_key": "secret123",
            "region": "us-east-1"
        });
        let provider = AwsProvider::from_config(&config).unwrap();
        assert_eq!(provider.access_key_id, "AKIA1234567890");
        assert_eq!(provider.region, "us-east-1");
        assert!(provider.session_token.is_none());
    }

    #[test]
    fn aws_provider_with_session_token() {
        let config = serde_json::json!({
            "access_key_id": "ASIA123",
            "secret_access_key": "secret",
            "session_token": "token123",
            "region": "eu-west-1"
        });
        let provider = AwsProvider::from_config(&config).unwrap();
        assert_eq!(provider.session_token.as_deref(), Some("token123"));
    }

    #[test]
    fn aws_provider_rejects_missing_keys() {
        let config = serde_json::json!({"region": "us-east-1"});
        assert!(AwsProvider::from_config(&config).is_err());
    }

    #[test]
    fn gcp_provider_from_valid_config() {
        let json_key = serde_json::json!({
            "type": "service_account",
            "project_id": "my-project-123",
            "private_key": "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----",
            "client_email": "sa@my-project.iam.gserviceaccount.com"
        });
        let config = serde_json::json!({
            "json_key": json_key.to_string(),
            "region": "us-central1"
        });
        let provider = GcpProvider::from_config(&config).unwrap();
        assert_eq!(provider.project_id, "my-project-123");
        assert_eq!(provider.region, "us-central1");
    }

    #[test]
    fn gcp_provider_rejects_invalid_key() {
        let config = serde_json::json!({"json_key": "not json"});
        assert!(GcpProvider::from_config(&config).is_err());
    }

    fn gcp_vm_config_fixture(bootstrap: Option<&str>) -> VmConfig {
        VmConfig {
            name: "bm-gcp-test".to_string(),
            region: "us-central1".to_string(),
            vm_size: "e2-small".to_string(),
            ssh_user: "ubuntu".to_string(),
            image: "ubuntu-2404-lts".to_string(),
            tags: HashMap::new(),
            bootstrap_script: bootstrap.map(|s| s.to_string()),
        }
    }

    #[test]
    fn gcp_create_vm_args_include_startup_script_when_bootstrap_set() {
        use std::io::Write;

        let config = gcp_vm_config_fixture(Some("#!/bin/bash\necho hello-gcp\n"));

        // Write the script to a real tempfile (matching the runtime path).
        let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile for startup-script");
        tmp.write_all(config.bootstrap_script.as_deref().unwrap().as_bytes())
            .expect("write startup-script");
        tmp.flush().ok();

        let args = GcpProvider::build_create_args(&config, "us-central1-a", None, Some(tmp.path()));

        // Find a --metadata-from-file flag whose following arg is the
        // startup-script= reference.
        let mut found_startup = None;
        for (i, a) in args.iter().enumerate() {
            if a == "--metadata-from-file" {
                if let Some(next) = args.get(i + 1) {
                    if next.starts_with("startup-script=") {
                        found_startup = Some(next.clone());
                        break;
                    }
                }
            }
        }
        let val = found_startup.expect("startup-script metadata-from-file arg present");
        let path = val.trim_start_matches("startup-script=");
        let contents = std::fs::read_to_string(path).expect("read back tempfile");
        assert!(
            contents.contains("echo hello-gcp"),
            "tempfile should contain bootstrap script body, got: {contents}"
        );
    }

    #[test]
    fn gcp_create_vm_args_omit_startup_script_when_bootstrap_none() {
        let config = gcp_vm_config_fixture(None);
        let args = GcpProvider::build_create_args(&config, "us-central1-a", None, None);
        assert!(
            !args.iter().any(|a| a.starts_with("startup-script=")),
            "no startup-script metadata when bootstrap_script is None; got args = {args:?}"
        );
    }

    #[test]
    fn parse_gcp_resource_id_extracts_name_and_zone() {
        let id = "https://www.googleapis.com/compute/v1/projects/my-proj/zones/us-central1-a/instances/test-vm";
        let (name, zone) = parse_gcp_resource_id(id);
        assert_eq!(name, "test-vm");
        assert_eq!(zone, "us-central1-a");
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
