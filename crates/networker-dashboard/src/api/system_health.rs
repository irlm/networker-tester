use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/system/health", get(get_health))
        .with_state(state)
}

/// GET /api/system/health — admin-only system health overview.
async fn get_health(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Admin-only: prevent information disclosure to non-admin users
    let user = req
        .extensions()
        .get::<AuthUser>()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_platform_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in system health");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let checks = crate::db::system_health::latest_all(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to read health checks");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let core_live = state.db.get().await.is_ok();
    let logs_live = state.logs_db.get().await.is_ok();

    Ok(Json(serde_json::json!({
        "live": {
            "core_db": core_live,
            "logs_db": logs_live,
        },
        "checks": checks,
    })))
}
