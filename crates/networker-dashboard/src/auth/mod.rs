use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

/// Role-based access control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    /// Check if this role has at least the permissions of `required`.
    pub fn has_permission(&self, required: &Role) -> bool {
        matches!(
            (self, required),
            (Role::Admin, _)
                | (Role::Operator, Role::Operator | Role::Viewer)
                | (Role::Viewer, Role::Viewer)
        )
    }
}

/// Check that the authenticated user's role meets the required level.
/// Returns `Ok(())` if permitted, `Err(FORBIDDEN)` otherwise.
pub fn require_role(user: &AuthUser, required: Role) -> Result<(), StatusCode> {
    let user_role: Role = match serde_json::from_value(serde_json::Value::String(user.role.clone()))
    {
        Ok(r) => r,
        Err(_) => {
            tracing::warn!(role = %user.role, user_id = %user.user_id, "Unknown role, defaulting to Viewer");
            Role::Viewer
        }
    };
    if user_role.has_permission(&required) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub email: String,
    pub role: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
}

pub fn create_token(
    user_id: Uuid,
    email: &str,
    role: &str,
    secret: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: user_id,
        email: email.to_string(),
        role: role.to_string(),
        exp: now + 24 * 3600, // 24 hours
        iat: now,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn validate_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(data.claims)
}

/// Axum middleware that requires a valid JWT Bearer token.
/// Reads the JWT secret from AppState and injects AuthUser into request extensions.
/// Enforces must_change_password: only /auth/change-password is allowed when flag is set.
pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return (StatusCode::UNAUTHORIZED, "Missing authorization header").into_response(),
    };
    match validate_token(token, &state.jwt_secret) {
        Ok(claims) => {
            let is_change_password = req.uri().path().ends_with("/auth/change-password");
            let client = match state.db.get().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "DB pool error in auth middleware");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Service temporarily unavailable",
                    )
                        .into_response();
                }
            };

            // Check user status, role, and must_change_password in a single query
            let row = client
                .query_opt(
                    "SELECT status, role, must_change_password FROM dash_user WHERE user_id = $1",
                    &[&claims.sub],
                )
                .await
                .ok()
                .flatten();

            if let Some(ref row) = row {
                let status: String = row.get("status");
                let must_change: bool = row
                    .get::<_, Option<bool>>("must_change_password")
                    .unwrap_or(false);

                let path = req.uri().path().to_string();
                let is_profile = path.ends_with("/auth/profile");
                let is_pending_allowed = is_change_password || is_profile;

                // Pending users: allow only /auth/profile and /auth/change-password
                if status == "pending" && !is_pending_allowed {
                    return (StatusCode::FORBIDDEN, "pending_approval").into_response();
                }

                // Block other non-active users (disabled, denied)
                if status != "active"
                    && status != "pending"
                    && !(is_change_password && must_change)
                {
                    return (StatusCode::FORBIDDEN, "Account is not active").into_response();
                }

                // Enforce must_change_password
                if !is_change_password && must_change && status == "active" {
                    return (
                        StatusCode::FORBIDDEN,
                        "Password change required before accessing this resource",
                    )
                        .into_response();
                }
            }

            // Use DB role when available, fall back to JWT claim
            let db_role = row.as_ref().map(|r| r.get::<_, String>("role"));
            req.extensions_mut().insert(AuthUser {
                user_id: claims.sub,
                email: claims.email,
                role: db_role.unwrap_or(claims.role),
            });
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "Invalid token").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_all_permissions() {
        assert!(Role::Admin.has_permission(&Role::Admin));
        assert!(Role::Admin.has_permission(&Role::Operator));
        assert!(Role::Admin.has_permission(&Role::Viewer));
    }

    #[test]
    fn operator_cannot_admin() {
        assert!(!Role::Operator.has_permission(&Role::Admin));
        assert!(Role::Operator.has_permission(&Role::Operator));
        assert!(Role::Operator.has_permission(&Role::Viewer));
    }

    #[test]
    fn viewer_is_read_only() {
        assert!(!Role::Viewer.has_permission(&Role::Admin));
        assert!(!Role::Viewer.has_permission(&Role::Operator));
        assert!(Role::Viewer.has_permission(&Role::Viewer));
    }
}
