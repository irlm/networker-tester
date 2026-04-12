//! CSV batch import of project members + invite email send/resend.
//!
//! `POST /api/projects/{pid}/members/import` (multipart/form-data, Admin)
//! `POST /api/projects/{pid}/members/send-invites` (JSON, Admin)
//!
//! CSV format: `email,role` (one per line, header row optional).
//! Valid roles: `admin`, `operator`, `viewer`.

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Extension, Json, Router,
};
use base64::Engine;
use chrono::{Duration, Utc};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

#[derive(Serialize)]
struct ImportDetail {
    email: String,
    result: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ImportResponse {
    imported: usize,
    skipped: usize,
    errors: usize,
    details: Vec<ImportDetail>,
}

/// POST /api/projects/{pid}/members/import
async fn import_members(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Extension(auth_user): Extension<AuthUser>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".into()))?;

    let actor_user_id = auth_user.user_id;

    // Find the file field
    let mut csv_text = String::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            csv_text = field
                .text()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("Failed to read file: {e}")))?;
            break;
        }
    }

    if csv_text.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No file field found or file is empty".into(),
        ));
    }

    let client = state.db.get().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {e}"),
        )
    })?;

    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut details = Vec::new();

    for line in csv_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip header row
        if line.eq_ignore_ascii_case("email,role") {
            continue;
        }

        let parts: Vec<&str> = line.splitn(2, ',').collect();
        if parts.len() < 2 {
            errors += 1;
            details.push(ImportDetail {
                email: line.to_string(),
                result: "error",
                message: "Invalid CSV row (expected email,role)".into(),
            });
            continue;
        }

        let email = parts[0].trim();
        let role = parts[1].trim().to_lowercase();

        // Validate email (basic check)
        if !email.contains('@') || email.len() < 3 {
            errors += 1;
            details.push(ImportDetail {
                email: email.to_string(),
                result: "error",
                message: "Invalid email format".into(),
            });
            continue;
        }

        // Validate role
        if !["admin", "operator", "viewer"].contains(&role.as_str()) {
            errors += 1;
            details.push(ImportDetail {
                email: email.to_string(),
                result: "error",
                message: format!("Invalid role '{role}' (must be admin, operator, or viewer)"),
            });
            continue;
        }

        // Create placeholder user if needed
        let user_id = match crate::db::users::create_placeholder_user(&client, email).await {
            Ok(uid) => uid,
            Err(e) => {
                tracing::error!(error = %e, email = %email, "Failed to create placeholder user");
                errors += 1;
                details.push(ImportDetail {
                    email: email.to_string(),
                    result: "error",
                    message: format!("Failed to create user: {e}"),
                });
                continue;
            }
        };

        // Add as pending member
        match crate::db::projects::add_pending_member(
            &client,
            &ctx.project_id,
            &user_id,
            &role,
            &actor_user_id,
        )
        .await
        {
            Ok(crate::db::projects::AddMemberResult::Added) => {
                imported += 1;
                details.push(ImportDetail {
                    email: email.to_string(),
                    result: "invited",
                    message: format!("New user created + invited as {role}"),
                });
            }
            Ok(crate::db::projects::AddMemberResult::AlreadyMember) => {
                skipped += 1;
                details.push(ImportDetail {
                    email: email.to_string(),
                    result: "already_member",
                    message: "Already active member".into(),
                });
            }
            Ok(crate::db::projects::AddMemberResult::AlreadyPending) => {
                skipped += 1;
                details.push(ImportDetail {
                    email: email.to_string(),
                    result: "already_pending",
                    message: "Already has pending invitation".into(),
                });
            }
            Ok(crate::db::projects::AddMemberResult::ReInvited) => {
                imported += 1;
                details.push(ImportDetail {
                    email: email.to_string(),
                    result: "re_invited",
                    message: format!("Re-invited as {role} (was previously denied)"),
                });
            }
            Err(e) => {
                tracing::error!(error = %e, email = %email, "Failed to add pending member");
                errors += 1;
                details.push(ImportDetail {
                    email: email.to_string(),
                    result: "error",
                    message: format!("Failed to add member: {e}"),
                });
            }
        }
    }

    Ok(Json(ImportResponse {
        imported,
        skipped,
        errors,
        details,
    }))
}

// ── Send / resend invites ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct SendInvitesRequest {
    user_ids: Vec<Uuid>,
}

#[derive(Serialize)]
struct InviteDetail {
    user_id: Uuid,
    email: String,
    result: &'static str,
    message: String,
    /// The raw (unencoded) invite token — only populated when email is not configured
    /// so admins can copy the URL manually.
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_url: Option<String>,
}

