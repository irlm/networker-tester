use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use std::sync::Arc;

use crate::AppState;

async fn update_local_tester(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    tracing::info!("Starting local tester update");

    // Find current tester binary location
    let bin_path = crate::deploy::agent_provisioner::find_tester_binary_path()
        .await
        .ok_or_else(|| {
            tracing::error!("Cannot find local tester binary");
            StatusCode::NOT_FOUND
        })?;

    let events_tx = state.events_tx.clone();
    let update_id = uuid::Uuid::new_v4();

    // Send initial log
    let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
        deployment_id: update_id,
        line: format!("Updating local tester at {bin_path}..."),
        stream: "stdout".into(),
    });

    tokio::spawn(async move {
        match do_update_tester(&bin_path, &events_tx, update_id).await {
            Ok(version) => {
                let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
                    deployment_id: update_id,
                    line: format!("Update complete: v{version}"),
                    stream: "stdout".into(),
                });
                let _ =
                    events_tx.send(networker_common::messages::DashboardEvent::DeployComplete {
                        deployment_id: update_id,
                        status: "completed".into(),
                        endpoint_ips: vec![],
                    });
                tracing::info!(version, "Local tester updated");
            }
            Err(e) => {
                let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
                    deployment_id: update_id,
                    line: format!("Update failed: {e}"),
                    stream: "stderr".into(),
                });
                let _ =
                    events_tx.send(networker_common::messages::DashboardEvent::DeployComplete {
                        deployment_id: update_id,
                        status: "failed".into(),
                        endpoint_ips: vec![],
                    });
                tracing::error!(error = %e, "Local tester update failed");
            }
        }
    });

    Ok(Json(serde_json::json!({
        "status": "updating",
        "update_id": update_id.to_string(),
    })))
}

async fn do_update_tester(
    bin_path: &str,
    events_tx: &tokio::sync::broadcast::Sender<networker_common::messages::DashboardEvent>,
    update_id: uuid::Uuid,
) -> anyhow::Result<String> {
    // Determine platform
    let target = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-apple-darwin"
        }
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    } else {
        anyhow::bail!("Unsupported platform for auto-update");
    };

    let asset_name = format!("networker-tester-{target}.tar.gz");

    let log = |msg: &str| {
        let _ = events_tx.send(networker_common::messages::DashboardEvent::DeployLog {
            deployment_id: update_id,
            line: msg.to_string(),
            stream: "stdout".into(),
        });
    };

    // Get latest release info
    log("Fetching latest release from GitHub...");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let release: serde_json::Value = client
        .get("https://api.github.com/repos/irlm/networker-tester/releases/latest")
        .header("User-Agent", "networker-dashboard")
        .send()
        .await?
        .json()
        .await?;

    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No tag_name in release"))?;
    log(&format!("Latest release: {tag}"));

    // Download the asset
    let download_url =
        format!("https://github.com/irlm/networker-tester/releases/download/{tag}/{asset_name}");
    log(&format!("Downloading {asset_name}..."));

    let tmp_dir = std::env::temp_dir().join("networker-update");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let tar_path = tmp_dir.join(&asset_name);

    let resp = client.get(&download_url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Download failed: HTTP {}", resp.status());
    }
    let bytes = resp.bytes().await?;
    tokio::fs::write(&tar_path, &bytes).await?;
    log(&format!("Downloaded {} bytes", bytes.len()));

    // Extract
    log("Extracting...");
    let extract_dir = tmp_dir.join("extract");
    tokio::fs::create_dir_all(&extract_dir).await?;

    let output = tokio::process::Command::new("tar")
        .args(["xzf", tar_path.to_str().unwrap()])
        .current_dir(&extract_dir)
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!(
            "tar extract failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Find the extracted binary
    let new_bin = extract_dir.join("networker-tester");
    if tokio::fs::metadata(&new_bin).await.is_err() {
        anyhow::bail!("networker-tester binary not found in archive");
    }

    // Replace old binary
    log(&format!("Installing to {bin_path}..."));
    tokio::fs::copy(&new_bin, bin_path).await?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(bin_path, perms)?;
    }

    // Verify version
    let verify = tokio::process::Command::new(bin_path)
        .arg("--version")
        .output()
        .await?;
    let ver_output = String::from_utf8_lossy(&verify.stdout);
    let installed_version = ver_output
        .split_whitespace()
        .last()
        .unwrap_or("unknown")
        .trim();
    log(&format!("Installed: networker-tester {installed_version}"));

    // Cleanup
    tokio::fs::remove_dir_all(&tmp_dir).await.ok();

    // Restart the local tester subprocess
    log("Restarting local tester...");

    // Kill existing tester processes
    #[cfg(unix)]
    {
        let pgrep = tokio::process::Command::new("pgrep")
            .args(["-f", "networker-agent"])
            .output()
            .await;
        if let Ok(output) = pgrep {
            if output.status.success() {
                let pids = String::from_utf8_lossy(&output.stdout);
                for pid_str in pids.trim().lines() {
                    if let Ok(pid) = pid_str.trim().parse::<i32>() {
                        if pid != std::process::id() as i32 {
                            unsafe { libc::kill(pid, libc::SIGTERM) };
                        }
                    }
                }
            }
        }
    }

    // The dashboard's monitor loop will auto-respawn the tester
    log("Tester will be respawned automatically");

    Ok(installed_version.to_string())
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/update/tester", post(update_local_tester))
        .with_state(state)
}
