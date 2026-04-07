use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

use crate::AppState;

/// The Default project UUID (legacy V010 constant, kept for migration/tests).
#[allow(dead_code)]
pub const DEFAULT_PROJECT_UUID: Uuid = Uuid::from_bytes([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
]);

/// The default project's 14-char base36 ID, set during migration.
/// Falls back to a deterministic generation if not yet set by the migration.
static DEFAULT_PROJECT_ID_CELL: OnceLock<String> = OnceLock::new();

/// Return the default project ID as a 14-char base36 string.
#[allow(dead_code)]
pub fn default_project_id() -> &'static str {
    DEFAULT_PROJECT_ID_CELL.get_or_init(|| {
        // 1767225600 = 2026-01-01T00:00:00Z (PROJECT_EPOCH), used as fallback
        // for the Default project which was created at migration time.
        crate::project_id::ProjectId::generate_deterministic("us", "a20", 1767225600)
            .as_str()
            .to_string()
    })
}

/// Set the default project ID from the database (called during migration).
pub fn set_default_project_id(id: String) {
    let _ = DEFAULT_PROJECT_ID_CELL.set(id);
}

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

/// Project-scoped role (separate from the global `Role` enum).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectRole {
    Viewer,
    Operator,
    Admin,
}

impl ProjectRole {
    pub fn has_permission(&self, required: &ProjectRole) -> bool {
        self >= required
    }
}

/// Context injected by `require_project` middleware.
#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub project_id: String,
    #[allow(dead_code)] // Used in later PRs for slug-based URL routing
    pub project_slug: String,
    pub role: ProjectRole,
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
    #[serde(default)]
    pub is_platform_admin: bool,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
    pub is_platform_admin: bool,
}

pub fn create_token(
    user_id: Uuid,
    email: &str,
    role: &str,
    is_platform_admin: bool,
    secret: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: user_id,
        email: email.to_string(),
        role: role.to_string(),
        is_platform_admin,
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

            // Check user status, role, must_change_password, and is_platform_admin in a single query
            let row = client
                .query_opt(
                    "SELECT status, role, must_change_password, is_platform_admin FROM dash_user WHERE user_id = $1",
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
                if status != "active" && status != "pending" && !(is_change_password && must_change)
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
            let db_is_platform_admin = row
                .as_ref()
                .and_then(|r| r.get::<_, Option<bool>>("is_platform_admin"))
                .unwrap_or(claims.is_platform_admin);
            req.extensions_mut().insert(AuthUser {
                user_id: claims.sub,
                email: claims.email,
                role: db_role.unwrap_or(claims.role),
                is_platform_admin: db_is_platform_admin,
            });
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "Invalid token").into_response(),
    }
}

/// Check that the `ProjectContext` role meets the required level.
pub fn require_project_role(ctx: &ProjectContext, required: ProjectRole) -> Result<(), StatusCode> {
    if ctx.role.has_permission(&required) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Axum middleware that resolves a project from the `:project_id` path segment
/// and checks that the authenticated user has access. Inserts `ProjectContext`
/// into request extensions.
pub async fn require_project(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    // 1. Extract AuthUser (require_auth must run first)
    let auth_user = match req.extensions().get::<AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    // 2. Extract :project_id from the path.
    //    Use OriginalUri to get the full path before axum nest() strips the prefix.
    let path = req
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|u| u.0.path().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let project_id = match extract_project_id_from_path(&path) {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Missing or invalid project_id in path",
            )
                .into_response()
        }
    };

    // 3. Fetch project from DB
    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in require_project");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Service temporarily unavailable",
            )
                .into_response();
        }
    };

    let project_row = match crate::db::projects::get_project(&client, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, "Project not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch project");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    // 4. Check soft-delete (platform admins bypass for admin operations)
    if project_row.deleted_at.is_some() && !auth_user.is_platform_admin {
        return (StatusCode::FORBIDDEN, "Workspace suspended").into_response();
    }

    // 5. Platform admins get implicit Admin access
    let role = if auth_user.is_platform_admin {
        ProjectRole::Admin
    } else {
        // 6. Check project_member table
        match crate::db::projects::get_member_role(&client, &project_id, &auth_user.user_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return (StatusCode::FORBIDDEN, "Not a member of this project").into_response()
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to check project membership");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
            }
        }
    };

    // 7. Insert ProjectContext
    req.extensions_mut().insert(ProjectContext {
        project_id,
        project_slug: project_row.slug,
        role,
    });

    next.run(req).await
}

