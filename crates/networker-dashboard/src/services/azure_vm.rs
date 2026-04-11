//! Thin wrapper around the `az` CLI for the persistent-tester lifecycle.
//!
//! This module shells out to the locally-installed Azure CLI, matching the
//! pattern established by `tester_scheduler::vm_deallocate`. It intentionally
//! does NOT do any full ARM-SDK integration — Task 36 will revisit this at
//! the integration-test level.
//!
//! All functions return `anyhow::Result` so callers (background tasks) can
//! log the failure and mark the tester's `power_state = 'error'`.

#![allow(dead_code)] // wired in by Task 15 handlers

use anyhow::{anyhow, Context};

/// Default Azure resource group. Overridable via `DASHBOARD_AZURE_RG`.
const DEFAULT_RG: &str = "networker-testers";

/// Default admin username baked into the generated Ubuntu image.
const DEFAULT_ADMIN: &str = "azureuser";

/// Outcome of a successful `az vm create`.
#[derive(Debug, Clone)]
pub struct CreatedVm {
    pub vm_name: String,
    pub resource_group: String,
    pub resource_id: String,
    pub public_ip: String,
    pub admin_username: String,
}

/// Resolve the resource group from the environment (or fall back).
pub fn resource_group() -> String {
    std::env::var("DASHBOARD_AZURE_RG").unwrap_or_else(|_| DEFAULT_RG.to_string())
}

/// Generate a short, DNS-safe VM name `tester-{region}-{5 lowercase hex}`.
/// Kept under 15 chars on Windows is not a concern here (Linux VM), but we
/// still keep the suffix short to stay well within Azure's 64-char limit.
pub fn generate_vm_name(region: &str) -> String {
    let suffix: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(5)
        .collect();
    // Sanitize region: Azure region names are already lowercase-alphanumeric.
    format!("tester-{region}-{suffix}")
}

/// Create a new Azure VM and return the essential identifiers.
///
/// This uses `--generate-ssh-keys` so the calling dashboard host's SSH
/// identity is authorised on the new box. The dashboard operator is
/// responsible for ensuring `~/.ssh/id_rsa.pub` exists (or that the Azure
/// CLI is logged in with rights to create/manage keys).
pub async fn az_vm_create(vm_name: &str, region: &str, vm_size: &str) -> anyhow::Result<CreatedVm> {
    let rg = resource_group();

    let output = tokio::process::Command::new("az")
        .arg("vm")
        .arg("create")
        .arg("--resource-group")
        .arg(&rg)
        .arg("--name")
        .arg(vm_name)
        .arg("--location")
        .arg(region)
        .arg("--image")
        .arg("Ubuntu2204")
        .arg("--size")
        .arg(vm_size)
        .arg("--public-ip-sku")
        .arg("Standard")
        .arg("--admin-username")
        .arg(DEFAULT_ADMIN)
        .arg("--generate-ssh-keys")
        .arg("--output")
        .arg("json")
        .output()
        .await
        .context("failed to spawn `az vm create`")?;

    if !output.status.success() {
        anyhow::bail!(
            "az vm create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("az vm create produced non-JSON output")?;

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

    Ok(CreatedVm {
        vm_name: vm_name.to_string(),
        resource_group: rg,
        resource_id,
        public_ip,
        admin_username: DEFAULT_ADMIN.to_string(),
    })
}

/// Start a stopped (deallocated) VM. Blocks until Azure accepts the request.
pub async fn az_vm_start(resource_id: &str) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("az")
        .arg("vm")
        .arg("start")
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

/// Deallocate a running VM (stop-billing). Mirrors `tester_scheduler`.
pub async fn az_vm_deallocate(resource_id: &str) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("az")
        .arg("vm")
        .arg("deallocate")
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

/// Permanently delete a VM (and its managed disks / NIC). `--yes` skips
/// the CLI's interactive confirmation prompt.
pub async fn az_vm_delete(resource_id: &str) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("az")
        .arg("vm")
        .arg("delete")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_vm_name_contains_region() {
        let name = generate_vm_name("eastus");
        assert!(name.starts_with("tester-eastus-"));
        assert!(name.len() > "tester-eastus-".len());
        assert!(name.len() < 64);
    }

    #[test]
    fn generate_vm_name_is_unique_ish() {
        let a = generate_vm_name("westus2");
        let b = generate_vm_name("westus2");
        assert_ne!(a, b, "two generated names collided");
    }

    #[test]
    fn resource_group_env_override() {
        // Happy path: unset => default; set => echoed back. We can't fully
        // isolate env in a parallel test, so just check the default holds
        // when nothing unusual is set.
        if std::env::var("DASHBOARD_AZURE_RG").is_err() {
            assert_eq!(resource_group(), DEFAULT_RG);
        }
    }
}
