use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

// ── List / Create (top-level, no project context needed) ─────────────────

/// GET /api/projects — list projects visible to the authenticated user.
async fn list_projects(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let auth_user = match req.extensions().get::<AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in list_projects");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::list_user_projects(
        &client,
        &auth_user.user_id,
        auth_user.is_platform_admin,
    )
    .await
    {
        Ok(projects) => Json(serde_json::json!({ "projects": projects })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list projects");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    name: String,
    description: Option<String>,
    slug: Option<String>,
}

/// POST /api/projects — create a new project (platform admin only).
async fn create_project(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let auth_user = match req.extensions().get::<AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    if !auth_user.is_platform_admin {
        return (StatusCode::FORBIDDEN, "Platform admin required").into_response();
    }

    let body = match axum::body::to_bytes(req.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid body").into_response(),
    };
    let payload: CreateProjectRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    if payload.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "Project name is required").into_response();
    }

    let slug = payload
        .slug
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::db::projects::slugify(&payload.name));

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in create_project");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::create_project(
        &client,
        &payload.name,
        &slug,
        payload.description.as_deref(),
        &auth_user.user_id,
    )
    .await
    {
        Ok(project) => (StatusCode::CREATED, Json(serde_json::json!(project))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to create project");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Router for list + create (mounted under protected routes, no project context).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .with_state(state)
}

// ── Detail routes (require project context middleware) ────────────────────

/// GET /api/projects/:project_id — get project details.
async fn get_project_detail(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let ctx = match req.extensions().get::<ProjectContext>() {
        Some(c) => c.clone(),
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing project context").into_response()
        }
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in get_project_detail");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::get_project(&client, &ctx.project_id).await {
        Ok(Some(project)) => Json(serde_json::json!({
            "project_id": project.project_id,
            "name": project.name,
            "slug": project.slug,
            "description": project.description,
            "created_by": project.created_by,
            "created_at": project.created_at,
            "updated_at": project.updated_at,
            "settings": project.settings,
            "role": ctx.role,
        }))
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Project not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get project");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateProjectRequest {
    name: Option<String>,
    description: Option<String>,
    settings: Option<serde_json::Value>,
}

/// PUT /api/projects/:project_id — update project (project admin).
async fn update_project_detail(
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

    let body = match axum::body::to_bytes(req.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid body").into_response(),
    };
    let payload: UpdateProjectRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in update_project");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::update_project(
        &client,
        &ctx.project_id,
        payload.name.as_deref(),
        payload.description.as_deref(),
        payload.settings.as_ref(),
    )
    .await
    {
        Ok(true) => Json(serde_json::json!({ "success": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "Project not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to update project");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// DELETE /api/projects/:project_id — delete project (platform admin only).
async fn delete_project_detail(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let auth_user = match req.extensions().get::<AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    if !auth_user.is_platform_admin {
        return (StatusCode::FORBIDDEN, "Platform admin required").into_response();
    }

    let ctx = match req.extensions().get::<ProjectContext>() {
        Some(c) => c.clone(),
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing project context").into_response()
        }
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in delete_project");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::projects::delete_project(&client, &ctx.project_id).await {
        Ok(Ok(())) => Json(serde_json::json!({ "success": true })).into_response(),
        Ok(Err(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to delete project");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Detail router (mounted under project-scoped routes with require_project middleware).
pub fn detail_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/",
            get(get_project_detail)
                .put(update_project_detail)
                .delete(delete_project_detail),
        )
        .with_state(state)
}
