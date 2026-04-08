use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

/// GET /api/projects/{pid}/members — list project members (project admin).
async fn list_members(
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
            tracing::error!(error = %e, "DB pool error in list_members");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::list_members(&client, &ctx.project_id).await {
        Ok(members) => Json(serde_json::json!({ "members": members })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list project members");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    email: String,
    role: String,
}

/// POST /api/projects/{pid}/members — add a member (project admin).
async fn add_member(
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
    let payload: AddMemberRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    // Validate role
    if !["admin", "operator", "viewer"].contains(&payload.role.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            "Role must be admin, operator, or viewer",
        )
            .into_response();
    }

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in add_member");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // Look up user by email
    let target_user = match crate::db::users::find_by_email(&client, &payload.email).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "User not found with that email").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to look up user by email");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    match crate::db::projects::add_member(
        &client,
        &ctx.project_id,
        &target_user.user_id,
        &payload.role,
        &auth_user.user_id,
    )
    .await
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "success": true })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to add project member");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateMemberRoleRequest {
    role: String,
}

/// PUT /api/projects/{pid}/members/:uid — update member role (project admin).
async fn update_member_role(
    State(state): State<Arc<AppState>>,
    Path((_, member_id)): Path<(String, Uuid)>,
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

    let body = match axum::body::to_bytes(req.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid body").into_response(),
    };
    let payload: UpdateMemberRoleRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    if !["admin", "operator", "viewer"].contains(&payload.role.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            "Role must be admin, operator, or viewer",
        )
            .into_response();
    }

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in update_member_role");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::update_member_role(
        &client,
        &ctx.project_id,
        &member_id,
        &payload.role,
    )
    .await
    {
        Ok(Ok(true)) => Json(serde_json::json!({ "success": true })).into_response(),
        Ok(Ok(false)) => (StatusCode::NOT_FOUND, "Member not found").into_response(),
        Ok(Err(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to update member role");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// DELETE /api/projects/{pid}/members/:uid — remove member (project admin).
async fn remove_member(
    State(state): State<Arc<AppState>>,
    Path((_, member_id)): Path<(String, Uuid)>,
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
            tracing::error!(error = %e, "DB pool error in remove_member");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::remove_member(&client, &ctx.project_id, &member_id).await {
        Ok(Ok(())) => Json(serde_json::json!({ "success": true })).into_response(),
        Ok(Err(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to remove member");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Router for project member endpoints (mounted under project-scoped routes).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/members", get(list_members).post(add_member))
        .route(
            "/members/:member_id",
            put(update_member_role).delete(remove_member),
        )
        .with_state(state)
}
