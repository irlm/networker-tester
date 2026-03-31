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

    // Fetch cells from DB (they have cell_id which the config_json might not)
    let db_cells = crate::db::benchmark_cells::list_for_config(&client, &config.config_id)
        .await
        .unwrap_or_default();

    // Build cells array with cell_id + data from config_json
    let inner = &config.config_json;
    let config_cells = inner.get("cells").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let merged_cells: Vec<serde_json::Value> = db_cells.iter().enumerate().map(|(i, db_cell)| {
        let mut cell = config_cells.get(i).cloned().unwrap_or(serde_json::json!({}));
        if let Some(obj) = cell.as_object_mut() {
            obj.insert("cell_id".to_string(), serde_json::json!(db_cell.cell_id.to_string()));
            // Ensure existing_vm_ip is present
            if !obj.contains_key("existing_vm_ip") {
                if let Some(ip) = &db_cell.endpoint_ip {
                    obj.insert("existing_vm_ip".to_string(), serde_json::json!(ip));
                }
            }
        }
        cell
    }).collect();

    // Write config JSON in the format the orchestrator's DashboardBenchmarkConfig expects
    let config_path = format!("/tmp/bench-{}.json", config.config_id);
    let config_data = serde_json::json!({
        "config_id": config.config_id.to_string(),
        "cells": merged_cells,
        "methodology": inner.get("methodology").cloned().unwrap_or(serde_json::json!({})),
        "auto_teardown": inner.get("auto_teardown").and_then(|v| v.as_bool()).unwrap_or(true),
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
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    match child_result {
        Ok(mut child) => {
            let config_id = config.config_id;
            let db_pool = state.db.clone();

            // Monitor the child process in a spawned task
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(status) => {
                        if status.success() {
                            tracing::info!(
                                config_id = %config_id,
                                "Benchmark orchestrator completed successfully"
                            );
                            // Status will be set by the callback complete handler
                        } else {
                            let stderr = child.stderr.take();
                            let err_msg = if let Some(mut stderr) = stderr {
                                let mut buf = String::new();
                                use tokio::io::AsyncReadExt;
                                let _ = stderr.read_to_string(&mut buf).await;
                                buf
                            } else {
                                format!("exit code {:?}", status.code())
                            };
                            tracing::error!(
                                config_id = %config_id,
                                exit_code = ?status.code(),
                                stderr = %err_msg,
                                "Benchmark orchestrator failed"
                            );
                            // Mark as failed in DB
                            if let Ok(db) = db_pool.get().await {
                                let _ = crate::db::benchmark_configs::update_status(
                                    &db, &config_id, "failed",
                                    Some(&format!("Orchestrator exited with code {:?}: {}", status.code(), err_msg.chars().take(500).collect::<String>())),
                                ).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            config_id = %config_id,
                            error = %e,
                            "Failed to wait for benchmark orchestrator"
                        );
                        if let Ok(db) = db_pool.get().await {
                            let _ = crate::db::benchmark_configs::update_status(
                                &db, &config_id, "failed",
                                Some(&format!("Process wait error: {e}")),
                            ).await;
                        }
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
