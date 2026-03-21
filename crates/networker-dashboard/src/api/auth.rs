use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::{get, post}, Json, Router};
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
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in login");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let result = crate::db::users::authenticate(&client, &req.username, &req.password)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Authentication query failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

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
    email: Option<String>,
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let auth_user = match req.extensions().get::<crate::auth::AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

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
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    match crate::db::users::change_password(
        &client,
        &auth_user.user_id,
        &payload.current_password,
        &payload.new_password,
        payload.email.as_deref(),
    )
    .await
    {
        Ok(Ok(())) => {
            if let Some(ref email) = payload.email {
                tracing::info!(username = %auth_user.username, email = %email, "Password changed, email set");
            }
            Json(serde_json::json!({ "success": true })).into_response()
        }
        Ok(Err(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response(),
    }
}

/// Get the current user's email (for pre-filling the change-password form).
async fn get_profile(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let auth_user = match req.extensions().get::<crate::auth::AuthUser>() {
        Some(u) => u.clone(),
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    let client = match state.db.get().await {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    let email = crate::db::users::get_email(&client, &auth_user.user_id)
        .await
        .unwrap_or(None);

    Json(serde_json::json!({
        "username": auth_user.username,
        "role": auth_user.role,
        "email": email,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct ForgotPasswordRequest {
    email: String,
}

/// Request a password reset. Always returns 200 (don't reveal if email exists).
async fn forgot_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ForgotPasswordRequest>,
) -> Json<serde_json::Value> {
    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in forgot_password");
            return Json(serde_json::json!({ "sent": true }));
        }
    };

    match crate::db::users::create_reset_token(&client, &req.email).await {
        Ok(Some((username, token))) => {
            // Try to send email; if SMTP not configured, log the link
            let dashboard_url = std::env::var("DASHBOARD_PUBLIC_URL")
                .unwrap_or_else(|_| "http://localhost:5173".into());
            let reset_url = format!("{dashboard_url}/reset-password?token={token}");

            if let Err(e) = send_reset_email(&req.email, &username, &reset_url).await {
                tracing::warn!(error = %e, "SMTP not configured or send failed — logging reset link");
                tracing::info!(
                    username = %username,
                    email = %req.email,
                    reset_url = %reset_url,
                    "PASSWORD RESET LINK (SMTP unavailable)"
                );
            } else {
                tracing::info!(username = %username, email = %req.email, "Password reset email sent");
            }
        }
        Ok(None) => {
            tracing::info!(email = %req.email, "Password reset requested for unknown email");
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to create reset token");
        }
    }

    // Always return success (don't reveal whether email exists)
    Json(serde_json::json!({ "sent": true }))
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    token: String,
    new_password: String,
}

async fn reset_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, &'static str)> {
    let client = state.db.get().await.map_err(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?;

    match crate::db::users::reset_password_with_token(&client, &req.token, &req.new_password).await
    {
        Ok(Ok(())) => {
            tracing::info!("Password reset completed via token");
            Ok(Json(serde_json::json!({ "success": true })))
        }
        Ok(Err(msg)) => Err((StatusCode::BAD_REQUEST, msg)),
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error")),
    }
}

/// Send a password reset email via SMTP.
async fn send_reset_email(to: &str, username: &str, reset_url: &str) -> anyhow::Result<()> {
    use lettre::{
        message::header::ContentType, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
        AsyncTransport, Message, Tokio1Executor,
    };

    let smtp_host = std::env::var("DASHBOARD_SMTP_HOST")
        .map_err(|_| anyhow::anyhow!("DASHBOARD_SMTP_HOST not set"))?;
    let smtp_user = std::env::var("DASHBOARD_SMTP_USER").unwrap_or_default();
    let smtp_pass = std::env::var("DASHBOARD_SMTP_PASS").unwrap_or_default();
    let smtp_from = std::env::var("DASHBOARD_SMTP_FROM")
        .unwrap_or_else(|_| format!("noreply@{smtp_host}"));

    let email = Message::builder()
        .from(smtp_from.parse()?)
        .to(to.parse()?)
        .subject("Networker Dashboard — Password Reset")
        .header(ContentType::TEXT_PLAIN)
        .body(format!(
            "Hi {username},\n\n\
             A password reset was requested for your Networker Dashboard account.\n\n\
             Click the link below to set a new password (valid for 1 hour):\n\n\
             {reset_url}\n\n\
             If you did not request this, ignore this email.\n\n\
             — Networker Dashboard"
        ))?;

    let mailer = if smtp_user.is_empty() {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp_host)?.build()
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp_host)?
            .credentials(Credentials::new(smtp_user, smtp_pass))
            .build()
    };

    mailer.send(email).await?;
    Ok(())
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/forgot-password", post(forgot_password))
        .route("/auth/reset-password", post(reset_password))
        .with_state(state)
}

/// Protected routes (require valid JWT).
pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/change-password", post(change_password))
        .route("/auth/profile", get(get_profile))
        .with_state(state)
}
