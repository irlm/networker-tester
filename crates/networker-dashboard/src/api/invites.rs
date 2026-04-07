use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

// ── Project-scoped endpoints (workspace admin) ──────────────────────────

#[derive(Deserialize)]
struct CreateInviteRequest {
    email: String,
    role: String,
}

/// POST /invites — create a workspace invite
async fn create_invite(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let ctx = match req.extensions().get::<ProjectContext>() {
        Some(c) => c.clone(),
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing project context").into_response()
        }
    };
    let auth_user = match req.extensions().get::<AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    if let Err(status) = crate::auth::require_project_role(&ctx, ProjectRole::Admin) {
        return (status, "Project admin required").into_response();
    }

    let body = match axum::body::to_bytes(req.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid body").into_response(),
    };
    let payload: CreateInviteRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    // Validate role
    if !["admin", "operator", "viewer"].contains(&payload.role.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            "role must be 'admin', 'operator', or 'viewer'",
        )
            .into_response();
    }

    // Validate email (basic check)
    if !payload.email.contains('@') || payload.email.len() < 3 {
        return (StatusCode::BAD_REQUEST, "Invalid email address").into_response();
    }

    // Generate 32 random bytes as base64url token
    let mut raw_bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw_bytes);
    let raw_token =
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, raw_bytes);

    // SHA-256 hash for storage
    let token_hash = crate::db::invites::hash_token(&raw_token);

    let expires_at = Utc::now() + Duration::days(state.invite_expiry_days as i64);

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in create_invite");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::invites::create_invite(
        &client,
        &ctx.project_id,
        &payload.email,
        &payload.role,
        &token_hash,
        &auth_user.user_id,
        &expires_at,
    )
    .await
    {
        Ok(invite_id) => {
            let invite_url = format!("{}/invite/{}", state.public_url, raw_token);

            // Send invite email (best-effort)
            let email_body = format!(
                "You've been invited to join a workspace on AletheDash.\n\n\
                 Click the link below to accept:\n{invite_url}\n\n\
                 This invite expires on {}.",
                expires_at.format("%Y-%m-%d %H:%M UTC")
            );
            if let Err(e) = crate::email::send_email(
                &payload.email,
                "Workspace Invite — AletheDash",
                &email_body,
            )
            .await
            {
                tracing::warn!(error = %e, email = %payload.email, "Failed to send invite email");
            }

            Json(serde_json::json!({
                "invite_id": invite_id.to_string(),
                "url": invite_url,
                "expires_at": expires_at.to_rfc3339(),
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to create invite");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// GET /invites — list invites for the current project
async fn list_invites(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let ctx = match req.extensions().get::<ProjectContext>() {
        Some(c) => c.clone(),
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing project context").into_response()
        }
    };

    if let Err(status) = crate::auth::require_project_role(&ctx, ProjectRole::Admin) {
        return (status, "Project admin required").into_response();
    }

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in list_invites");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::invites::list_invites(&client, &ctx.project_id).await {
        Ok(invites) => Json(invites).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list invites");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// DELETE /invites/:invite_id — revoke a pending invite
async fn revoke_invite(
    State(state): State<Arc<AppState>>,
    Path((_, invite_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let ctx = match req.extensions().get::<ProjectContext>() {
        Some(c) => c.clone(),
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing project context").into_response()
        }
    };

    if let Err(status) = crate::auth::require_project_role(&ctx, ProjectRole::Admin) {
        return (status, "Project admin required").into_response();
    }

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in revoke_invite");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::invites::revoke_invite(&client, &invite_id, &ctx.project_id).await {
        Ok(()) => Json(serde_json::json!({ "revoked": true })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to revoke invite");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Project-scoped router for invite management (admin only).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/invites", get(list_invites).post(create_invite))
        .route("/invites/:invite_id", delete(revoke_invite))
        .with_state(state)
}

// ── Public endpoints (no auth required) ─────────────────────────────────

/// GET /invite/:token — resolve an invite link
async fn resolve_invite(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> impl IntoResponse {
    let token_hash = crate::db::invites::hash_token(&token);

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in resolve_invite");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Service unavailable").into_response();
        }
    };

    match crate::db::invites::resolve_invite(&client, &token_hash).await {
        Ok(Some(invite)) => Json(invite).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Invite expired or invalid").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to resolve invite");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct AcceptInviteRequest {
    /// Password for new account creation (required if no existing account)
    password: Option<String>,
    /// Email override (not used currently, reserved)
    #[allow(dead_code)]
    email: Option<String>,
    /// Current password for existing account (alternative to JWT auth)
    current_password: Option<String>,
}

/// POST /invite/:token/accept — accept an invite
async fn accept_invite(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let token_hash = crate::db::invites::hash_token(&token);

    // Extract optional Authorization header before consuming body
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let body = match axum::body::to_bytes(req.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid body").into_response(),
    };
    let payload: AcceptInviteRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => {
            // Allow empty body (e.g. authenticated user with JWT)
            AcceptInviteRequest {
                password: None,
                email: None,
                current_password: None,
            }
        }
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in accept_invite");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // 1. Resolve invite (must be pending + not expired)
    let invite = match crate::db::invites::resolve_invite(&client, &token_hash).await {
        Ok(Some(inv)) => inv,
        Ok(None) => return (StatusCode::NOT_FOUND, "Invite expired or invalid").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to resolve invite");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    // Resolved user info after authentication / account creation
    struct AcceptedUser {
        user_id: Uuid,
        email: String,
        is_platform_admin: bool,
    }

    let accepted = if invite.has_account {
        // 2. User has an existing account — authenticate them
        // Try JWT first
        let mut result: Option<AcceptedUser> = None;
        if let Some(ref header) = auth_header {
            if let Some(bearer) = header.strip_prefix("Bearer ") {
                if let Ok(claims) = crate::auth::validate_token(bearer, &state.jwt_secret) {
                    if claims.email.to_lowercase() == invite.email.to_lowercase() {
                        result = Some(AcceptedUser {
                            user_id: claims.sub,
                            email: claims.email,
                            is_platform_admin: claims.is_platform_admin,
                        });
                    }
                }
            }
        }

        if result.is_none() {
            // Try current_password auth
            if let Some(ref pwd) = payload.current_password {
                match crate::db::users::authenticate(&client, &invite.email, pwd).await {
                    Ok(Some((uid, email, _role, _mcp, _status, ipa))) => {
                        result = Some(AcceptedUser {
                            user_id: uid,
                            email,
                            is_platform_admin: ipa,
                        });
                    }
                    Ok(None) => {
                        return (StatusCode::UNAUTHORIZED, "Invalid credentials").into_response();
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Auth failed during invite accept");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
                            .into_response();
                    }
                }
            }
        }

        match result {
            Some(u) => u,
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    "Authentication required — provide Authorization header or current_password",
                )
                    .into_response();
            }
        }
    } else {
        // 3. No existing account — create one
        let password = match payload.password {
            Some(ref p) if p.len() >= 8 => p.clone(),
            Some(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "Password must be at least 8 characters",
                )
                    .into_response();
            }
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "password is required to create an account",
                )
                    .into_response();
            }
        };

        match crate::db::users::create_local_user(&client, &invite.email, &password).await {
            Ok(uid) => AcceptedUser {
                user_id: uid,
                email: invite.email.clone(),
                is_platform_admin: false,
            },
            Err(e) => {
                tracing::error!(error = %e, "Failed to create user from invite");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to create account",
                )
                    .into_response();
            }
        }
    };

    // 4. Accept invite in DB
    if let Err(e) =
        crate::db::invites::accept_invite(&client, &invite.invite_id, &accepted.user_id).await
    {
        tracing::error!(error = %e, "Failed to accept invite");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
    }

    // 5. Add user to project
    if let Err(e) = crate::db::projects::add_member(
        &client,
        &invite.project_id,
        &accepted.user_id,
        &invite.role,
        &accepted.user_id,
    )
    .await
    {
        tracing::error!(error = %e, "Failed to add member to project");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
    }

    // 6. Issue JWT
    let jwt = match crate::auth::create_token(
        accepted.user_id,
        &accepted.email,
        &invite.role,
        accepted.is_platform_admin,
        &state.jwt_secret,
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create JWT");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    tracing::info!(
        email = %accepted.email,
        project_id = %invite.project_id,
        role = %invite.role,
        "Invite accepted"
    );

    // 7. Return token + details
    Json(serde_json::json!({
        "token": jwt,
        "email": accepted.email,
        "role": invite.role,
        "project_id": invite.project_id.to_string(),
    }))
    .into_response()
}

/// Public router for invite resolution and acceptance (no auth).
pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/invite/:token", get(resolve_invite))
        .route("/invite/:token/accept", post(accept_invite))
        .with_state(state)
}