/// Extract the project_id segment that follows "projects/" in the request path.
fn extract_project_id_from_path(path: &str) -> Option<String> {
    let segments: Vec<&str> = path.split('/').collect();
    for (i, seg) in segments.iter().enumerate() {
        if *seg == "projects" {
            if let Some(next) = segments.get(i + 1) {
                if !next.is_empty() {
                    return Some((*next).to_string());
                }
            }
        }
    }
    None
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
            is_platform_admin: false,
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
        let token = create_token(
            Uuid::new_v4(),
            "user@example.com",
            "admin",
            false,
            TEST_SECRET,
        )
        .unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn create_and_validate_roundtrip() {
        let uid = Uuid::new_v4();
        let token = create_token(uid, "alice@test.com", "operator", false, TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.sub, uid);
        assert_eq!(claims.email, "alice@test.com");
        assert_eq!(claims.role, "operator");
    }

    #[test]
    fn validate_token_rejects_wrong_secret() {
        let token = create_token(Uuid::new_v4(), "a@b.com", "viewer", false, TEST_SECRET).unwrap();
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
        let token = create_token(Uuid::new_v4(), "a@b.com", "admin", false, TEST_SECRET).unwrap();
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
        let token = create_token(Uuid::new_v4(), "a@b.com", "admin", false, TEST_SECRET).unwrap();
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
            is_platform_admin: false,
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
        let token = create_token(Uuid::new_v4(), email, "viewer", false, TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.email, email);
    }

    #[test]
    fn different_users_get_different_tokens() {
        let t1 = create_token(Uuid::new_v4(), "a@b.com", "admin", false, TEST_SECRET).unwrap();
        let t2 = create_token(Uuid::new_v4(), "c@d.com", "viewer", false, TEST_SECRET).unwrap();
        assert_ne!(t1, t2);
    }

    // ── ProjectRole ordering / has_permission ────────────────────────────

    #[test]
    fn project_role_admin_has_all_permissions() {
        assert!(ProjectRole::Admin.has_permission(&ProjectRole::Admin));
        assert!(ProjectRole::Admin.has_permission(&ProjectRole::Operator));
        assert!(ProjectRole::Admin.has_permission(&ProjectRole::Viewer));
    }

    #[test]
    fn project_role_operator_cannot_admin() {
        assert!(!ProjectRole::Operator.has_permission(&ProjectRole::Admin));
        assert!(ProjectRole::Operator.has_permission(&ProjectRole::Operator));
        assert!(ProjectRole::Operator.has_permission(&ProjectRole::Viewer));
    }

    #[test]
    fn project_role_viewer_is_read_only() {
        assert!(!ProjectRole::Viewer.has_permission(&ProjectRole::Admin));
        assert!(!ProjectRole::Viewer.has_permission(&ProjectRole::Operator));
        assert!(ProjectRole::Viewer.has_permission(&ProjectRole::Viewer));
    }

    #[test]
    fn require_project_role_respects_hierarchy() {
        let ctx = ProjectContext {
            project_id: "test00000000x0".to_string(),
            project_slug: "test".into(),
            role: ProjectRole::Operator,
        };
        assert!(require_project_role(&ctx, ProjectRole::Viewer).is_ok());
        assert!(require_project_role(&ctx, ProjectRole::Operator).is_ok());
        assert!(require_project_role(&ctx, ProjectRole::Admin).is_err());
    }

    // ── extract_project_id_from_path ─────────────────────────────────────

    #[test]
    fn extract_project_id_valid_path() {
        let path = "/api/projects/us12345abcde00/members";
        assert_eq!(
            extract_project_id_from_path(path),
            Some("us12345abcde00".to_string())
        );
    }

    #[test]
    fn extract_project_id_no_projects_segment() {
        assert_eq!(extract_project_id_from_path("/api/jobs/123"), None);
    }

    #[test]
    fn extract_project_id_empty_segment() {
        assert_eq!(extract_project_id_from_path("/api/projects//members"), None);
    }

    // ── is_platform_admin in JWT roundtrip ───────────────────────────────

    #[test]
    fn create_token_with_platform_admin_roundtrips() {
        let uid = Uuid::new_v4();
        let token = create_token(uid, "admin@test.com", "admin", true, TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert!(claims.is_platform_admin);
    }

    #[test]
    fn create_token_without_platform_admin_roundtrips() {
        let uid = Uuid::new_v4();
        let token = create_token(uid, "user@test.com", "viewer", false, TEST_SECRET).unwrap();
        let claims = validate_token(&token, TEST_SECRET).unwrap();
        assert!(!claims.is_platform_admin);
    }
}
