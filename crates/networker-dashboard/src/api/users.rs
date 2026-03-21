use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
    Extension, Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_role, AuthUser, Role};
use crate::AppState;

const VALID_ROLES: &[&str] = &["admin", "operator", "viewer"];

async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<crate::db::users::UserRow>>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_users");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let users = crate::db::users::list_users(&client).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list users");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(users))
}

async fn list_pending(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_pending");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let users = crate::db::users::list_pending(&client).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list pending users");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let count = users.len();
    Ok(Json(serde_json::json!({
        "users": users,
        "count": count,
    })))
}

#[derive(Deserialize)]
pub struct ApproveRequest {
    pub role: String,
}

async fn approve_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<ApproveRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;

    // Validate role value
    if !VALID_ROLES.contains(&req.role.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in approve_user");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let approved = crate::db::users::approve_user(&client, &user_id, &req.role)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to approve user");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !approved {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(user_id = %user_id, role = %req.role, approved_by = %user.email, "User approved");
    Ok(Json(serde_json::json!({ "approved": true })))
}

async fn deny_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in deny_user");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let denied = crate::db::users::deny_user(&client, &user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to deny user");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !denied {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(user_id = %user_id, denied_by = %user.email, "User denied");
    Ok(Json(serde_json::json!({ "denied": true })))
}

#[derive(Deserialize)]
pub struct SetRoleRequest {
    pub role: String,
}

async fn set_role(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<SetRoleRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;

    if !VALID_ROLES.contains(&req.role.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Prevent admin from demoting themselves
    if user.user_id == user_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in set_role");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let updated = crate::db::users::set_role(&client, &user_id, &req.role)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to set user role");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !updated {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(user_id = %user_id, new_role = %req.role, changed_by = %user.email, "User role updated");
    Ok(Json(serde_json::json!({ "updated": true })))
}

async fn disable_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_role(&user, Role::Admin)?;

    // Prevent admin from disabling themselves
    if user.user_id == user_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in disable_user");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let disabled = crate::db::users::disable_user(&client, &user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to disable user");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !disabled {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::info!(user_id = %user_id, disabled_by = %user.email, "User disabled");
    Ok(Json(serde_json::json!({ "disabled": true })))
}

#[derive(Deserialize)]
pub struct InviteRequest {
    pub email: String,
    pub role: String,
}

async fn invite_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<InviteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, &'static str)> {
    require_role(&user, Role::Admin).map_err(|s| (s, "Admin access required"))?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in invite_user");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?;

    match crate::db::users::invite_user(&client, &req.email, &req.role).await {
        Ok(Ok((user_id, token))) => {
            tracing::info!(
                invited_email = %req.email,
                role = %req.role,
                invited_by = %user.email,
                "User invited"
            );

            // Send invite email (or log in dev mode)
            let setup_url = format!("{}/reset-password?token={token}", state.public_url);
            let body = format!(
                "Hi,\n\n\
                 You've been invited to the Networker Dashboard as {role}.\n\n\
                 Click the link below to set your password (valid for 24 hours):\n\n\
                 {setup_url}\n\n\
                 — Networker Dashboard",
                role = req.role,
            );
            if let Err(e) = crate::email::send_email(
                &req.email,
                "Networker Dashboard — You're Invited",
                &body,
            )
            .await
            {
                tracing::warn!(error = %e, email = %req.email, "Failed to send invite email");
            }

            Ok(Json(
                serde_json::json!({ "user_id": user_id.to_string() }),
            ))
        }
        Ok(Err("Email already registered")) => Err((StatusCode::CONFLICT, "Email already registered")),
        Ok(Err(msg)) => Err((StatusCode::BAD_REQUEST, msg)),
        Err(e) => {
            tracing::error!(error = %e, "Failed to invite user");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/pending", get(list_pending))
        .route("/users/invite", post(invite_user))
        .route("/users/:user_id/approve", post(approve_user))
        .route("/users/:user_id/deny", post(deny_user))
        .route("/users/:user_id/role", put(set_role))
        .route("/users/:user_id/disable", post(disable_user))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validate the role whitelist used in approve_user and set_role.
    mod role_validation {
        use super::super::VALID_ROLES;

        #[test]
        fn whitelist_contains_expected_roles() {
            assert!(VALID_ROLES.contains(&"admin"));
            assert!(VALID_ROLES.contains(&"operator"));
            assert!(VALID_ROLES.contains(&"viewer"));
            assert_eq!(VALID_ROLES.len(), 3, "unexpected extra roles in whitelist");
        }

        #[test]
        fn invalid_roles_rejected() {
            let invalid = ["Admin", "ADMIN", "superadmin", "root", "", " ", "moderator"];
            for role in &invalid {
                assert!(
                    !VALID_ROLES.contains(role),
                    "'{role}' should NOT be in the valid roles list"
                );
            }
        }
    }

    /// ApproveRequest deserialization.
    mod approve_request {
        use super::ApproveRequest;

        #[test]
        fn deserializes_valid_request() {
            let json = r#"{"role": "operator"}"#;
            let req: ApproveRequest = serde_json::from_str(json).unwrap();
            assert_eq!(req.role, "operator");
        }

        #[test]
        fn rejects_missing_role() {
            let json = "{}";
            let result: Result<ApproveRequest, _> = serde_json::from_str(json);
            assert!(result.is_err());
        }
    }

    /// SetRoleRequest deserialization.
    mod set_role_request {
        use super::SetRoleRequest;

        #[test]
        fn deserializes_valid_request() {
            let json = r#"{"role": "admin"}"#;
            let req: SetRoleRequest = serde_json::from_str(json).unwrap();
            assert_eq!(req.role, "admin");
        }

        #[test]
        fn rejects_missing_role() {
            let json = "{}";
            let result: Result<SetRoleRequest, _> = serde_json::from_str(json);
            assert!(result.is_err());
        }
    }
}
