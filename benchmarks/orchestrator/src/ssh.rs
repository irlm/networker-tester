use anyhow::{bail, Context, Result};
use std::time::Duration;

const SSH_CONNECT_TIMEOUT: &str = "10";
const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Execute a command on a remote VM via SSH with timeout and keepalive.
pub async fn ssh_exec(ip: &str, cmd: &str) -> Result<String> {
    tracing::debug!(target_ip = ip, command = cmd, "SSH exec");
    let fut = tokio::process::Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3",
            "-o", "BatchMode=yes",
            &format!("azureuser@{ip}"),
            cmd,
        ])
        .output();

    let output = tokio::time::timeout(SSH_COMMAND_TIMEOUT, fut)
        .await
        .context("SSH command timed out (5min limit)")?
        .context("failed to execute ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SSH command failed on {ip}: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Copy a local file to a remote VM via SCP with timeout.
pub async fn scp_to(ip: &str, local: &str, remote: &str) -> Result<()> {
    tracing::debug!(target_ip = ip, local_path = local, remote_path = remote, "SCP upload");
    let fut = tokio::process::Command::new("scp")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3",
            "-o", "BatchMode=yes",
            local,
            &format!("azureuser@{ip}:{remote}"),
        ])
        .output();

    let output = tokio::time::timeout(SSH_COMMAND_TIMEOUT, fut)
        .await
        .context("SCP timed out (5min limit)")?
        .context("failed to execute scp")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SCP to {ip}:{remote} failed: {}", stderr.trim());
    }
    Ok(())
}

/// Recursive SCP copy of a directory with timeout.
pub async fn scp_dir_to(ip: &str, local_dir: &str, remote_dir: &str) -> Result<()> {
    tracing::debug!(target_ip = ip, local_dir = local_dir, remote_dir = remote_dir, "SCP -r upload");
    let fut = tokio::process::Command::new("scp")
        .args([
            "-r",
            "-o", "StrictHostKeyChecking=no",
            "-o", &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3",
            "-o", "BatchMode=yes",
        ])
        .arg(format!("{local_dir}/."))
        .arg(format!("azureuser@{ip}:{remote_dir}/"))
        .output();

    let output = tokio::time::timeout(SSH_COMMAND_TIMEOUT, fut)
        .await
        .context("SCP -r timed out (5min limit)")?
        .context("failed to execute scp -r")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SCP -r to {ip}:{remote_dir} failed: {}", stderr.trim());
    }
    Ok(())
}

/// Validate IP address to prevent shell injection.
pub fn validate_ip(ip: &str) -> Result<()> {
    ip.parse::<std::net::IpAddr>()
        .context("vm.ip is not a valid IP address")?;
    Ok(())
}
