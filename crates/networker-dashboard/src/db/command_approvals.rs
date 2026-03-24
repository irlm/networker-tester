use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ApprovalRow {
    pub approval_id: Uuid,
    pub project_id: Uuid,
    pub agent_id: Uuid,
    pub command_type: String,
    pub command_detail: serde_json::Value,
    pub status: String,
    pub requested_by: Uuid,
    pub requested_by_email: String,
    pub decided_by: Option<Uuid>,
    pub decided_by_email: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub reason: Option<String>,
}

fn row_to_approval(r: &tokio_postgres::Row) -> ApprovalRow {
    ApprovalRow {
        approval_id: r.get("approval_id"),
        project_id: r.get("project_id"),
        agent_id: r.get("agent_id"),
        command_type: r.get("command_type"),
        command_detail: r.get("command_detail"),
        status: r.get("status"),
        requested_by: r.get("requested_by"),
        requested_by_email: r.get("requested_by_email"),
        decided_by: r.get("decided_by"),
        decided_by_email: r.get("decided_by_email"),
        requested_at: r.get("requested_at"),
        decided_at: r.get("decided_at"),
        expires_at: r.get("expires_at"),
        reason: r.get("reason"),
    }
}

#[allow(dead_code)] // Will be called from agent command handlers in a later PR
pub async fn create_approval(
    client: &Client,
    project_id: &Uuid,
    agent_id: &Uuid,
    command_type: &str,
    command_detail: &serde_json::Value,
    requested_by: &Uuid,
    expires_at: &DateTime<Utc>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO command_approval
                (approval_id, project_id, agent_id, command_type, command_detail, requested_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[&id, project_id, agent_id, &command_type, command_detail, requested_by, expires_at],
        )
        .await?;
    Ok(id)
}

const SELECT_WITH_JOINS: &str = "
    SELECT ca.approval_id, ca.project_id, ca.agent_id, ca.command_type,
           ca.command_detail, ca.status, ca.requested_by,
           req.email AS requested_by_email,
           ca.decided_by,
           dec.email AS decided_by_email,
           ca.requested_at, ca.decided_at, ca.expires_at, ca.reason
    FROM command_approval ca
    JOIN dash_user req ON req.user_id = ca.requested_by
    LEFT JOIN dash_user dec ON dec.user_id = ca.decided_by
";

pub async fn list_pending(client: &Client, project_id: &Uuid) -> anyhow::Result<Vec<ApprovalRow>> {
    let rows = client
        .query(
            &format!(
                "{SELECT_WITH_JOINS}
                 WHERE ca.project_id = $1 AND ca.status = 'pending' AND ca.expires_at > now()
                 ORDER BY ca.requested_at ASC"
            ),
            &[project_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_approval).collect())
}

pub async fn get_pending_count(client: &Client, project_id: &Uuid) -> anyhow::Result<i64> {
    let row = client
        .query_one(
            "SELECT COUNT(*) FROM command_approval
             WHERE project_id = $1 AND status = 'pending' AND expires_at > now()",
            &[project_id],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn get_approval(
    client: &Client,
    approval_id: &Uuid,
    project_id: &Uuid,
) -> anyhow::Result<Option<ApprovalRow>> {
    let row = client
        .query_opt(
            &format!(
                "{SELECT_WITH_JOINS}
                 WHERE ca.approval_id = $1 AND ca.project_id = $2"
            ),
            &[approval_id, project_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_approval))
}

pub async fn decide(
    client: &Client,
    approval_id: &Uuid,
    project_id: &Uuid,
    decided_by: &Uuid,
    approved: bool,
    reason: Option<&str>,
) -> anyhow::Result<()> {
    let status = if approved { "approved" } else { "denied" };
    client
        .execute(
            "UPDATE command_approval
             SET status = $1, decided_by = $2, decided_at = now(), reason = $3
             WHERE approval_id = $4 AND project_id = $5 AND status = 'pending'",
            &[&status, decided_by, &reason, approval_id, project_id],
        )
        .await?;
    Ok(())
}

pub async fn expire_stale(client: &Client) -> anyhow::Result<u64> {
    let count = client
        .execute(
            "UPDATE command_approval SET status = 'expired'
             WHERE status = 'pending' AND expires_at <= now()",
            &[],
        )
        .await?;
    Ok(count)
}
