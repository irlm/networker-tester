//! Tester lock primitives for the orchestrator.
//!
//! These are DUPLICATED from `crates/networker-dashboard/src/services/tester_state.rs`
//! until a shared crate exists. The dashboard's grep-guard test is scoped to
//! `crates/networker-dashboard/src/services/` so the duplication does not
//! trigger it; any update to the canonical version MUST be mirrored here.
//!
//! Only `release` and `force_release` are authorised to clear the
//! `(allocation, locked_by_config_id)` pair.

#![allow(dead_code)]

use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum AcquireOutcome {
    Acquired,
    NeedsStart,
    Transient(String),
    Upgrading,
    AlreadyLockedBy(Uuid),
    Errored,
    NotIdle(String),
}

pub async fn try_acquire(
    client: &Client,
    tester_id: &Uuid,
    config_id: &Uuid,
) -> anyhow::Result<AcquireOutcome> {
    let row = client
        .query_opt(
            r#"
            UPDATE project_tester
               SET allocation          = 'locked',
                   locked_by_config_id = $2,
                   last_used_at        = NOW(),
                   updated_at          = NOW()
             WHERE tester_id           = $1
               AND power_state         = 'running'
               AND allocation          = 'idle'
               AND locked_by_config_id IS NULL
             RETURNING tester_id
            "#,
            &[tester_id, config_id],
        )
        .await?;

    if row.is_some() {
        return Ok(AcquireOutcome::Acquired);
    }

    let cur = client
        .query_one(
            "SELECT power_state, allocation, locked_by_config_id \
             FROM project_tester WHERE tester_id = $1",
            &[tester_id],
        )
        .await?;
    let power: String = cur.get(0);
    let alloc: String = cur.get(1);
    let locker: Option<Uuid> = cur.get(2);

    Ok(match (power.as_str(), alloc.as_str()) {
        ("stopped", _) => AcquireOutcome::NeedsStart,
        ("starting" | "stopping" | "provisioning", _) => AcquireOutcome::Transient(power),
        ("running", "locked") => AcquireOutcome::AlreadyLockedBy(locker.unwrap_or_else(Uuid::nil)),
        ("running", "upgrading") => AcquireOutcome::Upgrading,
        ("error", _) => AcquireOutcome::Errored,
        _ => AcquireOutcome::NotIdle(format!("{power}/{alloc}")),
    })
}

pub async fn release(client: &Client, tester_id: &Uuid, config_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            r#"
            UPDATE project_tester
               SET allocation          = 'idle',
                   locked_by_config_id = NULL,
                   updated_at          = NOW()
             WHERE tester_id           = $1
               AND locked_by_config_id = $2
            "#,
            &[tester_id, config_id],
        )
        .await?;
    Ok(())
}

pub async fn try_power_transition(
    client: &Client,
    tester_id: &Uuid,
    expected: &str,
    next: &str,
) -> anyhow::Result<bool> {
    let rows = client
        .execute(
            r#"
            UPDATE project_tester
               SET power_state = $3,
                   updated_at  = NOW()
             WHERE tester_id   = $1
               AND power_state = $2
            "#,
            &[tester_id, &expected, &next],
        )
        .await?;
    Ok(rows == 1)
}
