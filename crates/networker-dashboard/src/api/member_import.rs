//! CSV batch import of project members.
//!
//! `POST /api/projects/{pid}/members/import` (multipart/form-data, Admin)
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
use serde::Serialize;
use std::sync::Arc;

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

/// Router for CSV member import (mounted under project-scoped routes).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/members/import", post(import_members))
        .with_state(state)
}
