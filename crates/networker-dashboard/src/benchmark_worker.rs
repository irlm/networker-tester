//! Background worker that polls for queued benchmark configs and spawns the
//! `alethabench` orchestrator process. Runs as a tokio task alongside the
//! dashboard API server.

use std::sync::Arc;

use crate::AppState;

/// Spawn the benchmark worker background task.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Wait for server startup
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        tracing::info!("Benchmark worker background task started");

        let worker_id = format!(
            "worker-{}",
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".into())
        );

        let mut last_cleanup = std::time::Instant::now();

        loop {
            // Poll every 5 seconds
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            if let Err(e) = poll_and_run(&state, &worker_id).await {
                tracing::error!(error = %e, "Benchmark worker poll failed");
            }

            // Cleanup stalled configs every 15 minutes
            if last_cleanup.elapsed() > std::time::Duration::from_secs(900) {
                last_cleanup = std::time::Instant::now();
                if let Err(e) = cleanup_stalled(&state).await {
                    tracing::error!(error = %e, "Benchmark worker cleanup failed");
                }
            }
        }
    });
}

/// Poll for a queued benchmark config, claim it, and spawn the orchestrator.
async fn poll_and_run(state: &AppState, worker_id: &str) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let config = crate::db::benchmark_configs::claim_queued(&client, worker_id).await?;

    let config = match config {
        Some(c) => c,
        None => return Ok(()), // Nothing queued
    };

    tracing::info!(
        config_id = %config.config_id,
        name = %config.name,
        worker_id = %worker_id,
        "Claimed benchmark config for execution"
    );

    // Write config JSON to temp file
    let config_path = format!("/tmp/bench-{}.json", config.config_id);
    let config_data = serde_json::json!({
        "config_id": config.config_id,
        "project_id": config.project_id,
        "name": config.name,
        "config": config.config_json,
        "max_duration_secs": config.max_duration_secs,
    });
    tokio::fs::write(&config_path, serde_json::to_string_pretty(&config_data)?)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write config file: {e}"))?;

    // Generate a scoped callback JWT for the orchestrator
    let callback_token = crate::auth::create_token(
        config.config_id, // Use config_id as the subject for scoped token
        &format!("benchmark-{}", config.config_id),
        "system",
        false,
        &state.jwt_secret,
    )?;

    // Construct callback URL
    let callback_url = format!("{}/api", state.public_url);

    // Spawn alethabench as a child process
    let child_result = tokio::process::Command::new("alethabench")
        .args([
            "run",
            "--config",
            &config_path,
            "--callback-url",
            &callback_url,
            "--callback-token",
            &callback_token,
            "--stream-logs",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    match child_result {
        Ok(mut child) => {
            let config_id = config.config_id;

            // Monitor the child process in a spawned task
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(status) => {
                        if status.success() {
                            tracing::info!(
                                config_id = %config_id,
                                "Benchmark orchestrator completed successfully"
                            );
                        } else {
                            tracing::warn!(
                                config_id = %config_id,
                                exit_code = ?status.code(),
                                "Benchmark orchestrator exited with non-zero status"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            config_id = %config_id,
                            error = %e,
                            "Failed to wait for benchmark orchestrator"
                        );
                    }
                }

                // Clean up temp file
                let _ = tokio::fs::remove_file(&format!("/tmp/bench-{config_id}.json")).await;
            });
        }
        Err(e) => {
            tracing::error!(
                config_id = %config.config_id,
                error = %e,
                "Failed to spawn alethabench process"
            );

            // Mark config as failed
            let client = state.db.get().await?;
            crate::db::benchmark_configs::update_status(
                &client,
                &config.config_id,
                "failed",
                Some(&format!("Failed to spawn orchestrator: {e}")),
            )
            .await?;

            // Clean up temp file
            let _ = tokio::fs::remove_file(&config_path).await;
        }
    }

    Ok(())
}

/// Find stalled configs (no heartbeat for 10 minutes) and mark them as failed.
async fn cleanup_stalled(state: &AppState) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let stalled = crate::db::benchmark_configs::find_stalled(&client, 10).await?;

    for config in &stalled {
        tracing::warn!(
            config_id = %config.config_id,
            name = %config.name,
            last_heartbeat = ?config.last_heartbeat,
            "Marking stalled benchmark config as failed"
        );

        crate::db::benchmark_configs::update_status(
            &client,
            &config.config_id,
            "failed",
            Some("No heartbeat received for 10 minutes — orchestrator presumed dead"),
        )
        .await?;
    }

    if !stalled.is_empty() {
        tracing::info!(
            count = stalled.len(),
            "Cleaned up stalled benchmark configs"
        );
    }

    Ok(())
}
