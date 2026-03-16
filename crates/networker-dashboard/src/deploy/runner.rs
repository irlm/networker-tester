//! Deployment runner: shells out to install.sh --deploy and streams output.

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;
use uuid::Uuid;

use networker_common::messages::DashboardEvent;

/// Spawn install.sh --deploy with a generated deploy.json and stream output.
///
/// Returns the collected log and any IPs parsed from the output.
pub async fn run_deployment(
    deployment_id: Uuid,
    deploy_json: &serde_json::Value,
    events_tx: broadcast::Sender<DashboardEvent>,
    db_pool: Arc<deadpool_postgres::Pool>,
) -> anyhow::Result<Vec<String>> {
    let deploy_dir = std::env::temp_dir();
    let deploy_file = deploy_dir.join(format!("deploy-{deployment_id}.json"));

    // Write deploy.json to temp file
    let json_str = serde_json::to_string_pretty(deploy_json)?;
    tokio::fs::write(&deploy_file, &json_str).await?;

    tracing::info!(
        deployment_id = %deployment_id,
        deploy_file = %deploy_file.display(),
        "Starting install.sh --deploy"
    );

    // Find install.sh relative to the workspace root
    let install_sh = find_install_sh().await?;

    // Read only stdout — install.sh prints all user-facing output there.
    // Discard stderr (az/aws/gcloud CLI noise that causes duplicates).
    let mut child = Command::new("bash")
        .arg(&install_sh)
        .arg("--deploy")
        .arg(&deploy_file)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    // Update status to running
    if let Ok(client) = db_pool.get().await {
        crate::db::deployments::update_status(&client, &deployment_id, "running")
            .await
            .ok();
    }

    let _ = events_tx.send(DashboardEvent::DeployLog {
        deployment_id,
        line: "Deployment started...".into(),
        stream: "stdout".into(),
    });

    let stdout = child.stdout.take().expect("stdout piped");
    let mut reader = BufReader::new(stdout).lines();

    let mut output = DeployOutput::new();

    let bare_ip = regex::Regex::new(r"\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\b").unwrap();

    // Single stream (stdout + stderr merged via 2>&1)
    while let Ok(Some(text)) = reader.next_line().await {
        output.process_line(&text, &events_tx, deployment_id, "stdout");
    }

    // Wait for process to exit
    let exit_status = child.wait().await?;

    // If regex didn't catch IPs, try scanning for bare IPs near "endpoint" lines
    if output.endpoint_ips.is_empty() {
        for line in output.full_log.lines() {
            let lower = line.to_lowercase();
            if lower.contains("endpoint")
                || lower.contains("deployed")
                || lower.contains("public ip")
            {
                for caps in bare_ip.captures_iter(line) {
                    if let Some(ip) = caps.get(1) {
                        let ip_str = ip.as_str().to_string();
                        if !ip_str.starts_with("127.")
                            && !ip_str.starts_with("0.")
                            && !output.endpoint_ips.contains(&ip_str)
                        {
                            output.endpoint_ips.push(ip_str);
                        }
                    }
                }
            }
        }
    }

    // Save log and update status in DB
    if let Ok(client) = db_pool.get().await {
        // Save full log (replace, not append)
        client
            .execute(
                "UPDATE deployment SET log = $1 WHERE deployment_id = $2",
                &[&output.full_log, &deployment_id],
            )
            .await
            .ok();

        if exit_status.success() {
            // Save endpoint IPs
            let ips_json = serde_json::to_value(&output.endpoint_ips).unwrap_or_default();
            crate::db::deployments::set_endpoint_ips(&client, &deployment_id, &ips_json)
                .await
                .ok();
            crate::db::deployments::update_status(&client, &deployment_id, "completed")
                .await
                .ok();
        } else {
            let code = exit_status.code().unwrap_or(-1);
            crate::db::deployments::set_error(
                &client,
                &deployment_id,
                &format!("install.sh exited with code {code}"),
            )
            .await
            .ok();
        }
    }

    let status = if exit_status.success() {
        "completed"
    } else {
        "failed"
    };

    let _ = events_tx.send(DashboardEvent::DeployComplete {
        deployment_id,
        status: status.into(),
        endpoint_ips: output.endpoint_ips.clone(),
    });

    tracing::info!(
        deployment_id = %deployment_id,
        status,
        ips = ?output.endpoint_ips,
        "Deployment finished"
    );

    // Clean up temp file
    tokio::fs::remove_file(&deploy_file).await.ok();

    if exit_status.success() {
        Ok(output.endpoint_ips)
    } else {
        anyhow::bail!(
            "install.sh exited with code {}",
            exit_status.code().unwrap_or(-1)
        )
    }
}

