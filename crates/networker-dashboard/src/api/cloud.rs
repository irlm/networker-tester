use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::ProjectContext;
use crate::AppState;

#[derive(Serialize)]
pub struct ProviderStatus {
    pub available: bool,
    pub authenticated: bool,
    pub account: Option<String>,
}

#[derive(Serialize)]
pub struct CloudStatus {
    pub azure: ProviderStatus,
    pub aws: ProviderStatus,
    pub gcp: ProviderStatus,
    pub ssh: ProviderStatus,
}

/// Check cloud provider status by querying the project's cloud_account table.
/// No server-side CLI checks — the dashboard should be decoupled from host tools.
async fn cloud_status(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<CloudStatus>, StatusCode> {
    let ctx = req
        .extensions()
        .get::<ProjectContext>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in cloud_status");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Query cloud accounts for this project, grouped by provider
    let rows = client
        .query(
            "SELECT provider, name, status, last_validated, validation_error \
             FROM cloud_account WHERE project_id = $1 ORDER BY provider, name",
            &[&ctx.project_id],
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to query cloud accounts");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut azure = ProviderStatus {
        available: false,
        authenticated: false,
        account: None,
    };
    let mut aws = ProviderStatus {
        available: false,
        authenticated: false,
        account: None,
    };
    let mut gcp = ProviderStatus {
        available: false,
        authenticated: false,
        account: None,
    };

    for row in &rows {
        let provider: String = row.get("provider");
        let name: String = row.get("name");
        let status: String = row.get("status");
        let is_active = status == "active";

        let ps = ProviderStatus {
            available: true,
            authenticated: is_active,
            account: Some(name),
        };

        match provider.to_lowercase().as_str() {
            "azure" => azure = ps,
            "aws" => aws = ps,
            "gcp" => gcp = ps,
            _ => {}
        }
    }

    // SSH/LAN is always available (no cloud account needed)
    let ssh = ProviderStatus {
        available: true,
        authenticated: true,
        account: None,
    };

    Ok(Json(CloudStatus {
        azure,
        aws,
        gcp,
        ssh,
    }))
}

/// Project-scoped cloud status — checks cloud_account table for the active project.
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/cloud/status", get(cloud_status))
        .with_state(state)
}
