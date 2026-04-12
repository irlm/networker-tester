use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
    Extension, Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

#[derive(Serialize)]
struct PendingProject {
    project_id: String,
    project_name: String,
    role: String,
    invited_by_email: Option<String>,
    invited_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct PendingProjectsResponse {
    pending: Vec<PendingProject>,
}

/// GET /api/me/pending-projects — list pending project memberships for the authenticated user.
async fn list_pending_projects(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<PendingProjectsResponse>, (StatusCode, String)> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_pending_projects");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into())
    })?;

    let rows = client
        .query(
            "SELECT pm.project_id, p.name, pm.role, u.email AS invited_by_email, pm.joined_at \
             FROM project_member pm \
             JOIN project p ON p.project_id = pm.project_id \
             LEFT JOIN dash_user u ON u.user_id = pm.invited_by \
             WHERE pm.user_id = $1 AND pm.status = 'pending_acceptance' \
             ORDER BY pm.joined_at DESC",
            &[&user.user_id],
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to query pending projects");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error".into())
        })?;

    let pending = rows
        .into_iter()
        .map(|row| PendingProject {
            project_id: row.get("project_id"),
            project_name: row.get("name"),
            role: row.get("role"),
            invited_by_email: row.get("invited_by_email"),
            invited_at: row.get("joined_at"),
        })
        .collect();

    Ok(Json(PendingProjectsResponse { pending }))
}

/// PUT /api/projects/{pid}/members/me/accept — accept a pending membership.
async fn accept_membership(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in accept_membership");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into())
    })?;

    let accepted =
        crate::db::projects::update_member_status(&client, &project_id, &user.user_id, "active")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to accept membership");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error".into())
            })?;

    if !accepted {
        return Err((
            StatusCode::NOT_FOUND,
            "No pending membership found".into(),
        ));
    }

    tracing::info!(
        user_id = %user.user_id,
        project_id = %project_id,
        "User accepted project membership"
    );
    Ok(Json(serde_json::json!({ "accepted": true })))
}

/// PUT /api/projects/{pid}/members/me/deny — deny a pending membership.
async fn deny_membership(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in deny_membership");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into())
    })?;

    let denied =
        crate::db::projects::update_member_status(&client, &project_id, &user.user_id, "denied")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to deny membership");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error".into())
            })?;

    if !denied {
        return Err((
            StatusCode::NOT_FOUND,
            "No pending membership found".into(),
        ));
    }

    tracing::info!(
        user_id = %user.user_id,
        project_id = %project_id,
        "User denied project membership"
    );
    Ok(Json(serde_json::json!({ "denied": true })))
}

/// Router for /api/me/pending-projects (mounted in protected_flat).
pub fn me_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/me/pending-projects", get(list_pending_projects))
        .with_state(state)
}

/// Router for project accept/deny endpoints (mounted in protected_flat under /projects/{pid}).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/projects/{project_id}/members/me/accept",
            put(accept_membership),
        )
        .route(
            "/projects/{project_id}/members/me/deny",
            put(deny_membership),
        )
        .with_state(state)
}
