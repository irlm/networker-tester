use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

/// Metadata about a single benchmark API token in Key Vault.
/// Secret values are never returned.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenInfo {
    name: String,
    config_id: String,
    testbed_id: String,
    created: Option<String>,
    expires: Option<String>,
    enabled: bool,
    user: Option<String>,
    project_id: Option<String>,
}

/// Extract AuthUser from request extensions (any authenticated user).
fn extract_user(req: &axum::extract::Request) -> Result<AuthUser, StatusCode> {
    req.extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)
}

/// Extract AuthUser from request extensions and require platform admin.
fn extract_admin(req: &axum::extract::Request) -> Result<AuthUser, StatusCode> {
    let user = extract_user(req)?;
    if !user.is_platform_admin {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(user)
}

/// Return the Key Vault name from env, or `None` if unset.
fn vault_name() -> Option<String> {
    std::env::var("BENCH_KEYVAULT_NAME")
        .ok()
        .filter(|v| !v.is_empty())
}

/// Parse a secret name like `bench-{config_id}-vm-{testbed_id}` into
/// `(config_id, testbed_id)`. Returns `None` if the format doesn't match.
fn parse_token_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("bench-")?;
    let idx = rest.find("-vm-")?;
    let config_id = &rest[..idx];
    let testbed_id = &rest[idx + 4..];
    if config_id.is_empty() || testbed_id.is_empty() {
        return None;
    }
    Some((config_id.to_string(), testbed_id.to_string()))
}