#[derive(Serialize)]
struct SendInvitesResponse {
    sent: usize,
    skipped: usize,
    errors: usize,
    email_configured: bool,
    details: Vec<InviteDetail>,
}

/// POST /api/projects/{pid}/members/send-invites
///
/// For each user_id in the request body:
///   1. Verify they're a pending_acceptance member of this project.
///   2. Create a workspace_invite row with a fresh token.
///   3. Optionally email the invite link.
///   4. Update project_member.invite_sent_at = NOW().
async fn send_invites(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<SendInvitesRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)
        .map_err(|s| (s, "Admin role required".into()))?;

    let client = state.db.get().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {e}"),
        )
    })?;

    // Detect whether ACS email is configured (both vars must be present).
    let email_configured = std::env::var("DASHBOARD_ACS_CONNECTION_STRING").is_ok()
        && std::env::var("DASHBOARD_ACS_SENDER").is_ok();

    let expires_at = Utc::now() + Duration::days(state.invite_expiry_days as i64);

    let mut sent = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;
    let mut details = Vec::new();

    for user_id in &req.user_ids {
        // 1. Verify this user is a pending_acceptance member of this project.
        let member_row = client
            .query_opt(
                "SELECT pm.status, u.email, pm.role \
                 FROM project_member pm \
                 JOIN dash_user u ON u.user_id = pm.user_id \
                 WHERE pm.project_id = $1 AND pm.user_id = $2",
                &[&ctx.project_id, user_id],
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Database error: {e}"),
                )
            })?;

        let (email, role, status) = match member_row {
            Some(row) => {
                let email: String = row.get("email");
                let role: String = row.get("role");
                let status: String = row.get("status");
                (email, role, status)
            }
            None => {
                errors += 1;
                details.push(InviteDetail {
                    user_id: *user_id,
                    email: String::new(),
                    result: "error",
                    message: "User is not a member of this project".into(),
                    invite_url: None,
                });
                continue;
            }
        };

        if status != "pending_acceptance" {
            skipped += 1;
            details.push(InviteDetail {
                user_id: *user_id,
                email,
                result: "skipped",
                message: format!("Member status is '{status}', not pending_acceptance"),
                invite_url: None,
            });
            continue;
        }

        // 2. Generate token + create workspace_invite row.
        let mut raw_bytes = [0u8; 32];
        rand::rng().fill(&mut raw_bytes);
        let raw_token =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes);
        let token_hash = crate::db::invites::hash_token(&raw_token);

        if let Err(e) = crate::db::invites::create_invite(
            &client,
            &ctx.project_id,
            &email,
            &role,
            &token_hash,
            &auth_user.user_id,
            &expires_at,
        )
        .await
        {
            tracing::error!(error = %e, user_id = %user_id, "Failed to create workspace_invite");
            errors += 1;
            details.push(InviteDetail {
                user_id: *user_id,
                email,
                result: "error",
                message: format!("Failed to create invite: {e}"),
                invite_url: None,
            });
            continue;
        }

        let invite_url = format!("{}/invite/{}", state.public_url, raw_token);

        // 3. Send email if configured.
        if email_configured {
            let body = format!(
                "You have been invited to join a project on AletheDash.\n\n\
                 Click the link below to accept your invitation (valid for {} days):\n\n\
                 {invite_url}\n\n\
                 — AletheDash",
                state.invite_expiry_days
            );
            if let Err(e) =
                crate::email::send_email(&email, "AletheDash — Project Invitation", &body).await
            {
                tracing::warn!(error = %e, email = %email, "Failed to send invite email");
            }
        }

        // 4. Update invite_sent_at.
        if let Err(e) =
            crate::db::projects::update_invite_sent_at(&client, &ctx.project_id, user_id).await
        {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to update invite_sent_at");
        }

        sent += 1;
        details.push(InviteDetail {
            user_id: *user_id,
            email,
            result: "sent",
            message: if email_configured {
                "Invite email sent".into()
            } else {
                "Invite created (email not configured — use invite_url)".into()
            },
            invite_url: if email_configured {
                None
            } else {
                Some(invite_url)
            },
        });
    }

    Ok(Json(SendInvitesResponse {
        sent,
        skipped,
        errors,
        email_configured,
        details,
    }))
}

/// Router for CSV member import and invite send (mounted under project-scoped routes).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/members/import", post(import_members))
        .route("/members/send-invites", post(send_invites))
        .with_state(state)
}
