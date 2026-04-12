use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::AppState;

// ── Validation ───────────────────────────────────────────────────────────────

const VALID_PROVIDER_TYPES: &[&str] = &["microsoft", "google", "oidc_generic"];

fn validate_provider_config(
    provider_type: &str,
    issuer_url: Option<&str>,
    tenant_id: Option<&str>,
) -> Result<(), String> {
    if !VALID_PROVIDER_TYPES.contains(&provider_type) {
        return Err(format!(
            "Invalid provider_type '{}'. Valid: {}",
            provider_type,
            VALID_PROVIDER_TYPES.join(", ")
        ));
    }
    if provider_type == "microsoft" {
        match tenant_id {
            None | Some("") => {
                return Err("tenant_id is required for microsoft provider".to_string())
            }
            _ => {}
        }
    }
    if provider_type == "oidc_generic" {
        match issuer_url {
            None | Some("") => {
                return Err("issuer_url is required for oidc_generic provider".to_string())
            }
            Some(url) if !url.starts_with("https://") => {
                return Err("issuer_url must start with https://".to_string())
            }
            _ => {}
        }
    }
    Ok(())
}

// ── Request / Response types ─────────────────────────────────────────────────

#[derive(Serialize)]
struct SsoProviderResponse {
    provider_id: Uuid,
    name: String,
    provider_type: String,
    client_id: String,
    has_client_secret: bool,
    issuer_url: Option<String>,
    tenant_id: Option<String>,
    extra_config: serde_json::Value,
    enabled: bool,
    display_order: i16,
}

impl From<crate::db::sso_providers::SsoProviderRow> for SsoProviderResponse {
    fn from(r: crate::db::sso_providers::SsoProviderRow) -> Self {
        Self {
            provider_id: r.provider_id,
            name: r.name,
            provider_type: r.provider_type,
            client_id: r.client_id,
            has_client_secret: !r.client_secret_enc.is_empty(),
            issuer_url: r.issuer_url,
            tenant_id: r.tenant_id,
            extra_config: r.extra_config,
            enabled: r.enabled,
            display_order: r.display_order,
        }
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    provider_type: String,
    client_id: String,
    client_secret: String,
    issuer_url: Option<String>,
    tenant_id: Option<String>,
    extra_config: Option<serde_json::Value>,
    enabled: Option<bool>,
    display_order: Option<i16>,
}

#[derive(Deserialize)]
struct UpdateBody {
    name: Option<String>,
    provider_type: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    issuer_url: Option<Option<String>>,
    tenant_id: Option<Option<String>>,
    extra_config: Option<serde_json::Value>,
    enabled: Option<bool>,
    display_order: Option<i16>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn extract_admin(req: &axum::extract::Request) -> Result<AuthUser, (StatusCode, String)> {
    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or((StatusCode::UNAUTHORIZED, "Not authenticated".to_string()))?;
    if !user.is_platform_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "Platform admin required".to_string(),
        ));
    }
    Ok(user)
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn list_providers(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<SsoProviderResponse>>, (StatusCode, String)> {
    extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_providers");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    let rows = crate::db::sso_providers::list_all(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list SSO providers");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list providers".to_string(),
            )
        })?;

    Ok(Json(rows.into_iter().map(SsoProviderResponse::from).collect()))
}

async fn create_provider(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<SsoProviderResponse>), (StatusCode, String)> {
    let user = extract_admin(&req)?;

    let key = state.credential_key.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "DASHBOARD_CREDENTIAL_KEY required for SSO provider management".to_string(),
        )
    })?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let create: CreateBody =
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Validate required fields
    if create.name.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name must be non-empty".to_string()));
    }
    if create.client_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "client_id must be non-empty".to_string(),
        ));
    }
    if create.client_secret.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "client_secret must be non-empty".to_string(),
        ));
    }

    validate_provider_config(
        &create.provider_type,
        create.issuer_url.as_deref(),
        create.tenant_id.as_deref(),
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Encrypt the client secret
    let (ciphertext, nonce) =
        crate::crypto::encrypt(create.client_secret.as_bytes(), key).map_err(|e| {
            tracing::error!(error = %e, "Failed to encrypt client secret");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Encryption failed".to_string(),
            )
        })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_provider");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    let provider_id = Uuid::new_v4();
    let row = crate::db::sso_providers::insert(
        &client,
        &provider_id,
        &create.name,
        &create.provider_type,
        &create.client_id,
        &ciphertext,
        &nonce,
        create.issuer_url.as_deref(),
        create.tenant_id.as_deref(),
        &create.extra_config.unwrap_or(serde_json::Value::Object(Default::default())),
        create.enabled.unwrap_or(true),
        create.display_order.unwrap_or(0),
        Some(&user.user_id),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to insert SSO provider");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to create provider".to_string(),
        )
    })?;

    tracing::info!(
        provider_id = %provider_id,
        provider_type = %create.provider_type,
        created_by = %user.email,
        "SSO provider created"
    );

    Ok((StatusCode::CREATED, Json(SsoProviderResponse::from(row))))
}