/// GET /api/bench-tokens -- list all active benchmark tokens from Key Vault.
/// Returns metadata only (never secret values).
async fn list_tokens(
    State(_state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<TokenInfo>>, StatusCode> {
    let user = extract_user(&req)?;

    let vault = match vault_name() {
        Some(v) => v,
        None => {
            // Mock mode for local dev: return sample tokens when BENCH_MOCK_TOKENS=1
            if std::env::var("BENCH_MOCK_TOKENS").unwrap_or_default() == "1" {
                let all = mock_tokens();
                let filtered = filter_tokens_for_user(all, &user);
                return Ok(Json(filtered));
            }
            return Ok(Json(vec![]));
        }
    };

    let output = tokio::process::Command::new("az")
        .args([
            "keyvault",
            "secret",
            "list",
            "--vault-name",
            &vault,
            "--query",
            "[?starts_with(name,'bench-')]",
            "-o",
            "json",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to spawn az keyvault secret list");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(stderr = %stderr, "az keyvault secret list failed");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let items: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).map_err(|e| {
        tracing::error!(error = %e, "Failed to parse az keyvault secret list output");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let all_tokens: Vec<TokenInfo> = items
        .into_iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let (config_id, testbed_id) = parse_token_name(&name)?;
            let attrs = item.get("attributes");
            let created = attrs
                .and_then(|a| a.get("created"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let expires = attrs
                .and_then(|a| a.get("expires"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let enabled = attrs
                .and_then(|a| a.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Parse user and project_id from Key Vault tags
            let tags = item.get("tags");
            let token_user = tags
                .and_then(|t| t.get("user"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let token_project_id = tags
                .and_then(|t| t.get("project"))
                .and_then(|v| v.as_str())
                .map(String::from);
            Some(TokenInfo {
                name,
                config_id,
                testbed_id,
                created,
                expires,
                enabled,
                user: token_user,
                project_id: token_project_id,
            })
        })
        .collect();

    let filtered = filter_tokens_for_user(all_tokens, &user);
    Ok(Json(filtered))
}

/// DELETE /api/bench-tokens/{name} -- revoke a single benchmark token.
async fn revoke_token(
    State(_state): State<Arc<AppState>>,
    Path(name): Path<String>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = extract_user(&req)?;

    // Prevent arbitrary secret deletion -- name must start with "bench-"
    if !name.starts_with("bench-") {
        tracing::warn!(
            name = %name,
            admin = %user.email,
            "Rejected token revocation: name does not start with bench-"
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    let vault = vault_name().ok_or_else(|| {
        tracing::error!("BENCH_KEYVAULT_NAME not set, cannot revoke token");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Non-admin users must own the token to revoke it
    if !user.is_platform_admin {
        let show_output = tokio::process::Command::new("az")
            .args([
                "keyvault",
                "secret",
                "show",
                "--vault-name",
                &vault,
                "--name",
                &name,
                "-o",
                "json",
            ])
            .output()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to check token ownership");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        if show_output.status.success() {
            let secret: serde_json::Value = serde_json::from_slice(&show_output.stdout)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let tag_user = secret
                .get("tags")
                .and_then(|t| t.get("user"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if tag_user != user.user_id.to_string() && tag_user != user.email {
                tracing::warn!(
                    name = %name,
                    user = %user.email,
                    tag_user = %tag_user,
                    "User attempted to revoke token they do not own"
                );
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    let output = tokio::process::Command::new("az")
        .args([
            "keyvault",
            "secret",
            "delete",
            "--vault-name",
            &vault,
            "--name",
            &name,
            "-o",
            "json",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to spawn az keyvault secret delete");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(name = %name, stderr = %stderr, "az keyvault secret delete failed");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    tracing::info!(name = %name, admin = %user.email, "Benchmark token revoked");
    Ok(Json(serde_json::json!({
        "status": "revoked",
        "name": name,
    })))
}

/// DELETE /api/bench-tokens -- revoke ALL benchmark tokens (emergency).
async fn revoke_all(
    State(_state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = extract_admin(&req)?;

    let vault = vault_name().ok_or_else(|| {
        tracing::error!("BENCH_KEYVAULT_NAME not set, cannot revoke tokens");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // List all bench-* secrets first
    let list_output = tokio::process::Command::new("az")
        .args([
            "keyvault",
            "secret",
            "list",
            "--vault-name",
            &vault,
            "--query",
            "[?starts_with(name,'bench-')].name",
            "-o",
            "json",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to spawn az keyvault secret list");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !list_output.status.success() {
        let stderr = String::from_utf8_lossy(&list_output.stderr);
        tracing::error!(stderr = %stderr, "az keyvault secret list failed during revoke_all");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let names: Vec<String> = serde_json::from_slice(&list_output.stdout).map_err(|e| {
        tracing::error!(error = %e, "Failed to parse secret names");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let total = names.len();
    let mut revoked = 0u32;
    let mut errors = 0u32;

    for name in &names {
        let output = tokio::process::Command::new("az")
            .args([
                "keyvault",
                "secret",
                "delete",
                "--vault-name",
                &vault,
                "--name",
                name,
            ])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => {
                revoked += 1;
                tracing::info!(name = %name, admin = %user.email, "Benchmark token revoked");
            }
            Ok(o) => {
                errors += 1;
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::error!(name = %name, stderr = %stderr, "Failed to delete secret");
            }
            Err(e) => {
                errors += 1;
                tracing::error!(name = %name, error = %e, "Failed to spawn az command");
            }
        }
    }

    tracing::warn!(
        admin = %user.email,
        total = total,
        revoked = revoked,
        errors = errors,
        "Bulk benchmark token revocation completed"
    );

    Ok(Json(serde_json::json!({
        "status": "completed",
        "total": total,
        "revoked": revoked,
        "errors": errors,
    })))
}

/// Filter tokens based on user role: admins see all, others see only their own.
fn filter_tokens_for_user(tokens: Vec<TokenInfo>, user: &AuthUser) -> Vec<TokenInfo> {
    if user.is_platform_admin {
        return tokens;
    }
    let uid = user.user_id.to_string();
    let email = &user.email;
    tokens
        .into_iter()
        .filter(|t| {
            t.user.as_deref() == Some(uid.as_str()) || t.user.as_deref() == Some(email.as_str())
        })
        .collect()
}

/// Mock tokens for local development (when BENCH_MOCK_TOKENS=1).
fn mock_tokens() -> Vec<TokenInfo> {
    let now = chrono::Utc::now();
    vec![
        TokenInfo {
            name: "bench-c4da3bda-vm-7b75a519".to_string(),
            config_id: "c4da3bda".to_string(),
            testbed_id: "7b75a519".to_string(),
            created: Some((now - chrono::Duration::hours(2)).to_rfc3339()),
            expires: Some((now + chrono::Duration::hours(2)).to_rfc3339()),
            enabled: true,
            user: Some("admin@localhost".to_string()),
            project_id: Some("benchmark-test".to_string()),
        },
        TokenInfo {
            name: "bench-a1b2c3d4-vm-eastus-01".to_string(),
            config_id: "a1b2c3d4".to_string(),
            testbed_id: "eastus-01".to_string(),
            created: Some((now - chrono::Duration::hours(5)).to_rfc3339()),
            expires: Some((now - chrono::Duration::hours(1)).to_rfc3339()),
            enabled: false,
            user: Some("admin@localhost".to_string()),
            project_id: Some("benchmark-test".to_string()),
        },
        TokenInfo {
            name: "bench-e5f6g7h8-vm-westus-02".to_string(),
            config_id: "e5f6g7h8".to_string(),
            testbed_id: "westus-02".to_string(),
            created: Some((now - chrono::Duration::minutes(30)).to_rfc3339()),
            expires: Some(
                (now + chrono::Duration::hours(3) + chrono::Duration::minutes(30)).to_rfc3339(),
            ),
            enabled: true,
            user: Some("dev@example.com".to_string()),
            project_id: Some("perf-testing".to_string()),
        },
        TokenInfo {
            name: "bench-i9j0k1l2-vm-eu-west-1".to_string(),
            config_id: "i9j0k1l2".to_string(),
            testbed_id: "eu-west-1".to_string(),
            created: Some((now - chrono::Duration::minutes(10)).to_rfc3339()),
            expires: Some((now + chrono::Duration::minutes(50)).to_rfc3339()),
            enabled: true,
            user: Some("admin@localhost".to_string()),
            project_id: Some("benchmark-test".to_string()),
        },
    ]
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/bench-tokens", get(list_tokens).delete(revoke_all))
        .route("/bench-tokens/{name}", delete(revoke_token))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_token_name() {
        let (config, testbed) = parse_token_name("bench-abc123-vm-east-us-1").unwrap();
        assert_eq!(config, "abc123");
        assert_eq!(testbed, "east-us-1");
    }

    #[test]
    fn parse_token_name_with_hyphens() {
        let (config, testbed) = parse_token_name("bench-my-config-vm-my-testbed").unwrap();
        assert_eq!(config, "my-config");
        assert_eq!(testbed, "my-testbed");
    }

    #[test]
    fn parse_invalid_prefix() {
        assert!(parse_token_name("notbench-abc-vm-def").is_none());
    }

    #[test]
    fn parse_missing_vm_separator() {
        assert!(parse_token_name("bench-abc-def").is_none());
    }

    #[test]
    fn parse_empty_parts() {
        assert!(parse_token_name("bench--vm-def").is_none());
        assert!(parse_token_name("bench-abc-vm-").is_none());
    }
}
