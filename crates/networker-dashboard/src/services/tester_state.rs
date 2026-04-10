//! Tester lock primitives — the only authoritative writers of the
//! (allocation, locked_by_config_id) pair on `project_tester`.
//!
//! `release` and `force_release` are the ONLY functions allowed to clear
//! the pair. A grep-guard test in Task 7 enforces this invariant.

#![allow(dead_code)] // downstream tasks will wire these in

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

pub async fn set_status_message(
    client: &Client,
    tester_id: &Uuid,
    msg: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE project_tester SET status_message = $2, updated_at = NOW() WHERE tester_id = $1",
            &[tester_id, &msg],
        )
        .await?;
    Ok(())
}

/// Clear the lock pair unconditionally. ONLY the recovery loop on
/// dashboard restart (Task 12) is allowed to call this.
pub async fn force_release(client: &Client, tester_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            r#"
            UPDATE project_tester
               SET allocation          = 'idle',
                   locked_by_config_id = NULL,
                   updated_at          = NOW()
             WHERE tester_id           = $1
            "#,
            &[tester_id],
        )
        .await?;
    Ok(())
}