async fn update_provider(
    State(state): State<Arc<AppState>>,
    Path(provider_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<SsoProviderResponse>, (StatusCode, String)> {
    let user = extract_admin(&req)?;

    let key = state.credential_key.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "DASHBOARD_CREDENTIAL_KEY required for SSO provider management".to_string(),
        )
    })?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let upd: UpdateBody =
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in update_provider");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    // Fetch existing row so we can merge for validation
    let existing = crate::db::sso_providers::get_by_id(&client, &provider_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get SSO provider for update");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, "Provider not found".to_string()))?;

    // Validate non-empty fields if provided
    if let Some(ref name) = upd.name {
        if name.trim().is_empty() {
            return Err((StatusCode::BAD_REQUEST, "name must be non-empty".to_string()));
        }
    }
    if let Some(ref cid) = upd.client_id {
        if cid.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "client_id must be non-empty".to_string(),
            ));
        }
    }

    // Merged values for cross-field validation
    let eff_type = upd.provider_type.as_deref().unwrap_or(&existing.provider_type);
    let eff_issuer = match &upd.issuer_url {
        Some(v) => v.as_deref(),
        None => existing.issuer_url.as_deref(),
    };
    let eff_tenant = match &upd.tenant_id {
        Some(v) => v.as_deref(),
        None => existing.tenant_id.as_deref(),
    };
    validate_provider_config(eff_type, eff_issuer, eff_tenant)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Encrypt new secret if provided
    let (secret_enc, secret_nonce) = if let Some(ref secret) = upd.client_secret {
        if secret.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "client_secret must be non-empty".to_string(),
            ));
        }
        let (ct, n) = crate::crypto::encrypt(secret.as_bytes(), key).map_err(|e| {
            tracing::error!(error = %e, "Failed to encrypt client secret");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Encryption failed".to_string(),
            )
        })?;
        (Some(ct), Some(n.to_vec()))
    } else {
        (None, None)
    };

    let row = crate::db::sso_providers::update(
        &client,
        &provider_id,
        upd.name.as_deref(),
        upd.provider_type.as_deref(),
        upd.client_id.as_deref(),
        secret_enc.as_deref(),
        secret_nonce.as_deref(),
        upd.issuer_url
            .as_ref()
            .map(|v| v.as_deref()),
        upd.tenant_id
            .as_ref()
            .map(|v| v.as_deref()),
        upd.extra_config.as_ref(),
        upd.enabled,
        upd.display_order,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to update SSO provider");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to update provider".to_string(),
        )
    })?
    .ok_or((StatusCode::NOT_FOUND, "Provider not found".to_string()))?;

    tracing::info!(
        provider_id = %provider_id,
        updated_by = %user.email,
        "SSO provider updated"
    );

    Ok(Json(SsoProviderResponse::from(row)))
}

async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Path(provider_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let user = extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_provider");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    let deleted = crate::db::sso_providers::delete(&client, &provider_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete SSO provider");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to delete provider".to_string(),
            )
        })?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "Provider not found".to_string()));
    }

    tracing::info!(
        provider_id = %provider_id,
        deleted_by = %user.email,
        "SSO provider deleted"
    );

    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ── Router ───────────────────────────────────────────────────────────────────

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/admin/sso-providers",
            get(list_providers).post(create_provider),
        )
        .route(
            "/admin/sso-providers/{id}",
            axum::routing::put(update_provider).delete(delete_provider),
        )
        .with_state(state)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rejects_missing_tenant_for_microsoft() {
        let result = validate_provider_config("microsoft", None, None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("tenant_id"),
            "should mention tenant_id"
        );

        let result2 = validate_provider_config("microsoft", None, Some(""));
        assert!(result2.is_err());
    }

    #[test]
    fn validation_rejects_http_issuer_for_oidc() {
        let result = validate_provider_config("oidc_generic", Some("http://example.com"), None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("https://"),
            "should mention https"
        );

        // Missing issuer_url entirely
        let result2 = validate_provider_config("oidc_generic", None, None);
        assert!(result2.is_err());
    }

    #[test]
    fn validation_accepts_valid_google_config() {
        let result = validate_provider_config("google", None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn validation_accepts_valid_microsoft_config() {
        let result =
            validate_provider_config("microsoft", None, Some("contoso.onmicrosoft.com"));
        assert!(result.is_ok());
    }

    #[test]
    fn validation_accepts_valid_oidc_generic_config() {
        let result =
            validate_provider_config("oidc_generic", Some("https://auth.example.com"), None);
        assert!(result.is_ok());
    }

    #[test]
    fn validation_rejects_unknown_provider_type() {
        let result = validate_provider_config("saml", None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid provider_type"));
    }

    #[test]
    fn create_body_deserialization() {
        let json = r#"{
            "name": "Google Workspace",
            "provider_type": "google",
            "client_id": "123.apps.googleusercontent.com",
            "client_secret": "secret-value"
        }"#;
        let body: CreateBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.name, "Google Workspace");
        assert_eq!(body.provider_type, "google");
        assert!(body.issuer_url.is_none());
        assert!(body.enabled.is_none());
        assert!(body.display_order.is_none());
    }

    #[test]
    fn update_body_all_fields_optional() {
        let json = r#"{}"#;
        let body: UpdateBody = serde_json::from_str(json).unwrap();
        assert!(body.name.is_none());
        assert!(body.provider_type.is_none());
        assert!(body.client_id.is_none());
        assert!(body.client_secret.is_none());
        assert!(body.issuer_url.is_none());
        assert!(body.tenant_id.is_none());
        assert!(body.extra_config.is_none());
        assert!(body.enabled.is_none());
        assert!(body.display_order.is_none());
    }

    #[test]
    fn response_redacts_secrets() {
        let row = crate::db::sso_providers::SsoProviderRow {
            provider_id: Uuid::new_v4(),
            name: "Test".to_string(),
            provider_type: "google".to_string(),
            client_id: "cid".to_string(),
            client_secret_enc: vec![1, 2, 3],
            client_secret_nonce: vec![4, 5, 6],
            issuer_url: None,
            tenant_id: None,
            extra_config: serde_json::json!({}),
            enabled: true,
            display_order: 0,
            created_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let resp = SsoProviderResponse::from(row);
        assert!(resp.has_client_secret);
        // The response struct has no secret fields — just has_client_secret bool
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("client_secret_enc").is_none());
        assert!(json.get("client_secret_nonce").is_none());
        assert!(json.get("client_secret").is_none());
    }
}
