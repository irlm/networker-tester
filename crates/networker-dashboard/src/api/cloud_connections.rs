use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_role, AuthUser, ProjectContext, ProjectRole, Role};
use crate::AppState;

const VALID_PROVIDERS: &[&str] = &["azure", "aws", "gcp"];

#[derive(Deserialize)]
pub struct CreateRequest {
    pub name: String,
    pub provider: String,
    pub config: serde_json::Value,
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    pub name: String,
    pub config: serde_json::Value,
}

async fn get_connection(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((_, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<crate::db::cloud_connections::CloudConnectionRow>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_connection");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let conn = crate::db::cloud_connections::get(&client, &id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get cloud connection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    match conn {
        Some(c) => Ok(Json(c)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn update_connection(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((_, id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in update_connection");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let updated = crate::db::cloud_connections::update(&client, &id, &req.name, &req.config)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update cloud connection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if !updated {
        return Err(StatusCode::NOT_FOUND);
    }
    tracing::info!(connection_id = %id, updated_by = %user.email, "Cloud connection updated");
    Ok(Json(serde_json::json!({ "updated": true })))
}

async fn delete_connection(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((_, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_connection");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let deleted = crate::db::cloud_connections::delete(&client, &id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete cloud connection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if !deleted {
        return Err(StatusCode::NOT_FOUND);
    }
    tracing::info!(connection_id = %id, deleted_by = %user.email, "Cloud connection deleted");
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn validate_connection(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((_, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in validate_connection");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let conn = crate::db::cloud_connections::get(&client, &id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get cloud connection for validation");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let conn = match conn {
        Some(c) => c,
        None => return Err(StatusCode::NOT_FOUND),
    };

    tracing::info!(
        connection_id = %id,
        provider = %conn.provider,
        validated_by = %user.email,
        "Validating cloud connection"
    );

    let (status, error) = match conn.provider.as_str() {
        "azure" => validate_azure(&conn.config).await,
        "aws" => validate_aws(&conn.config).await,
        "gcp" => validate_gcp(&conn.config).await,
        _ => ("error".to_string(), Some("Unknown provider".to_string())),
    };

    // Drop the first DB client before acquiring a new one
    drop(client);

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error updating validation status");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    crate::db::cloud_connections::set_status(&client, &id, &status, error.as_deref())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update validation status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "status": status,
        "validation_error": error,
    })))
}

async fn validate_azure(config: &serde_json::Value) -> (String, Option<String>) {
    let subscription_id = config
        .get("subscription_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if subscription_id.is_empty() {
        return (
            "error".to_string(),
            Some("Missing subscription_id in config".to_string()),
        );
    }

    match tokio::process::Command::new("az")
        .args([
            "account",
            "show",
            "--subscription",
            subscription_id,
            "--query",
            "name",
            "-o",
            "tsv",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            tracing::info!(subscription = %subscription_id, account_name = %name, "Azure validation succeeded");
            ("active".to_string(), None)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            tracing::warn!(subscription = %subscription_id, error = %stderr, "Azure validation failed");
            (
                "error".to_string(),
                Some(format!("az account show failed: {stderr}")),
            )
        }
        Err(e) => (
            "error".to_string(),
            Some(format!("Failed to run az CLI: {e}")),
        ),
    }
}

async fn validate_aws(config: &serde_json::Value) -> (String, Option<String>) {
    let role_arn = config
        .get("role_arn")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if role_arn.is_empty() {
        return (
            "error".to_string(),
            Some("Missing role_arn in config. Create an IAM role that trusts Azure AD, then provide its ARN.".to_string()),
        );
    }

    // For v0.14 keep simple: just check if aws CLI is available and role_arn is set.
    // Full STS assume-role-with-web-identity requires the Azure AD token exchange.
    match tokio::process::Command::new("aws")
        .args([
            "sts",
            "get-caller-identity",
            "--query",
            "Account",
            "--output",
            "text",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let account = String::from_utf8_lossy(&output.stdout).trim().to_string();
            tracing::info!(role_arn = %role_arn, account = %account, "AWS validation succeeded (caller identity check)");
            ("active".to_string(), None)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            (
                "error".to_string(),
                Some(format!(
                    "AWS CLI check failed: {stderr}. Ensure the IAM role trust policy is configured for Azure AD federation."
                )),
            )
        }
        Err(_) => (
            "error".to_string(),
            Some("AWS CLI not available. Install aws-cli and configure federation.".to_string()),
        ),
    }
}

async fn validate_gcp(config: &serde_json::Value) -> (String, Option<String>) {
    let project_id = config
        .get("project_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if project_id.is_empty() {
        return (
            "error".to_string(),
            Some("Missing project_id in config".to_string()),
        );
    }

    match tokio::process::Command::new("gcloud")
        .args(["projects", "describe", project_id, "--format=value(projectId)"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            tracing::info!(project_id = %id, "GCP validation succeeded");
            ("active".to_string(), None)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            (
                "error".to_string(),
                Some(format!(
                    "gcloud projects describe failed: {stderr}. Ensure workload identity federation is configured."
                )),
            )
        }
        Err(_) => (
            "error".to_string(),
            Some("gcloud CLI not available. Install Google Cloud SDK and configure workload identity.".to_string()),
        ),
    }
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn list_connections_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::cloud_connections::CloudConnectionRow>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)?;
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = crate::db::cloud_connections::list(&client, &ctx.project_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows))
}

async fn create_connection_scoped(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let create_req: CreateRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !VALID_PROVIDERS.contains(&create_req.provider.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let id = crate::db::cloud_connections::create(
        &client,
        &create_req.name,
        &create_req.provider,
        &create_req.config,
        Some(&user.user_id),
        &ctx.project_id,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "connection_id": id.to_string() })))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/cloud-connections",
            get(list_connections_scoped).post(create_connection_scoped),
        )
        .route(
            "/cloud-connections/:id",
            get(get_connection)
                .put(update_connection)
                .delete(delete_connection),
        )
        .route("/cloud-connections/:id/validate", post(validate_connection))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_providers() {
        assert!(VALID_PROVIDERS.contains(&"azure"));
        assert!(VALID_PROVIDERS.contains(&"aws"));
        assert!(VALID_PROVIDERS.contains(&"gcp"));
        assert_eq!(VALID_PROVIDERS.len(), 3);
    }

    #[test]
    fn invalid_providers_rejected() {
        let invalid = ["Azure", "AWS", "GCP", "digitalocean", "", "lan"];
        for p in &invalid {
            assert!(
                !VALID_PROVIDERS.contains(p),
                "'{p}' should NOT be in the valid providers list"
            );
        }
    }

    #[test]
    fn create_request_deserialization() {
        let json = r#"{"name": "Azure Prod", "provider": "azure", "config": {"subscription_id": "abc-123"}}"#;
        let req: CreateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Azure Prod");
        assert_eq!(req.provider, "azure");
        assert_eq!(
            req.config.get("subscription_id").unwrap().as_str().unwrap(),
            "abc-123"
        );
    }

    #[test]
    fn update_request_deserialization() {
        let json = r#"{"name": "Updated Name", "config": {"subscription_id": "new-id"}}"#;
        let req: UpdateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Updated Name");
    }
}
