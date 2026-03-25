use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{ProjectContext, ProjectRole};
use crate::AppState;

/// GET /command-approvals — list pending approvals (project admin).
async fn list_pending(
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
            tracing::error!(error = %e, "DB pool error in list_pending approvals");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::command_approvals::list_pending(&client, &ctx.project_id).await {
        Ok(approvals) => Json(serde_json::json!({ "approvals": approvals })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list pending approvals");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// GET /command-approvals/count — pending approval count (project admin).
async fn pending_count(
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
            tracing::error!(error = %e, "DB pool error in pending_count");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    match crate::db::command_approvals::get_pending_count(&client, &ctx.project_id).await {
        Ok(count) => Json(serde_json::json!({ "count": count })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to get pending count");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct DecideRequest {
    approved: bool,
    reason: Option<String>,
}

/// POST /command-approvals/:aid — approve or deny (project admin).
async fn decide_approval(
    State(state): State<Arc<AppState>>,
    Path((_, approval_id)): Path<(Uuid, Uuid)>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let ctx = match req.extensions().get::<ProjectContext>() {
        Some(c) => c.clone(),
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing project context").into_response()
        }
    };
    let auth = match req.extensions().get::<crate::auth::AuthUser>() {
        Some(a) => a.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    if let Err(status) = crate::auth::require_project_role(&ctx, ProjectRole::Admin) {
        return (status, "Project admin required").into_response();
    }

    // We need to parse body manually since we already consumed req for extensions
    let body: DecideRequest = match axum::body::to_bytes(req.into_body(), 1024 * 16).await {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(b) => b,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid request body").into_response(),
        },
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read body").into_response(),
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in decide_approval");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // Verify approval exists and belongs to this project
    let approval =
        match crate::db::command_approvals::get_approval(&client, &approval_id, &ctx.project_id)
            .await
        {
            Ok(Some(a)) => a,
            Ok(None) => return (StatusCode::NOT_FOUND, "Approval not found").into_response(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get approval");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
            }
        };

    if approval.status != "pending" {
        return (StatusCode::CONFLICT, "Approval is no longer pending").into_response();
    }

    if let Err(e) = crate::db::command_approvals::decide(
        &client,
        &approval_id,
        &ctx.project_id,
        &auth.user_id,
        body.approved,
        body.reason.as_deref(),
    )
    .await
    {
        tracing::error!(error = %e, "Failed to decide approval");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
    }

    // Broadcast SSE event
    let status_str = if body.approved { "approved" } else { "denied" };
    let event = serde_json::json!({
        "approval_id": approval_id,
        "project_id": ctx.project_id,
        "status": status_str,
        "decided_by": auth.user_id,
    });
    let _ = state.approval_tx.send(event.to_string());

    tracing::info!(
        approval_id = %approval_id,
        status = status_str,
        decided_by = %auth.user_id,
        "Command approval decided"
    );

    Json(serde_json::json!({ "status": status_str })).into_response()
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/command-approvals", get(list_pending))
        .route("/command-approvals/count", get(pending_count))
        .route("/command-approvals/:aid", post(decide_approval))
        .with_state(state)
}
