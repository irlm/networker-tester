//! Status writing: `benchmark_config` status/phase persistence, the
//! orchestrator's direct Postgres connection, and dashboard callbacks.

use crate::callback::CallbackClient;
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_postgres::{Client as PgClient, NoTls};
use uuid::Uuid;

/// RR-004: persist a terminal `benchmark_config.status` write, logging a
/// structured `error`-level event (target=`orchestrator_terminal_status_write_failed`)
/// on failure. Recovery logic (dashboard's `tester_recovery` periodic sweep)
/// keys on terminal status writes landing in the DB — silently dropping the
/// error leaks the tester lock indefinitely. We cannot retry from here (no
/// queue) but making the failure visible is far better than continuing
/// silently.
pub(super) async fn write_terminal_status(client: &PgClient, config_id: &Uuid, status: &str) {
    if let Err(e) = set_benchmark_status(client, config_id, status).await {
        tracing::error!(
            target: "orchestrator_terminal_status_write_failed",
            config_id = %config_id,
            status = %status,
            error = ?e,
            "CRITICAL: failed to persist terminal status; tester may become stuck"
        );
    }
}

/// RR-004: phase writes are advisory, but we still want visibility when
/// they fail so an operator can correlate missing phase markers with
/// DB connectivity incidents.
pub(super) async fn write_phase(client: &PgClient, config_id: &Uuid, phase: &str) {
    if let Err(e) = set_phase(client, config_id, phase).await {
        tracing::error!(
            target: "orchestrator_phase_write_failed",
            config_id = %config_id,
            phase = %phase,
            error = ?e,
            "failed to persist benchmark phase marker"
        );
    }
}

// ---------------------------------------------------------------------------
// DB plumbing helpers for the persistent-tester lock flow.
//
// The orchestrator historically had no direct DB access — it only spoke to
// the dashboard via HTTP callbacks. Task 23 introduces direct Postgres access
// so the orchestrator can participate in the tester-lock protocol without a
// round-trip-heavy callback API.
//
// For MVP we lazily construct a single short-lived connection inside
// `execute_testbed_application` by reading `ORCHESTRATOR_DB_URL` (fallback
// `DASHBOARD_DB_URL`). A top-down `Arc<Client>` is the cleaner end state but
// would touch far more files; see the persistent-testers plan for the
// follow-up refactor.
// ---------------------------------------------------------------------------

/// Lazily connect to Postgres using `ORCHESTRATOR_DB_URL` or `DASHBOARD_DB_URL`.
/// The spawned background task drives the connection to completion; callers
/// keep the returned `Client` for the duration of the work.
pub(super) async fn connect_orchestrator_db() -> Result<Arc<PgClient>> {
    let url = std::env::var("ORCHESTRATOR_DB_URL")
        .or_else(|_| std::env::var("DASHBOARD_DB_URL"))
        .context("ORCHESTRATOR_DB_URL (or DASHBOARD_DB_URL) must be set for tester-lock flow")?;
    let (client, conn) = tokio_postgres::connect(&url, NoTls)
        .await
        .context("connecting to Postgres for tester-lock flow")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::error!("orchestrator Postgres connection error: {e:#}");
        }
    });
    Ok(Arc::new(client))
}

/// Update `benchmark_config.current_phase` — a lightweight progress marker
/// consumed by the dashboard's phase-update WebSocket hub (future task).
async fn set_phase(client: &PgClient, config_id: &Uuid, phase: &str) -> Result<()> {
    client
        .execute(
            "UPDATE benchmark_config SET current_phase = $2, updated_at = NOW() \
             WHERE config_id = $1",
            &[config_id, &phase],
        )
        .await
        .with_context(|| format!("set_phase({phase}) for config {config_id} failed"))?;
    Ok(())
}

/// Update `benchmark_config.status`. When transitioning into `queued`, also
/// stamp `queued_at = NOW()` so the dispatcher's fairness ordering is correct.
async fn set_benchmark_status(client: &PgClient, config_id: &Uuid, status: &str) -> Result<()> {
    if status == "queued" {
        client
            .execute(
                "UPDATE benchmark_config \
                    SET status = 'queued', queued_at = NOW(), updated_at = NOW() \
                  WHERE config_id = $1",
                &[config_id],
            )
            .await
            .with_context(|| format!("set status=queued for config {config_id}"))?;
    } else {
        client
            .execute(
                "UPDATE benchmark_config SET status = $2, updated_at = NOW() \
                 WHERE config_id = $1",
                &[config_id, &status],
            )
            .await
            .with_context(|| format!("set status={status} for config {config_id}"))?;
    }
    Ok(())
}

/// TODO(Task 10 integration): push a `promote_next` event to the tester
/// dispatcher (a separate dashboard process). For MVP this is a tracing-only
/// stub — the dispatcher's periodic sweep (every 30s) will notice any dropped
/// events and still make forward progress.
pub(super) async fn notify_queue_dispatcher(tester_id: &Uuid) {
    tracing::info!(
        tester_id = %tester_id,
        "notify_queue_dispatcher stub — dispatcher sweep will promote next queued config"
    );
}

/// Helper: send a status callback, logging errors but not failing.
pub(super) async fn status_callback(
    callback: &CallbackClient,
    testbed_id: &str,
    status: &str,
    current_language: &str,
    language_index: u32,
    language_total: u32,
    message: &str,
) {
    if let Err(e) = callback
        .status(
            testbed_id,
            status,
            current_language,
            language_index,
            language_total,
            message,
        )
        .await
    {
        tracing::warn!("Status callback failed: {e}");
    }
}

/// Helper: send a log callback, logging errors but not failing.
pub(super) async fn log_callback(callback: &CallbackClient, testbed_id: &str, lines: Vec<String>) {
    if let Err(e) = callback.log(testbed_id, lines).await {
        tracing::warn!("Log callback failed: {e}");
    }
}
