use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug)]
#[allow(dead_code)]
pub struct CloudAccountRow {
    pub account_id: Uuid,
    pub project_id: String,
    pub owner_id: Option<Uuid>,
    pub name: String,
    pub provider: String,
    pub credentials_enc: Vec<u8>,
    pub credentials_nonce: Vec<u8>,
    pub region_default: Option<String>,
    pub status: String,
    pub last_validated: Option<DateTime<Utc>>,
    pub validation_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CloudAccountSummary {
    pub account_id: Uuid,
    pub name: String,
    pub provider: String,
    pub region_default: Option<String>,
    pub personal: bool,
    pub status: String,
    pub last_validated: Option<DateTime<Utc>>,
}

pub async fn list_accounts(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
) -> anyhow::Result<Vec<CloudAccountSummary>> {
    // Show project-shared accounts (owner_id IS NULL) + user's personal accounts
    let rows = client
        .query(
            "SELECT account_id, name, provider, region_default, owner_id, status, last_validated \
             FROM cloud_account \
             WHERE project_id = $1 AND (owner_id IS NULL OR owner_id = $2) \
             ORDER BY created_at",
            &[&project_id, user_id],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| {
            let owner_id: Option<Uuid> = r.get("owner_id");
            CloudAccountSummary {
                account_id: r.get("account_id"),
                name: r.get("name"),
                provider: r.get("provider"),
                region_default: r.get("region_default"),
                personal: owner_id.is_some(),
                status: r.get("status"),
                last_validated: r.get("last_validated"),
            }
        })
        .collect())
}

pub async fn get_account(
    client: &Client,
    account_id: &Uuid,
    project_id: &str,
) -> anyhow::Result<Option<CloudAccountRow>> {
    let row = client
        .query_opt(
            "SELECT account_id, project_id, owner_id, name, provider, \
                    credentials_enc, credentials_nonce, region_default, \
                    status, last_validated, validation_error, created_at, updated_at \
             FROM cloud_account WHERE account_id = $1 AND project_id = $2",
            &[account_id, &project_id],
        )
        .await?;

    Ok(row.map(|r| CloudAccountRow {
        account_id: r.get("account_id"),
        project_id: r.get("project_id"),
        owner_id: r.get("owner_id"),
        name: r.get("name"),
        provider: r.get("provider"),
        credentials_enc: r.get("credentials_enc"),
        credentials_nonce: r.get("credentials_nonce"),
        region_default: r.get("region_default"),
        status: r.get("status"),
        last_validated: r.get("last_validated"),
        validation_error: r.get("validation_error"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn create_account(
    client: &Client,
    project_id: &str,
    owner_id: Option<&Uuid>,
    name: &str,
    provider: &str,
    credentials_enc: &[u8],
    credentials_nonce: &[u8],
    region_default: Option<&str>,
) -> anyhow::Result<Uuid> {
    let account_id = Uuid::new_v4();
    let now = Utc::now();
    client
        .execute(
            "INSERT INTO cloud_account \
             (account_id, project_id, owner_id, name, provider, credentials_enc, \
              credentials_nonce, region_default, status, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active', $9, $10)",
            &[
                &account_id,
                &project_id,
                &owner_id,
                &name,
                &provider,
                &credentials_enc,
                &credentials_nonce,
                &region_default,
                &now,
                &now,
            ],
        )
        .await?;
    Ok(account_id)
}

pub async fn update_account(
    client: &Client,
    account_id: &Uuid,
    name: &str,
    region_default: Option<&str>,
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE cloud_account SET name = $1, region_default = $2, updated_at = now() \
             WHERE account_id = $3",
            &[&name, &region_default, account_id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn update_credentials(
    client: &Client,
    account_id: &Uuid,
    credentials_enc: &[u8],
    credentials_nonce: &[u8],
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE cloud_account SET credentials_enc = $1, credentials_nonce = $2, \
             status = 'pending', updated_at = now() WHERE account_id = $3",
            &[&credentials_enc, &credentials_nonce, account_id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn delete_account(
    client: &Client,
    account_id: &Uuid,
    project_id: &str,
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "DELETE FROM cloud_account WHERE account_id = $1 AND project_id = $2",
            &[account_id, &project_id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn update_validation(
    client: &Client,
    account_id: &Uuid,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE cloud_account SET status = $1, validation_error = $2, \
             last_validated = now(), updated_at = now() WHERE account_id = $3",
            &[&status, &error, account_id],
        )
        .await?;
    Ok(())
}