/// Accumulates deployment output state.
struct DeployOutput {
    full_log: String,
    /// Endpoint addresses: FQDNs preferred over bare IPs.
    endpoint_ips: Vec<String>,
    seen_lines: std::collections::HashSet<String>,
    ip_re: regex::Regex,
    /// Matches FQDN with IP in parens: "hostname.eastus.cloudapp.azure.com (20.127.36.61)"
    fqdn_re: regex::Regex,
}

impl DeployOutput {
    fn new() -> Self {
        Self {
            full_log: String::new(),
            endpoint_ips: Vec::new(),
            seen_lines: std::collections::HashSet::new(),
            ip_re: regex::Regex::new(
                r"(?i)(?:endpoint[_ ](?:ip|address)|deployed[_ ](?:to|at)|public[_ ]ip)[:\s]+(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})"
            ).unwrap(),
            // Capture FQDN from lines like:
            //   "networker-endpoint-vm.eastus.cloudapp.azure.com (20.127.36.61)"
            //   or similar cloud DNS patterns
            // Match full FQDN: any sequence of labels ending in a known cloud domain, followed by (IP)
            // Examples:
            //   networker-endpoint-vm.eastus.cloudapp.azure.com (20.85.209.213)
            //   ec2-3-235-187-180.compute-1.amazonaws.com (3.235.187.180)
            //   nt-east-win.eastus.cloudapp.azure.com (10.0.0.5)
            fqdn_re: regex::Regex::new(
                r"([-a-z0-9]+(?:\.[-a-z0-9]+)*\.(?:cloudapp\.azure\.com|amazonaws\.com|compute\.googleapis\.com))\s+\((\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\)"
            ).unwrap(),
        }
    }

    /// Process a single output line: deduplicate, collect for log, check for FQDNs/IPs, broadcast.
    fn process_line(
        &mut self,
        text: &str,
        events_tx: &broadcast::Sender<DashboardEvent>,
        deployment_id: Uuid,
        stream: &str,
    ) {
        // Always add to full log
        self.full_log.push_str(text);
        self.full_log.push('\n');

        // Prefer FQDN over bare IP when available
        if let Some(caps) = self.fqdn_re.captures(text) {
            if let Some(fqdn) = caps.get(1) {
                let fqdn_str = fqdn.as_str().to_string();
                // Also get the IP to replace it if already captured
                let ip = caps.get(2).map(|m| m.as_str().to_string());
                if let Some(ref ip_str) = ip {
                    // Replace bare IP with FQDN if we already captured it
                    if let Some(pos) = self.endpoint_ips.iter().position(|x| x == ip_str) {
                        self.endpoint_ips[pos] = fqdn_str.clone();
                    }
                }
                if !self.endpoint_ips.contains(&fqdn_str) {
                    self.endpoint_ips.push(fqdn_str);
                }
            }
        } else if let Some(caps) = self.ip_re.captures(text) {
            // Fallback: capture bare IP
            if let Some(ip) = caps.get(1) {
                let ip_str = ip.as_str().to_string();
                if !self.endpoint_ips.contains(&ip_str) {
                    self.endpoint_ips.push(ip_str);
                }
            }
        }

        // Only broadcast if we haven't seen this exact line yet (dedup stdout/stderr)
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() && self.seen_lines.insert(trimmed) {
            let _ = events_tx.send(DashboardEvent::DeployLog {
                deployment_id,
                line: text.to_string(),
                stream: stream.into(),
            });
        }
    }
}

/// Locate install.sh — try workspace root (two levels up from crate), then current dir.
async fn find_install_sh() -> anyhow::Result<std::path::PathBuf> {
    // Try relative to CARGO_MANIFEST_DIR if set
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = std::path::PathBuf::from(manifest_dir).join("../../install.sh");
        if tokio::fs::metadata(&p).await.is_ok() {
            return Ok(p.canonicalize().unwrap_or(p));
        }
    }

    // Try current working directory
    let cwd = std::env::current_dir()?;
    let p = cwd.join("install.sh");
    if tokio::fs::metadata(&p).await.is_ok() {
        return Ok(p);
    }

    // Try parent directories
    let mut dir = cwd.as_path();
    for _ in 0..5 {
        if let Some(parent) = dir.parent() {
            let p = parent.join("install.sh");
            if tokio::fs::metadata(&p).await.is_ok() {
                return Ok(p);
            }
            dir = parent;
        }
    }

    // Check INSTALL_SH_PATH env var as override
    if let Ok(path) = std::env::var("INSTALL_SH_PATH") {
        let p = std::path::PathBuf::from(path);
        if tokio::fs::metadata(&p).await.is_ok() {
            return Ok(p);
        }
    }

    anyhow::bail!("Cannot find install.sh. Set INSTALL_SH_PATH environment variable to the path.")
}
