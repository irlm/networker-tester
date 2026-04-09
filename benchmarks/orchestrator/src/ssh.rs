use anyhow::{bail, Context, Result};
use std::time::Duration;

const SSH_CONNECT_TIMEOUT: &str = "15";
const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(1200); // 20 min — Chrome install on fresh VMs needs apt-get update + 50+ deps
const SSH_CONTROL_DIR: &str = "/tmp/ssh-bench-ctl";

/// Get SSH args that enable ControlMaster multiplexing.
/// All SSH/SCP connections to the same host reuse one TCP connection.
fn ssh_control_args(ip: &str) -> Vec<String> {
    let _ = std::fs::create_dir_all(SSH_CONTROL_DIR);
    let socket = format!("{SSH_CONTROL_DIR}/ssh-{ip}");
    let key_path = if std::path::Path::new("/root/.ssh/id_ed25519").exists() {
        "/root/.ssh/id_ed25519".to_string()
    } else {
        "/root/.ssh/id_rsa".to_string()
    };
    vec![
        "-i".to_string(),
        key_path,
        "-o".to_string(),
        "StrictHostKeyChecking=no".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-o".to_string(),
        format!("ControlPath={socket}"),
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        "ControlPersist=120".to_string(),
    ]
}

/// Execute a command on a remote VM via SSH with timeout, keepalive, and ControlMaster.
pub async fn ssh_exec(ip: &str, cmd: &str) -> Result<String> {
    tracing::debug!(target_ip = ip, command = cmd, "SSH exec");
    let mut args = ssh_control_args(ip);
    args.push(format!("azureuser@{ip}"));
    args.push(cmd.to_string());
    let fut = tokio::process::Command::new("ssh").args(&args).output();

    let output = tokio::time::timeout(SSH_COMMAND_TIMEOUT, fut)
        .await
        .context("SSH command timed out (5min limit)")?
        .context("failed to execute ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            if stdout.trim().is_empty() {
                format!("exit code {:?}, no output", output.status.code())
            } else {
                format!(
                    "exit code {:?}, stdout: {}",
                    output.status.code(),
                    stdout.trim().chars().take(500).collect::<String>()
                )
            }
        } else {
            stderr.trim().to_string()
        };
        bail!("SSH command failed on {ip}: {detail}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Copy a local file to a remote VM via SCP with timeout and ControlMaster.
pub async fn scp_to(ip: &str, local: &str, remote: &str) -> Result<()> {
    tracing::debug!(
        target_ip = ip,
        local_path = local,
        remote_path = remote,
        "SCP upload"
    );
    let mut args = ssh_control_args(ip);
    args.push(local.to_string());
    args.push(format!("azureuser@{ip}:{remote}"));
    let fut = tokio::process::Command::new("scp").args(&args).output();

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
    tracing::debug!(
        target_ip = ip,
        local_dir = local_dir,
        remote_dir = remote_dir,
        "SCP -r upload"
    );
    let fut = tokio::process::Command::new("scp")
        .args([
            "-r",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "BatchMode=no",
            "-o",
            "PasswordAuthentication=no",
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
