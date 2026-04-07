use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct InviteRow {
    pub invite_id: Uuid,
    pub project_id: String,
    pub email: String,
    pub role: String,
    pub status: String,
    pub invited_by: Uuid,
    pub invited_by_email: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedInvite {
    pub invite_id: Uuid,
    pub project_id: String,
    pub project_name: String,
    pub email: String,
    pub role: String,
    pub has_account: bool,
    pub expires_at: DateTime<Utc>,
}

/// SHA-256 hash a token string for storage/lookup.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Create a workspace invite.
pub async fn create_invite(
    client: &Client,
    project_id: &str,
    email: &str,
    role: &str,
    token_hash: &str,
    invited_by: &Uuid,
    expires_at: &DateTime<Utc>,
) -> anyhow::Result<Uuid> {
    let row = client
        .query_one(
            "INSERT INTO workspace_invite (project_id, email, role, token_hash, invited_by, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING invite_id",
            &[&project_id, &email, &role, &token_hash, invited_by, expires_at],
        )
        .await?;
    Ok(row.get("invite_id"))
}

/// List invites for a project, with inviter email via JOIN.
pub async fn list_invites(client: &Client, project_id: &str) -> anyhow::Result<Vec<InviteRow>> {
    let rows = client
        .query(
            "SELECT i.invite_id, i.project_id, i.email, i.role, i.status, \
                    i.invited_by, u.email AS invited_by_email, \
                    i.created_at, i.expires_at, i.accepted_at \
             FROM workspace_invite i \
             JOIN dash_user u ON u.user_id = i.invited_by \
             WHERE i.project_id = $1 \
             ORDER BY i.created_at DESC",
            &[&project_id],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| InviteRow {
            invite_id: r.get("invite_id"),
            project_id: r.get("project_id"),
            email: r.get("email"),
            role: r.get("role"),
            status: r.get("status"),
            invited_by: r.get("invited_by"),
            invited_by_email: r.get("invited_by_email"),
            created_at: r.get("created_at"),
            expires_at: r.get("expires_at"),
            accepted_at: r.get("accepted_at"),
        })
        .collect())
}

/// Resolve an invite by token hash. Returns invite details if pending and not expired.
pub async fn resolve_invite(
    client: &Client,
    token_hash: &str,
) -> anyhow::Result<Option<ResolvedInvite>> {
    let row = client
        .query_opt(
            "SELECT i.invite_id, i.project_id, p.name AS project_name, i.email, i.role, i.expires_at \
             FROM workspace_invite i \
             JOIN project p ON p.project_id = i.project_id \
             WHERE i.token_hash = $1 AND i.status = 'pending' AND i.expires_at > now()",
            &[&token_hash],
        )
        .await?;

    match row {
        Some(r) => {
            let email: String = r.get("email");
            // Check if the invited email already has an account
            let existing = client
                .query_opt(
                    "SELECT user_id FROM dash_user WHERE LOWER(email) = LOWER($1) AND status = 'active'",
                    &[&email],
                )
                .await?;

            Ok(Some(ResolvedInvite {
                invite_id: r.get("invite_id"),
                project_id: r.get("project_id"),
                project_name: r.get("project_name"),
                email,
                role: r.get("role"),
                has_account: existing.is_some(),
                expires_at: r.get("expires_at"),
            }))
        }
        None => Ok(None),
    }
}

/// Accept an invite: set status to 'accepted' with timestamp and accepting user.
pub async fn accept_invite(
    client: &Client,
    invite_id: &Uuid,
    user_id: &Uuid,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE workspace_invite SET status = 'accepted', accepted_at = now(), accepted_by = $1 \
             WHERE invite_id = $2",
            &[user_id, invite_id],
        )
        .await?;
    Ok(())
}

/// Revoke a pending invite.
pub async fn revoke_invite(
    client: &Client,
    invite_id: &Uuid,
    project_id: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE workspace_invite SET status = 'revoked' \
             WHERE invite_id = $1 AND project_id = $2 AND status = 'pending'",
            &[invite_id, &project_id],
        )
        .await?;
    Ok(())
}

/// Expire all stale invites (pending + past expiry). Returns number of rows updated.
pub async fn expire_stale_invites(client: &Client) -> anyhow::Result<u64> {
    let n = client
        .execute(
            "UPDATE workspace_invite SET status = 'expired' \
             WHERE status = 'pending' AND expires_at <= now()",
            &[],
        )
        .await?;
    Ok(n)
}
