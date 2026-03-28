use crate::provisioner::VmInfo;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

const SSH_CONNECT_TIMEOUT: &str = "10";
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_POLL_MAX_WAIT: Duration = Duration::from_secs(60);

/// Execute a command on the remote VM via SSH.
async fn ssh_exec(ip: &str, cmd: &str) -> Result<String> {
    let output = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o",
            "BatchMode=yes",
            &format!("azureuser@{ip}"),
            cmd,
        ])
        .output()
        .await
        .context("failed to execute ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SSH command failed on {ip}: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Copy a local file to the remote VM via SCP.
async fn scp_to(ip: &str, local: &str, remote: &str) -> Result<()> {
    let output = tokio::process::Command::new("scp")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o",
            "BatchMode=yes",
            local,
            &format!("azureuser@{ip}:{remote}"),
        ])
        .output()
        .await
        .context("failed to execute scp")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SCP to {ip}:{remote} failed: {}", stderr.trim());
    }
    Ok(())
}

/// Deploy a reference API to the target VM.
///
/// Steps:
/// 1. Create /opt/bench/ directory on the VM
/// 2. Copy shared TLS cert + key
/// 3. Run the language-specific deploy.sh script
/// 4. Deploy metrics-agent binary and start it on :9100
/// 5. Wait for /health endpoint to respond (max 60s)
pub async fn deploy_api(vm: &VmInfo, language: &str, bench_dir: &Path) -> Result<()> {
    tracing::info!("Deploying {} API to {} ({})", language, vm.name, vm.ip);

    // 1. Create target directory
    ssh_exec(
        &vm.ip,
        "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench",
    )
    .await
    .context("creating /opt/bench on VM")?;

    // 2. Copy shared TLS certs
    let shared_dir = bench_dir.join("shared");
    let cert_path = shared_dir.join("cert.pem");
    let key_path = shared_dir.join("key.pem");

    if cert_path.exists() && key_path.exists() {
        scp_to(&vm.ip, cert_path.to_str().unwrap(), "/opt/bench/cert.pem")
            .await
            .context("copying cert.pem")?;
        scp_to(&vm.ip, key_path.to_str().unwrap(), "/opt/bench/key.pem")
            .await
            .context("copying key.pem")?;
        tracing::debug!("TLS certs copied to VM");
    } else {
        tracing::warn!(
            "Shared certs not found at {}, generating on VM",
            shared_dir.display()
        );
        let gen_script = shared_dir.join("generate-cert.sh");
        if gen_script.exists() {
            scp_to(
                &vm.ip,
                gen_script.to_str().unwrap(),
                "/opt/bench/generate-cert.sh",
            )
            .await?;
            ssh_exec(&vm.ip, "bash /opt/bench/generate-cert.sh").await?;
        }
    }

    // 3. Run language-specific deploy.sh
    let deploy_script = if language == "rust" {
        // Rust uses the top-level rust-deploy.sh (symlink to networker-endpoint)
        bench_dir.join("reference-apis/rust-deploy.sh")
    } else {
        bench_dir.join(format!("reference-apis/{language}/deploy.sh"))
    };

    if !deploy_script.exists() {
        bail!("deploy script not found: {}", deploy_script.display());
    }

    tracing::info!("Running deploy.sh for {language}");
    let status = tokio::process::Command::new("bash")
        .arg(deploy_script.to_str().unwrap())
        .arg(&vm.ip)
        .current_dir(bench_dir)
        .status()
        .await
        .context("running deploy.sh")?;

    if !status.success() {
        bail!("deploy.sh for {language} failed with exit code {status}");
    }

    // 4. Deploy metrics-agent
    deploy_metrics_agent(vm, bench_dir).await?;

    // 5. Wait for /health endpoint
    wait_for_health(vm).await?;

    tracing::info!("{language} API deployed and healthy on {}", vm.name);
    Ok(())
}

/// Deploy and start the metrics-agent on the VM.
async fn deploy_metrics_agent(vm: &VmInfo, bench_dir: &Path) -> Result<()> {
    // Try to find a pre-built metrics-agent binary
    let agent_binary = bench_dir.join("metrics-agent/target/release/metrics-agent");
    if !agent_binary.exists() {
        tracing::warn!(
            "metrics-agent binary not found at {}, skipping deployment",
            agent_binary.display()
        );
        return Ok(());
    }

    tracing::info!("Deploying metrics-agent to {}", vm.name);
    scp_to(
        &vm.ip,
        agent_binary.to_str().unwrap(),
        "/opt/bench/metrics-agent",
    )
    .await
    .context("copying metrics-agent binary")?;

    ssh_exec(
        &vm.ip,
        "pkill -f /opt/bench/metrics-agent || true; \
         chmod +x /opt/bench/metrics-agent; \
         nohup /opt/bench/metrics-agent > /opt/bench/metrics-agent.log 2>&1 &",
    )
    .await
    .context("starting metrics-agent")?;

    // Brief wait for the agent to bind its port
    tokio::time::sleep(Duration::from_secs(1)).await;
    tracing::debug!("metrics-agent started on {}:9100", vm.ip);
    Ok(())
}

