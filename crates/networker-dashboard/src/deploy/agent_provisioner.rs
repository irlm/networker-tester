//! Agent provisioning: spawn local agents or deploy agents to remote machines via SSH.

use tokio::process::Command;

/// Spawn a local tester process. Returns the PID if successful.
pub async fn spawn_local_agent(api_key: &str, dashboard_url: &str) -> Option<u32> {
    tracing::info!(dashboard_url, "Spawning local tester");

    let agent_bin = find_agent_binary().await;

    // Log tester output to a file for debugging
    let log_path = std::env::temp_dir().join("networker-tester-agent.log");
    let log_file = std::fs::File::create(&log_path).ok();
    let stderr_out = match &log_file {
        Some(f) => std::process::Stdio::from(
            f.try_clone()
                .unwrap_or_else(|_| std::fs::File::create("/dev/null").expect("/dev/null")),
        ),
        None => std::process::Stdio::null(),
    };
    if log_file.is_some() {
        tracing::info!(path = %log_path.display(), "Tester output logging to file");
    }

    let result = match &agent_bin {
        Some(bin) => {
            tracing::info!(binary = %bin, "Starting local tester process");
            Command::new(bin)
                .env("AGENT_API_KEY", api_key)
                .env("AGENT_DASHBOARD_URL", dashboard_url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(stderr_out)
                .spawn()
        }
        None => {
            tracing::info!("Tester binary not found, trying cargo run");
            Command::new("cargo")
                .args(["run", "-p", "networker-agent", "--release"])
                .env("AGENT_API_KEY", api_key)
                .env("AGENT_DASHBOARD_URL", dashboard_url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        }
    };

    match result {
        Ok(child) => {
            let pid = child.id().unwrap_or(0);
            tracing::info!(pid, "Local tester process spawned");
            Some(pid)
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to spawn local tester");
            None
        }
    }
}

/// Deploy an agent to a remote machine via SSH.
///
/// Steps:
/// 1. Download the agent binary on the remote machine (from GitHub Releases)
/// 2. Start it with the API key and dashboard URL
pub async fn provision_remote_agent(
    name: &str,
    api_key: &str,
    dashboard_url_template: &str,
    ssh_host: &str,
    ssh_user: &str,
    ssh_port: u16,
    events_tx: tokio::sync::broadcast::Sender<networker_common::messages::DashboardEvent>,
) {
    tracing::info!(
        name,
        ssh_host,
        ssh_user,
        ssh_port,
        "Provisioning remote agent via SSH"
    );

    // Determine the dashboard URL the remote agent should use.
    // Replace {DASHBOARD_HOST} with the SSH client's IP as seen from the remote.
    // For now, use the local machine's hostname.
    let local_host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".to_string());
    let dashboard_url = dashboard_url_template.replace("{DASHBOARD_HOST}", &local_host);

    // Determine platform for binary download
    let platform_cmd = "uname -s -m 2>/dev/null | tr ' ' '-' | tr '[:upper:]' '[:lower:]'";

    let ssh_dest = if ssh_user.is_empty() {
        ssh_host.to_string()
    } else {
        format!("{ssh_user}@{ssh_host}")
    };

    let ssh_args = vec![
        "-o".to_string(),
        "StrictHostKeyChecking=no".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout=10"),
        "-p".to_string(),
        ssh_port.to_string(),
    ];

    // Step 1: Detect platform
    let platform = match run_ssh(&ssh_dest, &ssh_args, platform_cmd).await {
        Ok(p) => {
            let p = p.trim().to_string();
            tracing::info!(platform = %p, "Remote platform detected");
            p
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to detect remote platform via SSH");
            return;
        }
    };

    // Map platform to release asset name
    let asset_name = match platform.as_str() {
        "linux-x86_64" => "networker-agent-x86_64-unknown-linux-gnu.tar.gz",
        "linux-aarch64" => "networker-agent-aarch64-unknown-linux-gnu.tar.gz",
        "darwin-arm64" => "networker-agent-aarch64-apple-darwin.tar.gz",
        "darwin-x86_64" => "networker-agent-x86_64-apple-darwin.tar.gz",
        _ => {
            tracing::error!(platform = %platform, "Unsupported remote platform for tester");
            return;
        }
    };

    // Validate inputs to prevent command injection via SSH
    let safe_chars = |s: &str| {
        s.chars()
            .all(|c| c.is_alphanumeric() || "-_:/.".contains(c))
    };
    if !safe_chars(api_key) || !safe_chars(&dashboard_url) {
        tracing::error!(
            "Refusing to provision: api_key or dashboard_url contains unsafe characters"
        );
        return;
    }

    // Step 2: Download and install agent binary
    let install_script = format!(
        r#"
set -e
mkdir -p ~/.networker
cd ~/.networker

# Get latest release tag
LATEST=$(curl -fsSL https://api.github.com/repos/irlm/networker-tester/releases/latest | grep '"tag_name"' | head -1 | cut -d'"' -f4)
if [ -z "$LATEST" ]; then
    echo "ERROR: Could not determine latest release"
    exit 1
fi

echo "Downloading agent $LATEST ({asset_name})..."
curl -fsSL "https://github.com/irlm/networker-tester/releases/download/$LATEST/{asset_name}" -o agent.tar.gz
tar xzf agent.tar.gz
rm -f agent.tar.gz
chmod +x networker-agent 2>/dev/null || true

# Kill any existing agent
pkill -f 'networker-agent' 2>/dev/null || true
sleep 1

# Start agent in background
export AGENT_API_KEY="{api_key}"
export AGENT_DASHBOARD_URL="{dashboard_url}"
nohup ./networker-agent > agent.log 2>&1 &
echo "Tester started (PID: $!)"
"#,
        asset_name = asset_name,
        api_key = api_key,
        dashboard_url = dashboard_url,
    );

    let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
        deployment_id: uuid::Uuid::nil(), // Use nil for agent provisioning logs
        line: format!("Provisioning agent '{name}' on {ssh_host}..."),
        stream: "stdout".into(),
    });

    match run_ssh(&ssh_dest, &ssh_args, &install_script).await {
        Ok(output) => {
            tracing::info!(name, ssh_host, output = %output.trim(), "Remote agent provisioned");
            let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
                deployment_id: uuid::Uuid::nil(),
                line: format!("Tester '{name}' provisioned on {ssh_host}"),
                stream: "stdout".into(),
            });
        }
        Err(e) => {
            tracing::error!(name, ssh_host, error = %e, "Failed to provision remote tester");
        }
    }
}

