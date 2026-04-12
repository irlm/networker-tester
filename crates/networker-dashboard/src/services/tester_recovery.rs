//! Crash recovery — force-releases stuck locks and handles testers left in
//! transient power states after a dashboard restart. Runs once 5 minutes
//! after startup.

#![allow(dead_code)] // wired in Task 34

use std::sync::Arc;
use std::time::Duration;
use tokio_postgres::Client;
use uuid::Uuid;

use crate::services::{cloud_provider, tester_dispatcher, tester_state};

const STARTUP_GRACE: Duration = Duration::from_secs(5 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);
const STUCK_THRESHOLD_MINUTES: i64 = 30;

/// Periodic crash-recovery loop. Waits `STARTUP_GRACE` after dashboard
/// boot, then scans every `SWEEP_INTERVAL` forever. Named
/// `recover_on_startup` for backwards compatibility with `main.rs`,
/// which spawns it in its own (non-supervised) task because this
/// function manages its own pacing.
///
/// RR-017: previously this was a one-shot scan — any lock leak that
/// developed during normal operation would never heal until the next
/// dashboard restart. Making it periodic means the dashboard self-heals
/// on its own cadence.
pub async fn recover_on_startup(client: Arc<Client>) {
    tokio::time::sleep(STARTUP_GRACE).await;
    let mut interval = tokio::time::interval(SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // First tick fires immediately after `interval` is created, giving
    // us the post-grace initial scan we want.
    loop {
        interval.tick().await;
        match scan(&client).await {
            Ok((locks, stucks)) => tracing::info!(
                locks_released = locks,
                transients_handled = stucks,
                "tester crash recovery scan complete"
            ),
            Err(e) => tracing::warn!(error = ?e, "tester crash recovery scan failed"),
        }
    }
}

async fn scan(client: &Client) -> anyhow::Result<(usize, usize)> {
    let locks = force_release_stuck_locks(client).await?;
    let stucks = handle_stuck_transients(client).await?;
    Ok((locks, stucks))
}

async fn force_release_stuck_locks(client: &Client) -> anyhow::Result<usize> {
    // TODO(RR-002 followup): consider reclaiming locks where the config hasn't
    // been updated in >2h, even if its status isn't terminal, as a belt-and-
    // suspenders for orchestrator hard-crashes. Requires distinguishing
    // "in-progress and healthy" from "stuck" — out of scope for this PR.
    // Identify testers whose lock holder is in a terminal state.
    let rows = client
        .query(
            r#"
            SELECT t.tester_id, t.project_id, t.name, c.status
              FROM project_tester t
              JOIN benchmark_config c ON c.config_id = t.locked_by_config_id
             WHERE t.allocation = 'locked'
               AND c.status IN ('completed','completed_with_errors','failed','cancelled')
            "#,
            &[],
        )
        .await?;

    let mut count = 0;
    for row in rows {
        let tester_id: Uuid = row.get(0);
        let project_id: String = row.get(1);
        let name: String = row.get(2);
        let terminal_status: String = row.get(3);

        match tester_state::force_release(client, &tester_id).await {
            Ok(()) => {
                tracing::info!(
                    target: "crash_recovery_lock_released",
                    %tester_id,
                    %project_id,
                    %name,
                    prior_holder_status = %terminal_status,
                    "force-released stuck lock"
                );
                count += 1;
                // Kick the dispatcher so any queued benchmark picks up the freed tester.
                if let Err(e) = tester_dispatcher::promote_next(client, &tester_id).await {
                    tracing::warn!(%tester_id, error = ?e, "promote_next after force_release failed");
                }
            }
            Err(e) => tracing::warn!(%tester_id, error = ?e, "force_release failed"),
        }
    }
    Ok(count)
}

async fn handle_stuck_transients(client: &Client) -> anyhow::Result<usize> {
    let rows = client
        .query(
            r#"
            SELECT tester_id, project_id, name, power_state, auto_probe_enabled,
                   vm_name, vm_resource_id
              FROM project_tester
             WHERE power_state IN ('starting','stopping','upgrading','provisioning')
               AND updated_at < NOW() - INTERVAL '30 minutes'
            "#,
            &[],
        )
        .await?;

    let mut count = 0;
    for row in rows {
        let tester_id: Uuid = row.get(0);
        let project_id: String = row.get(1);
        let name: String = row.get(2);
        let power: String = row.get(3);
        let auto_probe: bool = row.get(4);
        let vm_name: Option<String> = row.get(5);
        let vm_resource_id: Option<String> = row.get(6);

        if auto_probe {
            match probe_azure_state(&vm_resource_id, &vm_name).await {
                Ok(azure_state) => {
                    let new_state = azure_power_to_row(&azure_state);
                    client
                        .execute(
                            "UPDATE project_tester SET power_state = $2, status_message = $3, updated_at = NOW() WHERE tester_id = $1",
                            &[&tester_id, &new_state, &format!("Auto-probed after restart: Azure reported {}", azure_state)],
                        )
                        .await?;
                    tracing::info!(
                        target: "crash_recovery_auto_probed",
                        %tester_id,
                        %project_id,
                        %name,
                        previous = %power,
                        azure_state = %azure_state,
                        resolved = %new_state,
                        "stuck transient auto-probed"
                    );
                }
                Err(e) => {
                    client
                        .execute(
                            "UPDATE project_tester SET power_state = 'error', status_message = $2, updated_at = NOW() WHERE tester_id = $1",
                            &[&tester_id, &format!("Auto-probe failed after restart: {e}")],
                        )
                        .await?;
                    tracing::warn!(
                        target: "crash_recovery_auto_probed",
                        %tester_id,
                        error = ?e,
                        "auto-probe failed; marked error"
                    );
                }
            }
        } else {
            let msg = format!(
                "Stuck in {} after dashboard restart — needs manual recovery (auto-probe disabled)",
                power
            );
            client
                .execute(
                    "UPDATE project_tester SET power_state = 'error', status_message = $2, updated_at = NOW() WHERE tester_id = $1",
                    &[&tester_id, &msg],
                )
                .await?;
            tracing::warn!(
                target: "crash_recovery_marked_error",
                %tester_id,
                %project_id,
                %name,
                previous = %power,
                "stuck transient marked error (auto-probe disabled)"
            );
        }
        count += 1;
    }
    Ok(count)
}

/// Probe Azure for the current power state of a VM. Returns the raw
/// `powerState` string (e.g. "VM running", "VM deallocated"). Used by
/// crash recovery and the `POST /testers/{tid}/probe` REST endpoint.
pub async fn probe_azure_state(
    resource_id: &Option<String>,
    vm_name: &Option<String>,
) -> anyhow::Result<String> {
    let id = resource_id
        .as_deref()
        .or(vm_name.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no vm_resource_id or vm_name to probe"))?;
    let provider = cloud_provider::legacy_azure_provider()?;
    let info = provider.get_vm_state(id).await?;
    Ok(info.power_state)
}

/// Map an Azure `displayStatus` string onto a `project_tester.power_state`
/// value. Public so REST handlers can reuse it.
pub fn azure_power_to_row(azure_state: &str) -> String {
    // Azure states: "VM running", "VM stopped", "VM deallocated", "VM starting", "VM stopping"
    let lower = azure_state.to_ascii_lowercase();
    if lower.contains("running") {
        "running".to_string()
    } else if lower.contains("deallocated") || lower.contains("stopped") {
        "stopped".to_string()
    } else if lower.contains("starting") {
        "starting".to_string()
    } else if lower.contains("stopping") || lower.contains("deallocating") {
        "stopping".to_string()
    } else {
        "error".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_power_to_row_maps_known_states() {
        assert_eq!(azure_power_to_row("VM running"), "running");
        assert_eq!(azure_power_to_row("VM deallocated"), "stopped");
        assert_eq!(azure_power_to_row("VM stopped"), "stopped");
        assert_eq!(azure_power_to_row("VM starting"), "starting");
        assert_eq!(azure_power_to_row("VM stopping"), "stopping");
        assert_eq!(azure_power_to_row("PowerState/unknown"), "error");
    }

    #[test]
    fn constants_sane() {
        assert_eq!(STARTUP_GRACE, Duration::from_secs(300));
        assert_eq!(SWEEP_INTERVAL, Duration::from_secs(600));
        assert_eq!(STUCK_THRESHOLD_MINUTES, 30);
    }
}
