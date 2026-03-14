use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
struct AgentListResponse {
    agents: Vec<crate::db::agents::AgentRow>,
}

async fn list_agents(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AgentListResponse>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let agents = crate::db::agents::list(&client)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(AgentListResponse { agents }))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/agents", get(list_agents))
        .with_state(state)
}
