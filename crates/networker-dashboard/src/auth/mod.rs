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
/// Enforces must_change_password: only /auth/change-password is allowed when flag is set.
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

    if !auth_header.starts_with("Bearer ") {
        return (StatusCode::UNAUTHORIZED, "Missing authorization header").into_response();
    }

    let token = &auth_header[7..];
    match validate_token(token, &state.jwt_secret) {
        Ok(claims) => {
            // Enforce must_change_password server-side
            let is_change_password = req.uri().path().ends_with("/auth/change-password");
            if !is_change_password {
                if let Ok(client) = state.db.get().await {
                    let must_change = client
                        .query_opt(
                            "SELECT must_change_password FROM dash_user WHERE user_id = $1",
                            &[&claims.sub],
                        )
                        .await
                        .ok()
                        .flatten()
                        .and_then(|row| row.get::<_, Option<bool>>("must_change_password"))
                        .unwrap_or(false);
                    if must_change {
                        return (
                            StatusCode::FORBIDDEN,
                            "Password change required before accessing this resource",
                        )
                            .into_response();
                    }
                }
            }

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
