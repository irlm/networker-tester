use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    token: String,
    role: String,
    username: String,
    must_change_password: bool,
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let result = crate::db::users::authenticate(&client, &req.username, &req.password)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Some((user_id, role, must_change_password)) => {
            let token = crate::auth::create_token(user_id, &req.username, &role, &state.jwt_secret)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(Json(LoginResponse {
                token,
                role,
                username: req.username,
                must_change_password,
            }))
        }
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    // Extract auth user from extensions (set by require_auth middleware)
    let auth_user = match req.extensions().get::<crate::auth::AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    // Parse body
    let body = match axum::body::to_bytes(req.into_body(), 4096).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid body").into_response(),
    };
    let payload: ChangePasswordRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response(),
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    };

    match crate::db::users::change_password(
        &client,
        &auth_user.user_id,
        &payload.current_password,
        &payload.new_password,
    )
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({ "success": true })).into_response(),
        Ok(Err(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response(),
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .with_state(state)
}

/// Change-password route — must be added to the protected (auth-required) routes.
pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/change-password", post(change_password))
        .with_state(state)
}
