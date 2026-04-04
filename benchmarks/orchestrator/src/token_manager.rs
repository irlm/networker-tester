//! Per-test-run API token management via Azure Key Vault.
//!
//! Flow:
//! 1. Generate random token (64 hex chars)
//! 2. Store in Key Vault with expiry (audit trail + central management)
//! 3. SCP token file to each VM (/opt/bench/.api-token, chmod 600)
//! 4. Language server reads file at startup, deletes it from disk
//! 5. After test run, delete from Key Vault
//!
//! Key Vault is optional — if not configured, tokens are still generated
//! and deployed via SCP but without the central audit trail.

use anyhow::{Context, Result};
use rand::Rng;

use crate::ssh;

/// Name of the Key Vault (set via BENCH_KEYVAULT_NAME env var).
fn keyvault_name() -> Option<String> {
    std::env::var("BENCH_KEYVAULT_NAME")
        .ok()
        .filter(|v| !v.is_empty())
}

/// Generate a cryptographically random 64-character hex token.
pub fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Store a per-VM token in Azure Key Vault with a 4-hour expiry.
/// Each VM gets its own token — compromising one VM doesn't expose others.
/// Secret name: bench-{config_id}-vm-{testbed_id}
/// Returns Ok(()) if Key Vault is not configured (graceful skip).
pub async fn store_in_keyvault(
    config_id: &str,
    testbed_id: &str,
    token: &str,
) -> Result<()> {
    let vault_name = match keyvault_name() {
        Some(name) => name,
        None => {
            tracing::debug!("BENCH_KEYVAULT_NAME not set, skipping Key Vault storage");
            return Ok(());
        }
    };

    let secret_name = format!(
        "bench-{}-vm-{}",
        &config_id[..config_id.len().min(12)],
        &testbed_id[..testbed_id.len().min(12)]
    );
    let expiry = chrono::Utc::now() + chrono::Duration::hours(4);
    let expiry_str = expiry.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    tracing::info!(
        "Storing token in Key Vault {}/{} (expires {})",
        vault_name,
        secret_name,
        expiry_str
    );

    // Uses az CLI with explicit args (no shell interpolation)
    let output = tokio::process::Command::new("az")
        .args([
            "keyvault",
            "secret",
            "set",
            "--vault-name",
            &vault_name,
            "--name",
            &secret_name,
            "--value",
            token,
            "--expires",
            &expiry_str,
            "--output",
            "none",
        ])
        .output()
        .await
        .context("Failed to run az keyvault secret set")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            "Key Vault store failed (non-fatal): {}",
            stderr.chars().take(200).collect::<String>()
        );
    }

    Ok(())
}

/// Deploy token to a VM via SCP (secure file transfer).
/// Writes to /opt/bench/.api-token with mode 600.
pub async fn deploy_to_vm(ip: &str, token: &str) -> Result<()> {
    tracing::info!("Deploying API token to VM {}", ip);

    // Write token to a local temp file
    let tmp_path = format!("/tmp/bench-token-{}.tmp", &token[..8]);
    tokio::fs::write(&tmp_path, token)
        .await
        .context("Failed to write temp token file")?;

    // SCP to VM
    ssh::scp_to(ip, &tmp_path, "/tmp/.bench-api-token")
        .await
        .context("SCP token to VM failed")?;

    // Move to final location with correct permissions
    ssh::ssh_exec(
        ip,
        "sudo mv /tmp/.bench-api-token /opt/bench/.api-token && sudo chmod 600 /opt/bench/.api-token && sudo chown root:root /opt/bench/.api-token",
    )
    .await
    .context("Failed to set token file permissions")?;

    // Clean up local temp file
    let _ = tokio::fs::remove_file(&tmp_path).await;

    tracing::info!("API token deployed to VM {} at /opt/bench/.api-token", ip);
    Ok(())
}

/// Delete a single VM's token from Azure Key Vault.
pub async fn cleanup_keyvault_vm(config_id: &str, testbed_id: &str) -> Result<()> {
    let vault_name = match keyvault_name() {
        Some(name) => name,
        None => return Ok(()),
    };

    let secret_name = format!(
        "bench-{}-vm-{}",
        &config_id[..config_id.len().min(12)],
        &testbed_id[..testbed_id.len().min(12)]
    );

    tracing::info!("Revoking VM token: {}/{}", vault_name, secret_name);

    let output = tokio::process::Command::new("az")
        .args([
            "keyvault",
            "secret",
            "delete",
            "--vault-name",
            &vault_name,
            "--name",
            &secret_name,
            "--output",
            "none",
        ])
        .output()
        .await
        .context("Failed to run az keyvault secret delete")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("Key Vault cleanup failed (non-fatal): {}", stderr.chars().take(200).collect::<String>());
    }

    Ok(())
}

/// Revoke ALL tokens for a config run (emergency kill switch).
/// Deletes all secrets matching bench-{config_id}-vm-*.
pub async fn revoke_all_tokens(config_id: &str) -> Result<()> {
    let vault_name = match keyvault_name() {
        Some(name) => name,
        None => return Ok(()),
    };

    let prefix = format!("bench-{}", &config_id[..config_id.len().min(12)]);

    tracing::warn!("REVOKING ALL tokens for config {} from Key Vault {}", config_id, vault_name);

    // List all secrets matching the prefix
    let output = tokio::process::Command::new("az")
        .args([
            "keyvault",
            "secret",
            "list",
            "--vault-name",
            &vault_name,
            "--query",
            &format!("[?starts_with(name, '{}')].name", prefix),
            "--output",
            "tsv",
        ])
        .output()
        .await
        .context("Failed to list Key Vault secrets")?;

    if output.status.success() {
        let names = String::from_utf8_lossy(&output.stdout);
        for name in names.lines().filter(|l| !l.is_empty()) {
            tracing::info!("Revoking: {}/{}", vault_name, name);
            let _ = tokio::process::Command::new("az")
                .args([
                    "keyvault",
                    "secret",
                    "delete",
                    "--vault-name",
                    &vault_name,
                    "--name",
                    name,
                    "--output",
                    "none",
                ])
                .output()
                .await;
        }
    }

    Ok(())
}

/// Delete the token file from a VM.
pub async fn cleanup_vm(ip: &str) {
    let _ = ssh::ssh_exec(ip, "sudo rm -f /opt/bench/.api-token").await;
}
