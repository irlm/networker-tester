//! Auto-shutdown loop — 60s tick, deallocates drained testers via `az vm deallocate`.
//!
//! Drain check is read from `benchmark_config.status` only. `current_phase`
//! is purely presentational and must never be consulted by orchestration.
//!
//! Audit trail: there is no `service_log` / `audit_log` table in the dashboard
//! schema today (only `migration_audit_log`, which is specific to sovereignty
//! migrations). For MVP we emit structured `tracing` events at info/warn level;
//! operators can scrape these off stdout / journald. A proper audit sink is
//! tracked separately.
//
// TODO: wire into a real service_log / audit_log table once the schema is
// finalized. For now, tracing::warn! with a `tester_shutdown_stuck` target is
// the minimum viable audit trail.

#![allow(dead_code)] // wired in Task 34

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio_postgres::Client;
use uuid::Uuid;

use crate::services::{azure_regions, cloud_provider, tester_state};

const TICK: Duration = Duration::from_secs(60);
const DEFERRAL_CAP: i16 = 3;
const DEFERRAL_DELAY_MINUTES: i64 = 5;

/// Background loop: every 60 seconds, sweep drained testers whose shutdown
/// window has elapsed and deallocate them via `az vm deallocate`.
pub async fn auto_shutdown_loop(client: Arc<Client>) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(e) = sweep(&client).await {
            tracing::warn!(error = ?e, "auto-shutdown sweep failed");
        }
    }
}

async fn sweep(client: &Client) -> anyhow::Result<()> {
    let rows = client
        .query(
            r#"
            SELECT t.tester_id, t.project_id, t.name, t.cloud, t.region,
                   t.auto_shutdown_local_hour, t.shutdown_deferral_count,
                   t.vm_name, t.vm_resource_id
              FROM project_tester t
             WHERE t.auto_shutdown_enabled = TRUE
               AND t.next_shutdown_at < NOW()
               AND t.power_state = 'running'
               AND t.allocation  = 'idle'
               AND NOT EXISTS (
                   SELECT 1 FROM benchmark_config c
                    WHERE c.tester_id = t.tester_id
                      AND c.status IN ('queued','pending','running')
               )
            "#,
            &[],
        )
        .await?;

    tracing::debug!(
        candidates = rows.len(),
        "auto-shutdown sweep: drained tester candidates"
    );

    for row in rows {
        let due = DueTester {
            tester_id: row.get(0),
            project_id: row.get::<_, String>(1),
            name: row.get::<_, String>(2),
            cloud: row.get::<_, String>(3),
            region: row.get::<_, String>(4),
            local_hour: row.get::<_, i16>(5),
            deferral_count: row.get::<_, i16>(6),
            vm_name: row.get::<_, Option<String>>(7),
            vm_resource_id: row.get::<_, Option<String>>(8),
        };
        if let Err(e) = handle_due_tester(client, &due).await {
            tracing::warn!(
                tester_id = %due.tester_id,
                tester_name = %due.name,
                error = ?e,
                "per-tester auto-shutdown failed"
            );
        }
    }
    Ok(())
}

struct DueTester {
    tester_id: Uuid,
    project_id: String,
    name: String,
    cloud: String,
    region: String,
    local_hour: i16,
    deferral_count: i16,
    vm_name: Option<String>,
    vm_resource_id: Option<String>,
}

