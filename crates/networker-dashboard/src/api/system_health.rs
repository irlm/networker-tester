use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/system/health", get(get_health))
        .with_state(state)
}

/// Router for public (no-auth) health endpoint.
pub fn public_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health_check))
        .with_state(state)
}

// ── GET /api/health — public ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub db: String,
    pub logs_db: String,
    pub uptime_secs: u64,
}

/// GET /api/health — public, no auth required.
///
/// Returns HTTP 200 with JSON body indicating component health.
/// Returns HTTP 503 only if the main DB is unreachable.
pub async fn health_check(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<HealthResponse>) {
    let uptime_secs = state.started_at.elapsed().as_secs();

    // Test main DB with SELECT 1
    let db_status = match state.db.get().await {
        Ok(client) => match client.query_one("SELECT 1", &[]).await {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("error: {e}"),
        },
        Err(e) => format!("error: {e}"),
    };

    // Test logs DB with SELECT 1
    let logs_db_status = match state.logs_db.get().await {
        Ok(client) => match client.query_one("SELECT 1", &[]).await {
            Ok(_) => {
                // If logs_db URL matches db URL, it's a fallback (degraded)
                if state.logs_database_url == state.database_url {
                    "degraded".to_string()
                } else {
                    "ok".to_string()
                }
            }
            Err(e) => format!("error: {e}"),
        },
        Err(e) => format!("error: {e}"),
    };

    let main_db_ok = db_status == "ok";

    let (http_status, overall) = if main_db_ok {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "degraded")
    };

    (
        http_status,
        Json(HealthResponse {
            status: overall,
            version: env!("CARGO_PKG_VERSION"),
            db: db_status,
            logs_db: logs_db_status,
            uptime_secs,
        }),
    )
}

// ── GET /api/system/health — admin-only internal health overview ───────────────

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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn test_health_check_version() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty(), "CARGO_PKG_VERSION must not be empty");
        // Version should look like semver (at least one dot)
        assert!(version.contains('.'), "version should be semver-like: {version}");
    }
}
