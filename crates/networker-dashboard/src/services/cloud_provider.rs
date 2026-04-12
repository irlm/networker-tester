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
            "azure" => Ok(CloudProvider::Azure(AzureProvider::from_config(conn_config)?)),
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

        Ok(Self {
            subscription_id,
            resource_group,
            identity_type,
        })
    }

    /// Create a new Azure VM via `az vm create`.
    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo> {
        let mut cmd = tokio::process::Command::new("az");
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

        if !output.status.success() {
            anyhow::bail!(
                "az vm create failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let v: serde_json::Value = serde_json::from_slice(&output.stdout)
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

    /// Deallocate (stop-billing) a running VM.
    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()> {
        let output = tokio::process::Command::new("az")
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
        let output = tokio::process::Command::new("az")
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
        let output = tokio::process::Command::new("az")
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

        let mut cmd = tokio::process::Command::new("az");
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
    let rg = std::env::var("DASHBOARD_AZURE_RG")
        .unwrap_or_else(|_| "networker-testers".to_string());
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
        assert_eq!(provider.subscription_id, "11111111-2222-3333-4444-555555555555");
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
