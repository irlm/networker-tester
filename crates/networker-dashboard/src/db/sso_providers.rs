use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct SsoProviderRow {
    pub provider_id: Uuid,
    pub name: String,
    pub provider_type: String,
    pub client_id: String,
    pub client_secret_enc: Vec<u8>,
    pub client_secret_nonce: Vec<u8>,
    pub issuer_url: Option<String>,
    pub tenant_id: Option<String>,
    pub extra_config: serde_json::Value,
    pub enabled: bool,
    pub display_order: i16,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn row_to_provider(r: &tokio_postgres::Row) -> SsoProviderRow {
    SsoProviderRow {
        provider_id: r.get("provider_id"),
        name: r.get("name"),
        provider_type: r.get("provider_type"),
        client_id: r.get("client_id"),
        client_secret_enc: r.get("client_secret_enc"),
        client_secret_nonce: r.get("client_secret_nonce"),
        issuer_url: r.get("issuer_url"),
        tenant_id: r.get("tenant_id"),
        extra_config: r.get("extra_config"),
        enabled: r.get("enabled"),
        display_order: r.get("display_order"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

/// List all SSO providers, ordered by display_order then created_at.
pub async fn list_all(client: &Client) -> anyhow::Result<Vec<SsoProviderRow>> {
    let rows = client
        .query(
            "SELECT provider_id, name, provider_type, client_id, \
                    client_secret_enc, client_secret_nonce, issuer_url, tenant_id, \
                    extra_config, enabled, display_order, created_by, created_at, updated_at \
             FROM sso_provider \
             ORDER BY display_order, created_at",
            &[],
        )
        .await?;
    Ok(rows.iter().map(row_to_provider).collect())
}

/// List only enabled SSO providers, ordered by display_order then created_at.
/// Used by the SSO login flow and provider cache.
pub async fn list_enabled(client: &Client) -> anyhow::Result<Vec<SsoProviderRow>> {
    let rows = client
        .query(
            "SELECT provider_id, name, provider_type, client_id, \
                    client_secret_enc, client_secret_nonce, issuer_url, tenant_id, \
                    extra_config, enabled, display_order, created_by, created_at, updated_at \
             FROM sso_provider \
             WHERE enabled = TRUE \
             ORDER BY display_order, created_at",
            &[],
        )
        .await?;
    Ok(rows.iter().map(row_to_provider).collect())
}

/// Get a single SSO provider by ID.
pub async fn get_by_id(
    client: &Client,
    provider_id: &Uuid,
) -> anyhow::Result<Option<SsoProviderRow>> {
    let row = client
        .query_opt(
            "SELECT provider_id, name, provider_type, client_id, \
                    client_secret_enc, client_secret_nonce, issuer_url, tenant_id, \
                    extra_config, enabled, display_order, created_by, created_at, updated_at \
             FROM sso_provider \
             WHERE provider_id = $1",
            &[provider_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_provider))
}

/// Insert a new SSO provider and return the full row.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    client: &Client,
    provider_id: &Uuid,
    name: &str,
    provider_type: &str,
    client_id: &str,
    client_secret_enc: &[u8],
    client_secret_nonce: &[u8],
    issuer_url: Option<&str>,
    tenant_id: Option<&str>,
    extra_config: &serde_json::Value,
    enabled: bool,
    display_order: i16,
    created_by: Option<&Uuid>,
) -> anyhow::Result<SsoProviderRow> {
    let now = Utc::now();
    let row = client
        .query_one(
            "INSERT INTO sso_provider \
             (provider_id, name, provider_type, client_id, client_secret_enc, \
              client_secret_nonce, issuer_url, tenant_id, extra_config, enabled, \
              display_order, created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
             RETURNING provider_id, name, provider_type, client_id, \
                       client_secret_enc, client_secret_nonce, issuer_url, tenant_id, \
                       extra_config, enabled, display_order, created_by, created_at, updated_at",
            &[
                provider_id,
                &name,
                &provider_type,
                &client_id,
                &client_secret_enc,
                &client_secret_nonce,
                &issuer_url,
                &tenant_id,
                extra_config,
                &enabled,
                &display_order,
                &created_by,
                &now,
                &now,
            ],
        )
        .await?;
    Ok(row_to_provider(&row))
}

/// Update an SSO provider. Returns the updated row, or None if not found.
///
/// `client_secret_enc` / `client_secret_nonce` are `Option` — if `None`, the
/// existing encrypted secret is preserved.
#[allow(clippy::too_many_arguments)]
pub async fn update(
    client: &Client,
    provider_id: &Uuid,
    name: Option<&str>,
    provider_type: Option<&str>,
    client_id_val: Option<&str>,
    client_secret_enc: Option<&[u8]>,
    client_secret_nonce: Option<&[u8]>,
    issuer_url: Option<Option<&str>>,
    tenant_id: Option<Option<&str>>,
    extra_config: Option<&serde_json::Value>,
    enabled: Option<bool>,
    display_order: Option<i16>,
) -> anyhow::Result<Option<SsoProviderRow>> {
    // Build a dynamic UPDATE. We always set updated_at.
    let mut sets = vec!["updated_at = now()".to_string()];
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
    let mut idx = 1usize;

    macro_rules! push_set {
        ($col:expr, $val:expr) => {
            if let Some(v) = $val {
                sets.push(format!("{} = ${}", $col, idx));
                params.push(Box::new(v));
                idx += 1;
            }
        };
    }

    push_set!("name", name.map(|s| s.to_string()));
    push_set!("provider_type", provider_type.map(|s| s.to_string()));
    push_set!("client_id", client_id_val.map(|s| s.to_string()));
    push_set!("client_secret_enc", client_secret_enc.map(|b| b.to_vec()));
    push_set!(
        "client_secret_nonce",
        client_secret_nonce.map(|b| b.to_vec())
    );
    push_set!("enabled", enabled);
    push_set!("display_order", display_order);
    push_set!("extra_config", extra_config.cloned());

    // issuer_url and tenant_id use Option<Option<&str>> so callers can
    // explicitly set them to NULL vs leaving them unchanged.
    if let Some(v) = issuer_url {
        sets.push(format!("issuer_url = ${idx}"));
        params.push(Box::new(v.map(|s| s.to_string())));
        idx += 1;
    }
    if let Some(v) = tenant_id {
        sets.push(format!("tenant_id = ${idx}"));
        params.push(Box::new(v.map(|s| s.to_string())));
        idx += 1;
    }

    // provider_id is the final param for the WHERE clause
    sets.push(String::new()); // placeholder — not used
    let _ = sets.pop(); // remove placeholder

    let sql = format!(
        "UPDATE sso_provider SET {} WHERE provider_id = ${idx} \
         RETURNING provider_id, name, provider_type, client_id, \
                   client_secret_enc, client_secret_nonce, issuer_url, tenant_id, \
                   extra_config, enabled, display_order, created_by, created_at, updated_at",
        sets.join(", ")
    );
    params.push(Box::new(*provider_id));

    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|p| p.as_ref() as _).collect();

    let row = client.query_opt(&sql, &param_refs).await?;
    Ok(row.as_ref().map(row_to_provider))
}

/// Delete an SSO provider. Returns true if a row was deleted.
pub async fn delete(client: &Client, provider_id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "DELETE FROM sso_provider WHERE provider_id = $1",
            &[provider_id],
        )
        .await?;
    Ok(n > 0)
}