/// Poll the /health endpoint until it responds or timeout.
async fn wait_for_health(vm: &VmInfo) -> Result<()> {
    tracing::info!("Waiting for /health on {}:8443...", vm.ip);
    let deadline = tokio::time::Instant::now() + HEALTH_POLL_MAX_WAIT;

    loop {
        if tokio::time::Instant::now() > deadline {
            bail!(
                "Timed out waiting for /health on {}:8443 after {}s",
                vm.ip,
                HEALTH_POLL_MAX_WAIT.as_secs()
            );
        }

        let result = tokio::process::Command::new("curl")
            .args([
                "-sk",
                "--connect-timeout",
                "5",
                "--max-time",
                "10",
                &format!("https://{}:8443/health", vm.ip),
            ])
            .output()
            .await;

        if let Ok(output) = result {
            if output.status.success() {
                let body = String::from_utf8_lossy(&output.stdout);
                if body.contains("ok") || body.contains("healthy") {
                    tracing::info!("/health responded OK on {}", vm.ip);
                    return Ok(());
                }
            }
        }

        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

/// Validate that the deployed API is functioning correctly.
pub async fn validate_api(vm: &VmInfo) -> Result<()> {
    tracing::info!("Validating API on {} ({})", vm.name, vm.ip);

    // 1. GET /health -- verify status:ok
    let health_output = curl_get(vm, "/health").await.context("GET /health")?;
    let health: serde_json::Value =
        serde_json::from_str(&health_output).context("parsing /health response as JSON")?;
    if health.get("status").and_then(|v| v.as_str()) != Some("ok") {
        bail!(
            "/health did not return status:ok, got: {}",
            health_output.trim()
        );
    }
    tracing::debug!("/health OK");

    // 2. GET /download/1024 -- verify exactly 1024 bytes
    let download_output = tokio::process::Command::new("curl")
        .args([
            "-sk",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "-o",
            "/dev/null",
            "-w",
            "%{size_download}",
            &format!("https://{}:8443/download/1024", vm.ip),
        ])
        .output()
        .await
        .context("curl /download/1024")?;

    let size_str = String::from_utf8_lossy(&download_output.stdout);
    let size: u64 = size_str
        .trim()
        .parse()
        .with_context(|| format!("parsing download size: '{}'", size_str.trim()))?;
    if size != 1024 {
        bail!("/download/1024 returned {size} bytes, expected 1024");
    }
    tracing::debug!("/download/1024 OK (1024 bytes)");

    // 3. POST /upload with 1024 bytes -- verify bytes_received
    let upload_body_data = "X".repeat(1024);
    let upload_output = tokio::process::Command::new("curl")
        .args([
            "-sk",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/octet-stream",
            "--data-binary",
            &upload_body_data,
            &format!("https://{}:8443/upload", vm.ip),
        ])
        .output()
        .await
        .context("curl POST /upload")?;

    let upload_body = String::from_utf8_lossy(&upload_output.stdout);
    let upload_json: serde_json::Value = serde_json::from_str(&upload_body)
        .with_context(|| format!("parsing /upload response: '{}'", upload_body.trim()))?;
    let received = upload_json
        .get("bytes_received")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if received != 1024 {
        bail!("/upload reported {received} bytes_received, expected 1024");
    }
    tracing::debug!("/upload OK (1024 bytes received)");

    tracing::info!("API validation passed on {}", vm.name);
    Ok(())
}

/// Helper: curl GET an endpoint and return the body as a string.
async fn curl_get(vm: &VmInfo, path: &str) -> Result<String> {
    let output = tokio::process::Command::new("curl")
        .args([
            "-sk",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            &format!("https://{}:8443{}", vm.ip, path),
        ])
        .output()
        .await
        .context("curl GET")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("curl GET {path} failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Stop the running API server on the VM.
pub async fn stop_api(vm: &VmInfo) -> Result<()> {
    tracing::info!("Stopping API on {} ({})", vm.name, vm.ip);
    // Kill common server process names
    ssh_exec(
        &vm.ip,
        "pkill -f '/opt/bench/.*server' || true; \
         pkill -f 'networker-endpoint' || true; \
         pkill -f '/opt/bench/metrics-agent' || true",
    )
    .await
    .context("stopping server processes")?;

    tracing::info!("API stopped on {}", vm.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provisioner::VmInfo;

    fn test_vm() -> VmInfo {
        VmInfo {
            name: "test-vm".into(),
            ip: "127.0.0.1".into(),
            cloud: "azure".into(),
            os: "ubuntu".into(),
            vm_size: "Standard_D2s_v3".into(),
            resource_group: "alethabench-rg".into(),
        }
    }

    #[test]
    fn test_deploy_script_path_resolution() {
        let bench_dir = Path::new("/tmp/bench");
        let go_path = bench_dir.join("reference-apis/go/deploy.sh");
        assert_eq!(
            go_path.to_str().unwrap(),
            "/tmp/bench/reference-apis/go/deploy.sh"
        );
        let rust_path = bench_dir.join("reference-apis/rust-deploy.sh");
        assert_eq!(
            rust_path.to_str().unwrap(),
            "/tmp/bench/reference-apis/rust-deploy.sh"
        );
    }

    #[test]
    fn test_curl_url_formatting() {
        let vm = test_vm();
        let url = format!("https://{}:8443/health", vm.ip);
        assert_eq!(url, "https://127.0.0.1:8443/health");
    }
}
