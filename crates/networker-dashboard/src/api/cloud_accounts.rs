use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

const VALID_PROVIDERS: &[&str] = &["azure", "aws", "gcp"];

#[derive(Deserialize)]
pub struct CreateAccountRequest {
    pub name: String,
    pub provider: String,
    pub credentials: serde_json::Value,
    pub region_default: Option<String>,
    #[serde(default)]
    pub personal: bool,
}

#[derive(Deserialize)]
pub struct UpdateAccountRequest {
    pub name: String,
    pub region_default: Option<String>,
}

#[derive(Serialize)]
struct AccountResponse {
    account_id: Uuid,
    name: String,
    provider: String,
    region_default: Option<String>,
    personal: bool,
    status: String,
    last_validated: Option<chrono::DateTime<chrono::Utc>>,
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn list_accounts(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::cloud_accounts::CloudAccountSummary>>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_accounts");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;
    let rows = crate::db::cloud_accounts::list_accounts(&client, &ctx.project_id, &user.user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list cloud accounts");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list accounts".to_string(),
            )
        })?;
    Ok(Json(rows))
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();

    // Check that credential encryption is configured
    let key = state.credential_key.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Cloud accounts require DASHBOARD_CREDENTIAL_KEY to be set".to_string(),
        )
    })?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let create_req: CreateAccountRequest =
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    if !VALID_PROVIDERS.contains(&create_req.provider.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Invalid provider '{}'. Valid: {}",
                create_req.provider,
                VALID_PROVIDERS.join(", ")
            ),
        ));
    }

    // Authorization: personal accounts require Operator+, shared accounts require Admin
    let owner_id = if create_req.personal {
        crate::auth::require_project_role(&ctx, ProjectRole::Operator).map_err(|s| {
            (
                s,
                "Operator role required for personal accounts".to_string(),
            )
        })?;
        Some(user.user_id)
    } else {
        crate::auth::require_project_role(&ctx, ProjectRole::Admin)
            .map_err(|s| (s, "Admin role required for shared accounts".to_string()))?;
        None
    };

    // Serialize credentials to JSON bytes and encrypt
    let cred_bytes = serde_json::to_vec(&create_req.credentials)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid credentials: {e}")))?;
    let (ciphertext, nonce) = crate::crypto::encrypt(&cred_bytes, key).map_err(|e| {
        tracing::error!(error = %e, "Failed to encrypt credentials");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Encryption failed".to_string(),
        )
    })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    let account_id = crate::db::cloud_accounts::create_account(
        &client,
        &ctx.project_id,
        owner_id.as_ref(),
        &create_req.name,
        &create_req.provider,
        &ciphertext,
        &nonce,
        create_req.region_default.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create cloud account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to create account".to_string(),
        )
    })?;

    tracing::info!(
        account_id = %account_id,
        provider = %create_req.provider,
        personal = create_req.personal,
        created_by = %user.email,
        "Cloud account created"
    );

    Ok(Json(serde_json::json!({
        "account_id": account_id.to_string(),
        "name": create_req.name,
        "provider": create_req.provider,
    })))
}

async fn get_account(
    State(state): State<Arc<AppState>>,
    Path((_project_id, account_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<AccountResponse>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Viewer)
        .map_err(|s| (s, "Insufficient permissions".to_string()))?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;
    let row = crate::db::cloud_accounts::get_account(&client, &account_id, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get cloud account");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get account".to_string(),
            )
        })?;

    match row {
        Some(acct) => Ok(Json(AccountResponse {
            account_id: acct.account_id,
            name: acct.name,
            provider: acct.provider,
            region_default: acct.region_default,
            personal: acct.owner_id.is_some(),
            status: acct.status,
            last_validated: acct.last_validated,
        })),
        None => Err((StatusCode::NOT_FOUND, "Account not found".to_string())),
    }
}

async fn update_account(
    State(state): State<Arc<AppState>>,
    Path((_project_id, account_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in update_account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    // Fetch existing account to check ownership
    let acct = crate::db::cloud_accounts::get_account(&client, &account_id, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get cloud account for update");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Account not found".to_string()))?;

    // Authorization: admin for shared, owner for personal
    if acct.owner_id.is_some() {
        // Personal account: owner or admin can update
        if acct.owner_id != Some(user.user_id) {
            crate::auth::require_project_role(&ctx, ProjectRole::Admin).map_err(|s| {
                (
                    s,
                    "Only the owner or admin can update this account".to_string(),
                )
            })?;
        }
    } else {
        // Shared account: admin only
        crate::auth::require_project_role(&ctx, ProjectRole::Admin)
            .map_err(|s| (s, "Admin role required for shared accounts".to_string()))?;
    }

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".to_string()))?;
    let update_req: UpdateAccountRequest =
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let updated = crate::db::cloud_accounts::update_account(
        &client,
        &account_id,
        &update_req.name,
        update_req.region_default.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to update cloud account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to update account".to_string(),
        )
    })?;

    if !updated {
        return Err((StatusCode::NOT_FOUND, "Account not found".to_string()));
    }

    tracing::info!(
        account_id = %account_id,
        updated_by = %user.email,
        "Cloud account updated"
    );
    Ok(Json(serde_json::json!({ "updated": true })))
}

