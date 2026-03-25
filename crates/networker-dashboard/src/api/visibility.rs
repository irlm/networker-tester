use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

#[derive(Deserialize)]
pub struct AddRuleRequest {
    pub user_id: Option<Uuid>,
    pub resource_type: String,
    pub resource_id: Uuid,
}

async fn list_rules(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::visibility::VisibilityRuleRow>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_rules");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let rules = crate::db::visibility::list_rules(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list visibility rules");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(rules))
}

async fn add_rule(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let add_req: AddRuleRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in add_rule");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let rule_id = crate::db::visibility::add_rule(
        &client,
        &ctx.project_id,
        add_req.user_id.as_ref(),
        &add_req.resource_type,
        &add_req.resource_id,
        &user.user_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to add visibility rule");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(rule_id = %rule_id, project_id = %ctx.project_id, "Visibility rule added");
    Ok(Json(serde_json::json!({ "rule_id": rule_id })))
}

async fn remove_rule(
    State(state): State<Arc<AppState>>,
    Path((_, rule_id)): Path<(Uuid, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Admin)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in remove_rule");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    crate::db::visibility::remove_rule(&client, &rule_id, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to remove visibility rule");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(rule_id = %rule_id, project_id = %ctx.project_id, "Visibility rule removed");
    Ok(Json(serde_json::json!({ "deleted": true })))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/visibility-rules", get(list_rules).post(add_rule))
        .route("/visibility-rules/:rule_id", delete(remove_rule))
        .with_state(state)
}