/// Run a command on a remote machine via SSH.
async fn run_ssh(dest: &str, extra_args: &[String], command: &str) -> anyhow::Result<String> {
    let mut cmd = Command::new("ssh");
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg(dest).arg(command);
    cmd.stdin(std::process::Stdio::null());

    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SSH command failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Find the networker-tester binary (public, used by version check).
pub async fn find_tester_binary_path() -> Option<String> {
    find_binary("networker-tester").await
}

/// Find the agent binary in common locations.
async fn find_agent_binary() -> Option<String> {
    find_binary("networker-agent").await
}

async fn find_binary(name: &str) -> Option<String> {
    for path in &[
        format!("target/debug/{name}"),
        format!("target/release/{name}"),
    ] {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(path.to_string());
        }
    }

    // Try workspace root
    if let Ok(cwd) = std::env::current_dir() {
        for sub in &[
            format!("target/debug/{name}"),
            format!("target/release/{name}"),
        ] {
            let p = cwd.join(sub);
            if tokio::fs::metadata(&p).await.is_ok() {
                return Some(p.to_string_lossy().to_string());
            }
        }
        let mut dir = cwd.as_path();
        for _ in 0..5 {
            if let Some(parent) = dir.parent() {
                for sub in &[
                    format!("target/debug/{name}"),
                    format!("target/release/{name}"),
                ] {
                    let p = parent.join(sub);
                    if tokio::fs::metadata(&p).await.is_ok() {
                        return Some(p.to_string_lossy().to_string());
                    }
                }
                dir = parent;
            }
        }
    }

    // Try which
    if let Ok(output) = Command::new("which").arg(name).output().await {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}
