//! Persistence helpers for the `agent_command` table (V033).
//!
//! Centralises the SQL used by the dispatch service and the WS ingestion
//! path so both code paths agree on column names and status strings.
//!
//! Several helpers are consumed by the REST API that lands in a later task;
//! silence dead-code warnings until then.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

/// A row in `agent_command`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCommandRow {
    pub command_id: Uuid,
    pub agent_id: Uuid,
    pub config_id: Option<Uuid>,
    pub verb: String,
    pub args: serde_json::Value,
    pub status: String,
    pub result: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

fn row_to_record(r: &tokio_postgres::Row) -> AgentCommandRow {
    AgentCommandRow {
        command_id: r.get("command_id"),
        agent_id: r.get("agent_id"),
        config_id: r.get("config_id"),
        verb: r.get("verb"),
        args: r.get("args"),
        status: r.get("status"),
        result: r.get("result"),
        error_message: r.get("error_message"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
    }
}

/// Insert a new pending command row.
pub async fn insert_pending(
    client: &Client,
    command_id: &Uuid,
    agent_id: &Uuid,
    config_id: Option<&Uuid>,
    verb: &str,
    args: &serde_json::Value,
    created_by: Option<&Uuid>,
) -> anyhow::Result<()> {
    client
        .execute(
            "INSERT INTO agent_command \
               (command_id, agent_id, config_id, verb, args, status, created_by) \
             VALUES ($1, $2, $3, $4, $5, 'pending', $6)",
            &[
                &command_id,
                &agent_id,
                &config_id,
                &verb,
                &args,
                &created_by,
            ],
        )
        .await?;
    Ok(())
}

/// Stamp `started_at` to NOW() if it is currently NULL. Idempotent.
pub async fn mark_started(client: &Client, command_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE agent_command \
                SET started_at = NOW() \
              WHERE command_id = $1 \
                AND started_at IS NULL",
            &[&command_id],
        )
        .await?;
    Ok(())
}

/// Mark a command as terminal (ok/error/timeout/cancelled) and persist its
/// result payload + error message. Also stamps `finished_at` and, if
/// `started_at` was never set, back-fills it with NOW().
pub async fn mark_finished(
    client: &Client,
    command_id: &Uuid,
    status: &str,
    result: Option<&serde_json::Value>,
    error_message: Option<&str>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE agent_command SET \
               status = $2, \
               result = $3, \
               error_message = $4, \
               finished_at = NOW(), \
               started_at = COALESCE(started_at, NOW()) \
             WHERE command_id = $1",
            &[&command_id, &status, &result, &error_message],
        )
        .await?;
    Ok(())
}

/// Mark a command as errored synchronously from the dispatcher (before it
/// ever reaches the agent). Unlike `mark_finished` this does NOT back-fill
/// `started_at` — the command never actually ran.
pub async fn mark_dispatch_error(
    client: &Client,
    command_id: &Uuid,
    error_message: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE agent_command SET \
               status = 'error', \
               error_message = $2, \
               finished_at = NOW() \
             WHERE command_id = $1",
            &[&command_id, &error_message],
        )
        .await?;
    Ok(())
}

#[allow(dead_code)] // used by upcoming REST API in later plan tasks
pub async fn fetch_by_id(
    client: &Client,
    command_id: &Uuid,
) -> anyhow::Result<Option<AgentCommandRow>> {
    let row = client
        .query_opt(
            "SELECT command_id, agent_id, config_id, verb, args, status, \
                    result, error_message, created_by, created_at, \
                    started_at, finished_at \
             FROM agent_command WHERE command_id = $1",
            &[&command_id],
        )
        .await?;
    Ok(row.map(|r| row_to_record(&r)))
}

#[allow(dead_code)] // used by upcoming REST API in later plan tasks
pub async fn list_for_agent(
    client: &Client,
    agent_id: &Uuid,
    limit: i64,
) -> anyhow::Result<Vec<AgentCommandRow>> {
    let rows = client
        .query(
            "SELECT command_id, agent_id, config_id, verb, args, status, \
                    result, error_message, created_by, created_at, \
                    started_at, finished_at \
             FROM agent_command \
             WHERE agent_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
            &[&agent_id, &limit],
        )
        .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

/// Map our strongly-typed `CommandStatus` enum to the text stored in the
/// `status` column. Centralised so the mapping is not re-invented by each
/// call site.
pub fn command_status_str(status: &networker_common::messages::CommandStatus) -> &'static str {
    use networker_common::messages::CommandStatus;
    match status {
        CommandStatus::Ok => "ok",
        CommandStatus::Error => "error",
        CommandStatus::Timeout => "timeout",
        CommandStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use networker_common::messages::CommandStatus;

    #[test]
    fn command_status_str_covers_all_variants() {
        assert_eq!(command_status_str(&CommandStatus::Ok), "ok");
        assert_eq!(command_status_str(&CommandStatus::Error), "error");
        assert_eq!(command_status_str(&CommandStatus::Timeout), "timeout");
        assert_eq!(command_status_str(&CommandStatus::Cancelled), "cancelled");
    }
}
