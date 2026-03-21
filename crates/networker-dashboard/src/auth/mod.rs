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

    // ── Role::has_permission ─────────────────────────────────────────────

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

    // ── Role serde ───────────────────────────────────────────────────────

    #[test]
    fn role_serializes_to_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        assert_eq!(
            serde_json::to_string(&Role::Operator).unwrap(),
            "\"operator\""
        );
        assert_eq!(serde_json::to_string(&Role::Viewer).unwrap(), "\"viewer\"");
    }

    #[test]
    fn role_deserializes_from_lowercase() {
        let admin: Role = serde_json::from_str("\"admin\"").unwrap();
        assert_eq!(admin, Role::Admin);
        let op: Role = serde_json::from_str("\"operator\"").unwrap();
        assert_eq!(op, Role::Operator);
        let v: Role = serde_json::from_str("\"viewer\"").unwrap();
        assert_eq!(v, Role::Viewer);
    }

    #[test]
    fn role_rejects_unknown_string() {
        let result: Result<Role, _> = serde_json::from_str("\"superadmin\"");
        assert!(result.is_err());
    }

    #[test]
    fn role_rejects_capitalized() {
        // rename_all = "lowercase" means "Admin" (capitalized) is invalid JSON input
        let result: Result<Role, _> = serde_json::from_str("\"Admin\"");
        assert!(result.is_err());
    }

    // ── require_role ─────────────────────────────────────────────────────

    fn make_user(role: &str) -> AuthUser {
        AuthUser {
            user_id: Uuid::new_v4(),
            email: "test@example.com".into(),
            role: role.into(),
        }
    }

    #[test]
    fn require_role_admin_passes_all_checks() {
        let user = make_user("admin");
        assert!(require_role(&user, Role::Admin).is_ok());
        assert!(require_role(&user, Role::Operator).is_ok());
        assert!(require_role(&user, Role::Viewer).is_ok());
    }

    #[test]
    fn require_role_operator_blocked_from_admin() {
        let user = make_user("operator");
        assert_eq!(
            require_role(&user, Role::Admin).unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert!(require_role(&user, Role::Operator).is_ok());
    }

    #[test]
    fn require_role_viewer_blocked_from_operator_and_admin() {
        let user = make_user("viewer");
        assert_eq!(
            require_role(&user, Role::Admin).unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            require_role(&user, Role::Operator).unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert!(require_role(&user, Role::Viewer).is_ok());
    }

    #[test]
    fn require_role_unknown_role_defaults_to_viewer() {
        // Unknown role should default to Viewer (fail closed)
        let user = make_user("superadmin");
        assert_eq!(
            require_role(&user, Role::Admin).unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            require_role(&user, Role::Operator).unwrap_err(),
            StatusCode::FORBIDDEN
        );
        // Viewer should still pass since unknown defaults to Viewer
        assert!(require_role(&user, Role::Viewer).is_ok());
    }

    #[test]
    fn require_role_empty_role_defaults_to_viewer() {
        let user = make_user("");
        assert_eq!(
            require_role(&user, Role::Admin).unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert!(require_role(&user, Role::Viewer).is_ok());
    }

    // ── JWT token create / validate ──────────────────────────────────────

    const TEST_SECRET: &str = "test-secret-at-least-32-bytes-long!!";

    #[test]
    fn create_token_returns_nonempty_string() {
        let token = create_token(Uuid::new_v4(), "user@example.com", "admin", TEST_SECRET).unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn create_and_validate_roundtrip() {
        let uid = Uuid::new_v4();
        let token = create_token(uid, "alice@test.com", "operator", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.sub, uid);
        assert_eq!(claims.email, "alice@test.com");
        assert_eq!(claims.role, "operator");
    }

    #[test]
    fn validate_token_rejects_wrong_secret() {
        let token = create_token(Uuid::new_v4(), "a@b.com", "viewer", TEST_SECRET).unwrap();
        let result = validate_token(&token, "wrong-secret-xxxxxxxxxxxxxxxxxxx");
        assert!(result.is_err());
    }

    #[test]
    fn validate_token_rejects_garbage() {
        let result = validate_token("not.a.jwt", TEST_SECRET);
        assert!(result.is_err());
    }

    #[test]
    fn validate_token_rejects_empty_string() {
        let result = validate_token("", TEST_SECRET);
        assert!(result.is_err());
    }

    #[test]
    fn token_expiry_is_24_hours_from_now() {
        let token = create_token(Uuid::new_v4(), "a@b.com", "admin", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        let now = chrono::Utc::now().timestamp() as usize;
        // exp should be roughly now + 86400 (24h), allow 5s tolerance
        let diff = claims.exp.abs_diff(now + 86400);
        assert!(
            diff < 5,
            "Token expiry should be ~24h from now, diff={diff}s"
        );
    }

    #[test]
    fn token_iat_is_recent() {
        let token = create_token(Uuid::new_v4(), "a@b.com", "admin", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        let now = chrono::Utc::now().timestamp() as usize;
        assert!(
            now.abs_diff(claims.iat) < 5,
            "iat should be within 5s of now"
        );
    }

    #[test]
    fn validate_token_rejects_expired_token() {
        // Manually craft an expired token
        let now = chrono::Utc::now().timestamp() as usize;
        let claims = Claims {
            sub: Uuid::new_v4(),
            email: "expired@test.com".into(),
            role: "admin".into(),
            exp: now - 3600, // expired 1 hour ago
            iat: now - 7200,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();
        let result = validate_token(&token, TEST_SECRET);
        assert!(result.is_err(), "Expired token should be rejected");
    }

    #[test]
    fn create_token_preserves_special_chars_in_email() {
        let email = "user+tag@sub.domain.co.uk";
        let token = create_token(Uuid::new_v4(), email, "viewer", TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.email, email);
    }

    #[test]
    fn different_users_get_different_tokens() {
        let t1 = create_token(Uuid::new_v4(), "a@b.com", "admin", TEST_SECRET).unwrap();
        let t2 = create_token(Uuid::new_v4(), "c@d.com", "viewer", TEST_SECRET).unwrap();
        assert_ne!(t1, t2);
    }
}