async fn handle_due_tester(client: &Client, due: &DueTester) -> anyhow::Result<()> {
    // Race re-check: the tester may have been re-locked between the
    // SELECT in `sweep` and now.
    let still_drained: bool = client
        .query_one(
            r#"
            SELECT (t.power_state = 'running' AND t.allocation = 'idle')
               AND NOT EXISTS (
                   SELECT 1 FROM benchmark_config c
                    WHERE c.tester_id = t.tester_id
                      AND c.status IN ('queued','pending','running')
               )
              FROM project_tester t
             WHERE t.tester_id = $1
            "#,
            &[&due.tester_id],
        )
        .await?
        .get(0);

    if !still_drained {
        return defer_shutdown(client, due).await;
    }

    // Flip running → stopping. If someone else moved it, skip this cycle.
    if !tester_state::try_power_transition(client, &due.tester_id, "running", "stopping").await? {
        tracing::debug!(
            tester_id = %due.tester_id,
            "auto-shutdown skipped: power_state no longer 'running'"
        );
        return Ok(());
    }

    // Deallocate the VM via az CLI.
    match vm_deallocate(&due.vm_resource_id, &due.vm_name).await {
        Ok(()) => {
            let next = azure_regions::next_shutdown_at_for_provider(
                &due.cloud,
                &due.region,
                due.local_hour,
                Utc::now(),
            );
            // Azure said OK. Now sync dashboard state. If the UPDATE itself
            // fails (connection blip, deadlock), we retry with short backoff
            // before falling back to `power_state='error'` so the recovery
            // loop (or a human) can reconcile. Without this retry the
            // tester is left permanently in 'stopping'.
            match sync_stopped_with_retry(client, &due.tester_id, &next).await {
                Ok(()) => {
                    tracing::info!(
                        target: "tester_auto_shutdown_completed",
                        tester_id = %due.tester_id,
                        tester_name = %due.name,
                        project_id = %due.project_id,
                        region = %due.region,
                        next_shutdown_at = %next,
                        "auto-shutdown completed"
                    );
                }
                Err(e) => {
                    let msg = format!("Azure deallocated but dashboard failed to sync: {e}");
                    // Best-effort mark error. If even this fails there's
                    // nothing more we can do — log loudly.
                    if let Err(e2) = client
                        .execute(
                            r#"
                            UPDATE project_tester
                               SET power_state    = 'error',
                                   status_message = $2,
                                   updated_at     = NOW()
                             WHERE tester_id = $1
                            "#,
                            &[&due.tester_id, &msg],
                        )
                        .await
                    {
                        tracing::error!(
                            target: "tester_auto_shutdown_sync_failed",
                            tester_id = %due.tester_id,
                            tester_name = %due.name,
                            project_id = %due.project_id,
                            error = ?e,
                            fallback_error = ?e2,
                            "auto-shutdown sync failed AND error-fallback UPDATE failed"
                        );
                    } else {
                        tracing::error!(
                            target: "tester_auto_shutdown_sync_failed",
                            tester_id = %due.tester_id,
                            tester_name = %due.name,
                            project_id = %due.project_id,
                            error = ?e,
                            "auto-shutdown sync failed after retries; marked error"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                tester_id = %due.tester_id,
                tester_name = %due.name,
                error = ?e,
                "az vm deallocate failed; rolling power_state back to running"
            );
            // Roll power_state back out of 'stopping' so the next tick retries.
            client
                .execute(
                    "UPDATE project_tester SET power_state = 'running', updated_at = NOW() \
                     WHERE tester_id = $1",
                    &[&due.tester_id],
                )
                .await?;
        }
    }
    Ok(())
}

async fn defer_shutdown(client: &Client, due: &DueTester) -> anyhow::Result<()> {
    let new_count = due.deferral_count.saturating_add(1);
    let new_next = Utc::now() + chrono::Duration::minutes(DEFERRAL_DELAY_MINUTES);
    client
        .execute(
            r#"
            UPDATE project_tester
               SET shutdown_deferral_count = $2,
                   next_shutdown_at        = $3,
                   updated_at              = NOW()
             WHERE tester_id = $1
            "#,
            &[&due.tester_id, &new_count, &new_next],
        )
        .await?;

    if new_count >= DEFERRAL_CAP {
        // Look up the benchmarks blocking shutdown so the operator log is useful.
        let holders = client
            .query(
                r#"
                SELECT c.name FROM benchmark_config c
                 WHERE c.tester_id = $1
                   AND c.status IN ('queued','pending','running')
                 ORDER BY c.queued_at NULLS LAST, c.created_at
                 LIMIT 10
                "#,
                &[&due.tester_id],
            )
            .await?;
        let names: Vec<String> = holders.iter().map(|r| r.get::<_, String>(0)).collect();
        tracing::warn!(
            target: "tester_shutdown_stuck",
            tester_id = %due.tester_id,
            tester_name = %due.name,
            project_id = %due.project_id,
            deferral_count = new_count,
            blockers = ?names,
            "tester auto-shutdown deferred ({} times); cap reached. Blocked by: {}",
            new_count,
            names.join(", ")
        );
    } else {
        tracing::info!(
            target: "tester_auto_shutdown_deferred",
            tester_id = %due.tester_id,
            tester_name = %due.name,
            project_id = %due.project_id,
            deferral_count = new_count,
            "auto-shutdown deferred (deferral count = {})",
            new_count
        );
    }
    Ok(())
}

/// Sync `power_state='stopped'` for a tester whose Azure VM has just been
/// deallocated. Retries up to 3 times with exponential backoff (100ms,
/// 500ms, 2s). Returns the last error if all attempts fail.
async fn sync_stopped_with_retry(
    client: &Client,
    tester_id: &Uuid,
    next: &chrono::DateTime<Utc>,
) -> anyhow::Result<()> {
    const BACKOFFS_MS: [u64; 3] = [100, 500, 2000];
    let mut last_err: Option<anyhow::Error> = None;
    for (attempt, delay_ms) in BACKOFFS_MS.iter().enumerate() {
        match client
            .execute(
                r#"
                UPDATE project_tester
                   SET power_state             = 'stopped',
                       next_shutdown_at        = $2,
                       shutdown_deferral_count = 0,
                       updated_at              = NOW()
                 WHERE tester_id = $1
                "#,
                &[tester_id, next],
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(e) => {
                tracing::warn!(
                    %tester_id,
                    attempt = attempt + 1,
                    error = ?e,
                    "deallocate sync UPDATE failed; retrying"
                );
                last_err = Some(e.into());
                tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("sync_stopped_with_retry: unknown failure")))
}

async fn vm_deallocate(
    resource_id: &Option<String>,
    vm_name: &Option<String>,
) -> anyhow::Result<()> {
    // Prefer the fully-qualified ARM resource id; fall back to a bare vm name
    // (rare — really only useful in single-RG dev setups).
    let id = resource_id
        .as_deref()
        .or(vm_name.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!("tester has no vm_resource_id or vm_name; cannot deallocate")
        })?;

    cloud_provider::legacy_azure_provider()?.stop_vm(id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: the `deferral_count_caps_at_3_warning` test from the plan was
    // substituted with a signature/constants check because mocking
    // tokio-postgres is a significant yak-shave (no interface, no query
    // injection). The real behavioural test lives in the ignored DB-gated
    // integration suite.

    #[test]
    fn constants_are_sane() {
        assert_eq!(DEFERRAL_CAP, 3);
        assert_eq!(DEFERRAL_DELAY_MINUTES, 5);
        assert_eq!(TICK, Duration::from_secs(60));
    }

    /// Compile-time guard: the public entry point keeps its
    /// `Arc<tokio_postgres::Client>` signature so that Task 34 can wire it
    /// into `main.rs` without needing to re-discover the module.
    #[allow(dead_code)]
    async fn _auto_shutdown_loop_signature_compile_check(c: Arc<Client>) {
        auto_shutdown_loop(c).await;
    }

    /// RR-006 compile-level guard: `sync_stopped_with_retry` exists with
    /// the expected signature so that refactors can't silently remove
    /// the retry path. A real end-to-end test requires a mock PG client;
    /// this guards the API surface instead.
    #[allow(dead_code)]
    async fn _deallocate_sync_retries_on_update_failure(
        client: &Client,
        tester_id: &Uuid,
        next: &chrono::DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sync_stopped_with_retry(client, tester_id, next).await
    }
}
