use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

/// Extract AuthUser from request extensions and require platform admin.
fn extract_admin(req: &axum::extract::Request) -> Result<AuthUser, StatusCode> {
    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_platform_admin {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(user)
}

#[derive(Serialize)]
struct SystemMetricsResponse {
    system: crate::system_metrics::SystemMetrics,
    database: crate::system_metrics::DbMetrics,
}

async fn system_metrics(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<SystemMetricsResponse>, StatusCode> {
    extract_admin(&req)?;

    let sys =
        std::panic::catch_unwind(crate::system_metrics::collect_system_metrics).map_err(|_| {
            tracing::error!("collect_system_metrics panicked");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in system_metrics");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let db = crate::system_metrics::collect_db_metrics(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to collect DB metrics");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(SystemMetricsResponse {
        system: sys,
        database: db,
    }))
}

async fn workspace_usage(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::system_metrics::WorkspaceUsage>>, StatusCode> {
    extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in workspace_usage");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let usage = crate::system_metrics::collect_workspace_usage(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to collect workspace usage");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(usage))
}

async fn suspend_workspace(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    req: axum::extract::Request,
) -> Result<StatusCode, StatusCode> {
    let user = extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in suspend_workspace");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    crate::db::projects::suspend_project(&client, &project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to suspend project");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(project_id = %project_id, admin = %user.email, "Workspace suspended");
    Ok(StatusCode::OK)
}

async fn restore_workspace(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    req: axum::extract::Request,
) -> Result<StatusCode, StatusCode> {
    let user = extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in restore_workspace");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    crate::db::projects::restore_project(&client, &project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to restore project");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(project_id = %project_id, admin = %user.email, "Workspace restored");
    Ok(StatusCode::OK)
}

#[derive(Serialize)]
struct ProtectResponse {
    delete_protection: bool,
}

async fn protect_workspace(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    req: axum::extract::Request,
) -> Result<Json<ProtectResponse>, StatusCode> {
    let user = extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in protect_workspace");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let new_val = crate::db::projects::toggle_protection(&client, &project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to toggle protection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(
        project_id = %project_id,
        admin = %user.email,
        delete_protection = new_val,
        "Workspace protection toggled"
    );
    Ok(Json(ProtectResponse {
        delete_protection: new_val,
    }))
}

async fn hard_delete_workspace(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    req: axum::extract::Request,
) -> Result<StatusCode, StatusCode> {
    let user = extract_admin(&req)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in hard_delete_workspace");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let project = crate::db::projects::get_project(&client, &project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get project for hard delete");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    match project {
        None => return Err(StatusCode::NOT_FOUND),
        Some(p) if p.deleted_at.is_none() => {
            return Err(StatusCode::BAD_REQUEST);
        }
        _ => {}
    }

    crate::db::projects::hard_delete_project(&client, &project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to hard delete project");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::warn!(
        project_id = %project_id,
        admin = %user.email,
        "Workspace permanently deleted"
    );
    Ok(StatusCode::OK)
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/admin/metrics", get(system_metrics))
        .route("/admin/workspaces", get(workspace_usage))
        .route(
            "/admin/workspaces/{project_id}/suspend",
            post(suspend_workspace),
        )
        .route(
            "/admin/workspaces/{project_id}/restore",
            post(restore_workspace),
        )
        .route(
            "/admin/workspaces/{project_id}/protect",
            post(protect_workspace),
        )
        .route(
            "/admin/workspaces/{project_id}",
            delete(hard_delete_workspace),
        )
        .with_state(state)
}
