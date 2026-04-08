use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
    Json, Router,
};
use chrono::{Duration, Utc};
use rand::RngExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

// ── Project-scoped endpoints (admin only) ────────────────────────────────

#[derive(Deserialize)]
struct CreateShareLinkRequest {
    resource_type: String,
    resource_id: Uuid,
    label: Option<String>,
    expires_in_days: u32,
}

/// POST /share-links — create a share link
async fn create_share_link(
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
    let payload: CreateShareLinkRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    // Validate resource_type
    if !["run", "job"].contains(&payload.resource_type.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            "resource_type must be 'run' or 'job'",
        )
            .into_response();
    }

    // Validate expires_in_days
    if payload.expires_in_days == 0 || payload.expires_in_days > state.share_max_days {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "expires_in_days must be between 1 and {}",
                state.share_max_days
            ),
        )
            .into_response();
    }

    // Generate 32 random bytes, encode as URL-safe base64 (43 chars)
    let mut raw_bytes = [0u8; 32];
    rand::rng().fill(&mut raw_bytes);
    let raw_token =
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, raw_bytes);

    // SHA-256 hash of the raw token string
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    let token_hash = hex::encode(hasher.finalize());

    let expires_at = Utc::now() + Duration::days(payload.expires_in_days as i64);

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in create_share_link");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::share_links::create_link(
        &client,
        &ctx.project_id,
        &token_hash,
        &payload.resource_type,
        Some(&payload.resource_id),
        payload.label.as_deref(),
        &expires_at,
        &auth_user.user_id,
    )
    .await
    {
        Ok(link_id) => {
            let url = format!("{}/share/{}", state.share_base_url, raw_token);
            Json(serde_json::json!({
                "link_id": link_id.to_string(),
                "url": url,
                "expires_at": expires_at.to_rfc3339(),
                "label": payload.label,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to create share link");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// GET /share-links — list share links
async fn list_share_links(
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
            tracing::error!(error = %e, "DB pool error in list_share_links");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::share_links::list_links(&client, &ctx.project_id).await {
        Ok(links) => Json(links).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list share links");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct UpdateShareLinkRequest {
    action: String,
    expires_in_days: Option<u32>,
}

/// PUT /share-links/:lid — extend or revoke a share link
async fn update_share_link(
    State(state): State<Arc<AppState>>,
    Path((_, link_id)): Path<(String, Uuid)>,
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
    let payload: UpdateShareLinkRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in update_share_link");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match payload.action.as_str() {
        "revoke" => {
            match crate::db::share_links::revoke_link(&client, &link_id, &ctx.project_id).await {
                Ok(()) => Json(serde_json::json!({ "revoked": true })).into_response(),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to revoke share link");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
                }
            }
        }
        "extend" => {
            let days = payload.expires_in_days.unwrap_or(30);
            if days == 0 || days > state.share_max_days {
                return (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "expires_in_days must be between 1 and {}",
                        state.share_max_days
                    ),
                )
                    .into_response();
            }
            let new_expires = Utc::now() + Duration::days(days as i64);
            match crate::db::share_links::extend_link(&client, &link_id, &new_expires).await {
                Ok(()) => Json(
                    serde_json::json!({ "extended": true, "expires_at": new_expires.to_rfc3339() }),
                )
                .into_response(),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to extend share link");
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
                }
            }
        }
        _ => (
            StatusCode::BAD_REQUEST,
            "action must be 'extend' or 'revoke'",
        )
            .into_response(),
    }
}

/// DELETE /share-links/:lid — delete a share link
async fn delete_share_link(
    State(state): State<Arc<AppState>>,
    Path((_, link_id)): Path<(String, Uuid)>,
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
            tracing::error!(error = %e, "DB pool error in delete_share_link");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::share_links::delete_link(&client, &link_id, &ctx.project_id).await {
        Ok(()) => Json(serde_json::json!({ "deleted": true })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to delete share link");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ── Public endpoint (no auth required) ───────────────────────────────────

/// GET /api/share/:token — resolve a share link and return the resource
async fn resolve_share_link(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> impl IntoResponse {
    // SHA-256 hash the raw token
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let token_hash = hex::encode(hasher.finalize());

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in resolve_share_link");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Service unavailable").into_response();
        }
    };

    let link = match crate::db::share_links::resolve_link(&client, &token_hash).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "Link expired or invalid").into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to resolve share link");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    // Fetch the resource based on type
    let resource = match link.resource_type.as_str() {
        "run" => {
            if let Some(ref rid) = link.resource_id {
                match crate::db::runs::get_attempts(&client, rid).await {
                    Ok(data) => data,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to fetch run for share link");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to fetch resource",
                        )
                            .into_response();
                    }
                }
            } else {
                return (StatusCode::NOT_FOUND, "Resource not found").into_response();
            }
        }
        "job" => {
            if let Some(ref rid) = link.resource_id {
                match crate::db::jobs::get(&client, rid).await {
                    Ok(Some(job)) => serde_json::to_value(job).unwrap_or_default(),
                    Ok(None) => {
                        return (StatusCode::NOT_FOUND, "Resource not found").into_response()
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to fetch job for share link");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to fetch resource",
                        )
                            .into_response();
                    }
                }
            } else {
                return (StatusCode::NOT_FOUND, "Resource not found").into_response();
            }
        }
        _ => {
            return (StatusCode::NOT_FOUND, "Unknown resource type").into_response();
        }
    };

    Json(serde_json::json!({
        "resource_type": link.resource_type,
        "resource_id": link.resource_id.map(|id| id.to_string()),
        "label": link.label,
        "data": resource,
        "shared_by": link.created_by_email,
        "expires_at": link.expires_at.to_rfc3339(),
    }))
    .into_response()
}

/// Project-scoped router for share link management (admin only).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/share-links",
            get(list_share_links).post(create_share_link),
        )
        .route(
            "/share-links/:link_id",
            put(update_share_link).delete(delete_share_link),
        )
        .with_state(state)
}

/// Public router for share link resolution (no auth).
pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/share/{token}", get(resolve_share_link))
        .with_state(state)
}