async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path((_project_id, account_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    // Fetch existing account to check ownership
    let acct = crate::db::cloud_accounts::get_account(&client, &account_id, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get cloud account for delete");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Account not found".to_string()))?;

    // Authorization: admin for shared, owner for personal
    if acct.owner_id.is_some() {
        if acct.owner_id != Some(user.user_id) {
            crate::auth::require_project_role(&ctx, ProjectRole::Admin).map_err(|s| {
                (
                    s,
                    "Only the owner or admin can delete this account".to_string(),
                )
            })?;
        }
    } else {
        crate::auth::require_project_role(&ctx, ProjectRole::Admin)
            .map_err(|s| (s, "Admin role required for shared accounts".to_string()))?;
    }

    let deleted = crate::db::cloud_accounts::delete_account(&client, &account_id, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete cloud account");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to delete account".to_string(),
            )
        })?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "Account not found".to_string()));
    }

    tracing::info!(
        account_id = %account_id,
        deleted_by = %user.email,
        "Cloud account deleted"
    );
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn validate_account(
    State(state): State<Arc<AppState>>,
    Path((_project_id, account_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)
        .map_err(|s| (s, "Operator role required to validate accounts".to_string()))?;

    let key = state.credential_key.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Cloud accounts require DASHBOARD_CREDENTIAL_KEY to be set".to_string(),
        )
    })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in validate_account");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    let acct = crate::db::cloud_accounts::get_account(&client, &account_id, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get cloud account for validation");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Account not found".to_string()))?;

    // Decrypt credentials
    let nonce: [u8; 12] = acct.credentials_nonce.as_slice().try_into().map_err(|_| {
        tracing::error!(account_id = %account_id, "Invalid nonce length in DB");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid stored nonce".to_string(),
        )
    })?;

    let plaintext = crate::crypto::decrypt_with_fallback(
        &acct.credentials_enc,
        &nonce,
        key,
        state.credential_key_old.as_ref(),
    )
    .map_err(|e| {
        tracing::error!(error = %e, account_id = %account_id, "Failed to decrypt credentials");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to decrypt credentials".to_string(),
        )
    })?;

    let _credentials: serde_json::Value = serde_json::from_slice(&plaintext).map_err(|e| {
        tracing::error!(error = %e, account_id = %account_id, "Decrypted credentials are not valid JSON");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Corrupted credentials".to_string(),
        )
    })?;

    tracing::info!(
        account_id = %account_id,
        provider = %acct.provider,
        validated_by = %user.email,
        "Validating cloud account credentials"
    );

    // For now, mark as validated if decryption succeeded.
    // Actual provider-specific validation (az account show, aws sts, gcloud) can be added later.
    let (status, error): (&str, Option<&str>) = ("active", None);

    crate::db::cloud_accounts::update_validation(&client, &account_id, status, error)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update validation status");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to update validation".to_string(),
            )
        })?;

    Ok(Json(serde_json::json!({
        "status": status,
        "validation_error": error,
    })))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/cloud-accounts", get(list_accounts).post(create_account))
        .route(
            "/cloud-accounts/{aid}",
            get(get_account).put(update_account).delete(delete_account),
        )
        .route("/cloud-accounts/{aid}/validate", post(validate_account))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_providers_list() {
        assert!(VALID_PROVIDERS.contains(&"azure"));
        assert!(VALID_PROVIDERS.contains(&"aws"));
        assert!(VALID_PROVIDERS.contains(&"gcp"));
        assert_eq!(VALID_PROVIDERS.len(), 3);
    }

    #[test]
    fn create_request_deserialization() {
        let json = r#"{
            "name": "AWS Prod",
            "provider": "aws",
            "credentials": {"access_key_id": "AKIA...", "secret_access_key": "xxx"},
            "region_default": "us-east-1",
            "personal": true
        }"#;
        let req: CreateAccountRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "AWS Prod");
        assert_eq!(req.provider, "aws");
        assert!(req.personal);
        assert_eq!(req.region_default.as_deref(), Some("us-east-1"));
    }

    #[test]
    fn create_request_defaults() {
        let json = r#"{
            "name": "Azure Dev",
            "provider": "azure",
            "credentials": {"subscription_id": "abc-123"}
        }"#;
        let req: CreateAccountRequest = serde_json::from_str(json).unwrap();
        assert!(!req.personal);
        assert!(req.region_default.is_none());
    }

    #[test]
    fn update_request_deserialization() {
        let json = r#"{"name": "Updated Name", "region_default": "eu-west-1"}"#;
        let req: UpdateAccountRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Updated Name");
        assert_eq!(req.region_default.as_deref(), Some("eu-west-1"));
    }
}
