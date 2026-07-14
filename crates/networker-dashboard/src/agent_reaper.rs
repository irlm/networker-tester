//! Stale-agent status reconciler.
//!
//! An agent is *truly* online iff its WebSocket is registered in the
//! in-memory [`crate::ws::agent_hub::AgentHub`]. The `agent.status` column is
//! a cache written on connect/disconnect — but it goes stale in two situations
//! that leave a dead agent showing "online" forever:
//!
//!   1. **Unclean disconnect** — the VM is force-deallocated (credit
//!      exhaustion, manual stop, cloud maintenance) and the agent process
//!      dies without the WS close frame ever reaching the dashboard.
//!   2. **Dashboard restart** — the process that wrote `status='online'` is
//!      gone; the new process has an empty hub, but the DB still says online.
//!
//! This loop reconciles the cache against ground truth every 60s: any agent
//! the DB thinks is online but which is **not in the live hub** and whose
//! last heartbeat is stale gets flipped to `offline`. Hub membership is
//! authoritative, so it never marks a genuinely-connected agent offline and
//! is safe to run alongside the WS connect/disconnect handlers.

use std::sync::Arc;
use std::time::Duration;

use networker_common::messages::DashboardEvent;

use crate::AppState;

const TICK: Duration = Duration::from_secs(60);

/// Heartbeat is sent every 30s (see networker-agent `heartbeat::run`). Three
/// missed beats is a confident "gone" signal without flapping on a single
/// lagged heartbeat.
const STALE_AFTER_SECS: i64 = 90;

/// Spawn the reconciler loop. Cheap: one indexed query per tick plus a hub
/// membership check per flagged row.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = reconcile_once(&state).await {
                tracing::warn!(error = %e, "agent status reconcile tick failed");
            }
        }
    });
}

async fn reconcile_once(state: &Arc<AppState>) -> anyhow::Result<()> {
    let client = state.db.get().await?;

    // Candidates: DB says online, but heartbeat is stale (or never arrived).
    // A NULL last_heartbeat with status=online means the connect handler wrote
    // 'online' but the process died before the first beat — also stale.
    let rows = client
        .query(
            "SELECT agent_id, name, last_heartbeat \
             FROM agent \
             WHERE status = 'online' \
               AND (last_heartbeat IS NULL \
                    OR last_heartbeat < NOW() - make_interval(secs => $1))",
            &[&(STALE_AFTER_SECS as f64)],
        )
        .await?;

    let mut reaped = 0u32;
    for row in &rows {
        let agent_id: uuid::Uuid = row.get("agent_id");
        // Hub membership is authoritative: a heartbeat can lag under load
        // while the socket is still very much alive. Only reap agents that
        // are genuinely absent from the live registry.
        if state.agents.is_agent_online(&agent_id).await {
            continue;
        }
        let name: String = row.get("name");
        crate::db::agents::update_status(&client, &agent_id, "offline").await?;
        let _ = state.events_tx.send(DashboardEvent::AgentStatus {
            agent_id,
            status: "offline".into(),
            last_heartbeat: row.get("last_heartbeat"),
        });
        reaped += 1;
        tracing::info!(
            %agent_id,
            agent_name = %name,
            "Marked stale agent offline (no heartbeat, not in live hub)"
        );
    }

    if reaped > 0 {
        tracing::info!(
            count = reaped,
            "agent status reconcile: flipped stale agents offline"
        );
    }
    Ok(())
}
