use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use std::sync::Arc;

use crate::AppState;

async fn list_zones(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::db::zones::SovereigntyZone>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_zones");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let zones = crate::db::zones::list_zones(&client).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list zones");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(zones))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/zones", get(list_zones))
        .with_state(state)
}
