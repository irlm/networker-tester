use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ShareLinkRow {
    pub link_id: Uuid,
    pub project_id: Uuid,
    pub token_hash: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub label: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
    pub access_count: i32,
    pub last_accessed: Option<DateTime<Utc>>,
    pub created_by_email: String,
}

#[allow(clippy::too_many_arguments)]
pub async fn create_link(
    client: &Client,
    project_id: &Uuid,
    token_hash: &str,
    resource_type: &str,
    resource_id: Option<&Uuid>,
    label: Option<&str>,
    expires_at: &DateTime<Utc>,
    created_by: &Uuid,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO share_link (link_id, project_id, token_hash, resource_type, resource_id, label, expires_at, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                &id,
                project_id,
                &token_hash,
                &resource_type,
                &resource_id,
                &label,
                expires_at,
                created_by,
            ],
        )
        .await?;
    Ok(id)
}

pub async fn list_links(client: &Client, project_id: &Uuid) -> anyhow::Result<Vec<ShareLinkRow>> {
    let rows = client
        .query(
            "SELECT s.link_id, s.project_id, s.token_hash, s.resource_type, s.resource_id,
                    s.label, s.expires_at, s.created_by, s.created_at, s.revoked,
                    s.access_count, s.last_accessed,
                    COALESCE(u.email, 'unknown') AS created_by_email
             FROM share_link s
             LEFT JOIN dash_user u ON u.user_id = s.created_by
             WHERE s.project_id = $1
             ORDER BY s.created_at DESC",
            &[project_id],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| ShareLinkRow {
            link_id: r.get("link_id"),
            project_id: r.get("project_id"),
            token_hash: r.get("token_hash"),
            resource_type: r.get("resource_type"),
            resource_id: r.get("resource_id"),
            label: r.get("label"),
            expires_at: r.get("expires_at"),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
            revoked: r.get("revoked"),
            access_count: r.get("access_count"),
            last_accessed: r.get("last_accessed"),
            created_by_email: r.get("created_by_email"),
        })
        .collect())
}

pub async fn resolve_link(
    client: &Client,
    token_hash: &str,
) -> anyhow::Result<Option<ShareLinkRow>> {
    let row = client
        .query_opt(
            "UPDATE share_link
             SET access_count = access_count + 1, last_accessed = now()
             WHERE token_hash = $1 AND revoked = FALSE AND expires_at > now()
             RETURNING link_id, project_id, token_hash, resource_type, resource_id,
                       label, expires_at, created_by, created_at, revoked,
                       access_count, last_accessed",
            &[&token_hash],
        )
        .await?;

    match row {
        Some(r) => {
            // Fetch creator email separately
            let created_by: Uuid = r.get("created_by");
            let email_row = client
                .query_opt(
                    "SELECT email FROM dash_user WHERE user_id = $1",
                    &[&created_by],
                )
                .await?;
            let email = email_row
                .map(|er| er.get::<_, String>("email"))
                .unwrap_or_else(|| "unknown".to_string());

            Ok(Some(ShareLinkRow {
                link_id: r.get("link_id"),
                project_id: r.get("project_id"),
                token_hash: r.get("token_hash"),
                resource_type: r.get("resource_type"),
                resource_id: r.get("resource_id"),
                label: r.get("label"),
                expires_at: r.get("expires_at"),
                created_by,
                created_at: r.get("created_at"),
                revoked: r.get("revoked"),
                access_count: r.get("access_count"),
                last_accessed: r.get("last_accessed"),
                created_by_email: email,
            }))
        }
        None => Ok(None),
    }
}

pub async fn revoke_link(client: &Client, link_id: &Uuid, project_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE share_link SET revoked = TRUE WHERE link_id = $1 AND project_id = $2",
            &[link_id, project_id],
        )
        .await?;
    Ok(())
}

pub async fn delete_link(client: &Client, link_id: &Uuid, project_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "DELETE FROM share_link WHERE link_id = $1 AND project_id = $2",
            &[link_id, project_id],
        )
        .await?;
    Ok(())
}

pub async fn extend_link(
    client: &Client,
    link_id: &Uuid,
    new_expires: &DateTime<Utc>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE share_link SET expires_at = $1 WHERE link_id = $2",
            &[new_expires, link_id],
        )
        .await?;
    Ok(())
}
