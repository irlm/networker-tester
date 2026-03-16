use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub username: String,
    pub role: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields read in Phase 2 RBAC checks
pub struct AuthUser {
    pub user_id: Uuid,
    pub username: String,
    pub role: String,
}

pub fn create_token(
    user_id: Uuid,
    username: &str,
    role: &str,
    secret: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        role: role.to_string(),
        exp: now + 24 * 3600, // 24 hours
        iat: now,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn validate_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(data.claims)
}

/// Axum middleware that requires a valid JWT Bearer token.
/// Reads the JWT secret from AppState and injects AuthUser into request extensions.
pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return (StatusCode::UNAUTHORIZED, "Missing authorization header").into_response(),
    };
    match validate_token(token, &state.jwt_secret) {
        Ok(claims) => {
            req.extensions_mut().insert(AuthUser {
                user_id: claims.sub,
                username: claims.username,
                role: claims.role,
            });
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "Invalid token").into_response(),
    }
}
