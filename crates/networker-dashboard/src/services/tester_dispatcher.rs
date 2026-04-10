//! Tester dispatcher — promotes queued benchmarks to `pending` on testers
//! that are running+idle.
//!
//! `promote_next` is the atomic per-tester promotion step (uses
//! `FOR UPDATE SKIP LOCKED` so concurrent sweeps cannot double-promote).
//! `sweep_loop` is a 30s tick that finds candidate testers and calls
//! `promote_next` for each.
//!
//! Task 34 wires `sweep_loop` into `main.rs` with an
//! `Arc<tokio_postgres::Client>` carved out of the dashboard pool as a
//! dedicated dispatcher connection.

#![allow(dead_code)] // Task 34 wires this into main.rs

use std::sync::Arc;
use std::time::Duration;

use tokio_postgres::Client;
use uuid::Uuid;

/// Atomically promote the oldest queued benchmark for `tester_id` from
/// `queued` → `pending`. Returns the promoted `config_id`, or `None` if
/// the queue was empty (or lost a race to a concurrent sweep).
pub async fn promote_next(client: &Client, tester_id: &Uuid) -> anyhow::Result<Option<Uuid>> {
    let row = client
        .query_opt(
            r#"
            UPDATE benchmark_config
               SET status    = 'pending',
                   queued_at = NULL
             WHERE config_id = (
                 SELECT config_id FROM benchmark_config
                  WHERE tester_id = $1 AND status = 'queued'
                  ORDER BY queued_at ASC
                  LIMIT 1
                  FOR UPDATE SKIP LOCKED
             )
             RETURNING config_id
            "#,
            &[tester_id],
        )
        .await?;

    Ok(row.map(|r| r.get::<_, Uuid>(0)))
}

/// One sweep pass: find every `running`+`idle` tester with at least one
/// `queued` benchmark and call `promote_next` on it.
async fn sweep_tick(client: &Client) -> anyhow::Result<()> {
    let rows = client
        .query(
            r#"
            SELECT DISTINCT t.tester_id
              FROM project_tester t
              JOIN benchmark_config b ON b.tester_id = t.tester_id
             WHERE t.power_state = 'running'
               AND t.allocation  = 'idle'
               AND b.status      = 'queued'
            "#,
            &[],
        )
        .await?;

    tracing::debug!(candidates = rows.len(), "tester dispatcher sweep tick");

    for row in rows {
        let tester_id: Uuid = row.get(0);
        match promote_next(client, &tester_id).await {
            Ok(Some(config_id)) => {
                tracing::info!(
                    %tester_id,
                    %config_id,
                    "dispatcher promoted queued benchmark"
                );
            }
            Ok(None) => {
                // Race: queue drained between our SELECT DISTINCT and the
                // UPDATE ... SKIP LOCKED. Benign.
            }
            Err(e) => {
                tracing::warn!(
                    %tester_id,
                    error = ?e,
                    "promote_next failed"
                );
            }
        }
    }

    Ok(())
}

/// Long-running background sweep. Ticks every 30 seconds and never
/// returns; spawn it with `tokio::spawn`.
///
/// Takes an `Arc<Client>` rather than a pool reference to stay crate-
/// agnostic — callers in `main.rs` reserve one dedicated connection for
/// the dispatcher and hand it in wrapped in an `Arc`.
pub async fn sweep_loop(client: Arc<Client>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(e) = sweep_tick(&client).await {
            tracing::warn!(error = ?e, "tester dispatcher sweep failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO(task-7-followup): a proper mocked-DB test requires standing up
    // a tokio-postgres mock (or a real ephemeral Postgres) which is out
    // of scope for this task. The signature-only assertion below catches
    // refactor regressions; the runtime race coverage lives in
    // `tester_state.rs` integration tests.
    // Compile-time smoke: if someone refactors `promote_next` so it no
    // longer accepts `(&Client, &Uuid)` and returns an
    // `anyhow::Result<Option<Uuid>>`-returning future, this `async fn`
    // fails to type-check.
    #[allow(dead_code)]
    async fn _promote_next_signature_compile_check(
        client: &Client,
        tester_id: &Uuid,
    ) -> anyhow::Result<Option<Uuid>> {
        promote_next(client, tester_id).await
    }

    #[tokio::test]
    async fn sweep_loop_takes_arc_client() {
        // Type-level check only — ensures `sweep_loop` accepts exactly
        // `Arc<Client>`. We don't actually run it (would need a live DB).
        let _f: fn(Arc<Client>) -> _ = sweep_loop;
        let _ = _f;
    }
}
