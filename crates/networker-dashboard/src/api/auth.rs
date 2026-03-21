use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    token: String,
    role: String,
    email: String,
    status: String,
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
    let result = crate::db::users::authenticate(&client, &req.email, &req.password)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Authentication query failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    match result {
        Some((user_id, email, role, must_change_password, status)) => {
            let token = crate::auth::create_token(user_id, &email, &role, &state.jwt_secret)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(Json(LoginResponse {
                token,
                role,
                email,
                status,
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
    )
    .await
    {
        Ok(Ok(())) => {
            tracing::info!(user_email = %auth_user.email, "Password changed");
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

    let profile = crate::db::users::get_profile_info(&client, &auth_user.user_id)
        .await
        .unwrap_or(None);

    let (email, status) = match profile {
        Some((e, s)) => (e, s),
        None => (auth_user.email.clone(), "active".to_string()),
    };

    Json(serde_json::json!({
        "email": email,
        "role": auth_user.role,
        "status": status,
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
        Ok(Some((user_email, token))) => {
            // Try to send email; if SMTP not configured, log the link
            let dashboard_url = std::env::var("DASHBOARD_PUBLIC_URL")
                .unwrap_or_else(|_| "http://localhost:5173".into());
            let reset_url = format!("{dashboard_url}/reset-password?token={token}");

            if let Err(e) = send_reset_email(&req.email, &user_email, &reset_url).await {
                tracing::warn!(error = %e, "SMTP not configured or send failed — logging reset link");
                tracing::info!(
                    email = %user_email,
                    reset_url = %reset_url,
                    "PASSWORD RESET LINK (SMTP unavailable)"
                );
            } else {
                tracing::info!(email = %user_email, "Password reset email sent");
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
    let client = state
        .db
        .get()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error"))?;

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
async fn send_reset_email(to: &str, display_name: &str, reset_url: &str) -> anyhow::Result<()> {
    use lettre::{
        message::header::ContentType, transport::smtp::authentication::Credentials,
        AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    };

    let smtp_host = std::env::var("DASHBOARD_SMTP_HOST")
        .map_err(|_| anyhow::anyhow!("DASHBOARD_SMTP_HOST not set"))?;
    let smtp_user = std::env::var("DASHBOARD_SMTP_USER").unwrap_or_default();
    let smtp_pass = std::env::var("DASHBOARD_SMTP_PASS").unwrap_or_default();
    let smtp_from =
        std::env::var("DASHBOARD_SMTP_FROM").unwrap_or_else(|_| format!("noreply@{smtp_host}"));

    let email = Message::builder()
        .from(smtp_from.parse()?)
        .to(to.parse()?)
        .subject("Networker Dashboard — Password Reset")
        .header(ContentType::TEXT_PLAIN)
        .body(format!(
            "Hi {display_name},\n\n\
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

// ─── SSO endpoints ────────────────────────────────────────────────────

/// Returns which SSO providers are configured.
async fn get_providers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "microsoft": state.microsoft_client_id.is_some(),
        "google": state.google_client_id.is_some(),
    }))
}

#[derive(Deserialize)]
pub struct CheckEmailRequest {
    email: String,
}

/// Check if an email domain should be routed to an SSO provider.
async fn check_email(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckEmailRequest>,
) -> Json<serde_json::Value> {
    // If hide_sso_domains is set, never reveal the provider
    if state.hide_sso_domains {
        return Json(serde_json::json!({ "provider": null }));
    }

    // If Microsoft is configured, route all emails to Microsoft SSO.
    // Domain-specific mapping is a v0.15 feature.
    if state.microsoft_client_id.is_some() && req.email.contains('@') {
        return Json(serde_json::json!({ "provider": "microsoft" }));
    }

    // If Google is configured (but not Microsoft), route to Google
    if state.google_client_id.is_some() && req.email.contains('@') {
        return Json(serde_json::json!({ "provider": "google" }));
    }

    Json(serde_json::json!({ "provider": null }))
}

/// Initiate SSO flow — redirect to provider authorize URL.
async fn sso_redirect(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> Response {
    let (authorize_url, client_id) = match provider.as_str() {
        "microsoft" => {
            let client_id = match &state.microsoft_client_id {
                Some(id) => id.clone(),
                None => {
                    return (StatusCode::BAD_REQUEST, "Microsoft SSO not configured")
                        .into_response()
                }
            };
            let tenant = &state.microsoft_tenant_id;
            let redirect_uri =
                format!("{}/api/auth/callback/microsoft", state.public_url);
            let redirect_uri_encoded = urlencoded(&redirect_uri);
            let url = format!(
                "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize\
                 ?client_id={client_id}\
                 &redirect_uri={redirect_uri_encoded}\
                 &scope=openid%20email%20profile\
                 &response_type=code"
            );
            (url, client_id)
        }
        "google" => {
            let client_id = match &state.google_client_id {
                Some(id) => id.clone(),
                None => {
                    return (StatusCode::BAD_REQUEST, "Google SSO not configured").into_response()
                }
            };
            let redirect_uri =
                format!("{}/api/auth/callback/google", state.public_url);
            let redirect_uri_encoded = urlencoded(&redirect_uri);
            let url = format!(
                "https://accounts.google.com/o/oauth2/v2/auth\
                 ?client_id={client_id}\
                 &redirect_uri={redirect_uri_encoded}\
                 &scope=openid%20email%20profile\
                 &response_type=code"
            );
            (url, client_id)
        }
        _ => return (StatusCode::BAD_REQUEST, "Unknown SSO provider").into_response(),
    };
    let _ = client_id; // used above in URL construction

    // Generate random state parameter for CSRF protection
    let state_value = generate_random_hex(32);
    let authorize_url = format!("{authorize_url}&state={state_value}");

    // Set sso_state cookie (HttpOnly, SameSite=Lax, 5min max-age)
    let cookie = format!(
        "sso_state={state_value}; HttpOnly; SameSite=Lax; Path=/; Max-Age=300"
    );

    (
        [(header::SET_COOKIE, cookie)],
        Redirect::temporary(&authorize_url),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// OAuth callback — exchange code for tokens, create/link user, redirect with one-time code.
async fn sso_callback(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    Query(query): Query<CallbackQuery>,
    req: axum::extract::Request,
) -> Response {
    // Check for OAuth error
    if let Some(ref err) = query.error {
        tracing::warn!(provider = %provider, error = %err, "SSO provider returned error");
        return Redirect::temporary(&format!(
            "{}/login?error=sso_denied",
            state.public_url
        ))
        .into_response();
    }

    let code = match &query.code {
        Some(c) => c.clone(),
        None => {
            return Redirect::temporary(&format!(
                "{}/login?error=missing_code",
                state.public_url
            ))
            .into_response()
        }
    };

    // Verify state matches cookie
    let query_state = match &query.state {
        Some(s) => s.clone(),
        None => {
            return Redirect::temporary(&format!(
                "{}/login?error=missing_state",
                state.public_url
            ))
            .into_response()
        }
    };

    let cookie_state = extract_cookie_value(&req, "sso_state");
    if cookie_state.as_deref() != Some(query_state.as_str()) {
        tracing::warn!(
            provider = %provider,
            "SSO state mismatch (CSRF check failed)"
        );
        return Redirect::temporary(&format!(
            "{}/login?error=state_mismatch",
            state.public_url
        ))
        .into_response();
    }

    // Exchange authorization code for tokens
    let (token_url, redirect_uri, client_id, client_secret) = match provider.as_str() {
        "microsoft" => {
            let tenant = &state.microsoft_tenant_id;
            (
                format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
                format!("{}/api/auth/callback/microsoft", state.public_url),
                state.microsoft_client_id.clone().unwrap_or_default(),
                state.microsoft_client_secret.clone().unwrap_or_default(),
            )
        }
        "google" => (
            "https://oauth2.googleapis.com/token".to_string(),
            format!("{}/api/auth/callback/google", state.public_url),
            state.google_client_id.clone().unwrap_or_default(),
            state.google_client_secret.clone().unwrap_or_default(),
        ),
        _ => {
            return (StatusCode::BAD_REQUEST, "Unknown provider").into_response();
        }
    };

    let http_client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "Failed to build HTTP client for SSO token exchange");
            return Redirect::temporary(&format!(
                "{}/login?error=internal",
                state.public_url
            ))
            .into_response();
        }
    };

    let token_resp = http_client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("client_id", &client_id),
            ("client_secret", &client_secret),
        ])
        .send()
        .await;

    let token_resp = match token_resp {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, provider = %provider, "SSO token exchange request failed");
            return Redirect::temporary(&format!(
                "{}/login?error=token_exchange_failed",
                state.public_url
            ))
            .into_response();
        }
    };

    let token_body: serde_json::Value = match token_resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, provider = %provider, "Failed to parse SSO token response");
            return Redirect::temporary(&format!(
                "{}/login?error=token_parse_failed",
                state.public_url
            ))
            .into_response();
        }
    };

    // Extract id_token and decode payload (no signature verification — received over TLS)
    let id_token = match token_body.get("id_token").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => {
            tracing::error!(provider = %provider, body = %token_body, "No id_token in SSO token response");
            return Redirect::temporary(&format!(
                "{}/login?error=no_id_token",
                state.public_url
            ))
            .into_response();
        }
    };

    let claims = match decode_jwt_payload(id_token) {
        Some(c) => c,
        None => {
            tracing::error!(provider = %provider, "Failed to decode id_token payload");
            return Redirect::temporary(&format!(
                "{}/login?error=id_token_decode",
                state.public_url
            ))
            .into_response();
        }
    };

    let email = claims
        .get("email")
        .and_then(|e| e.as_str())
        .or_else(|| {
            // Microsoft sometimes puts email in "preferred_username"
            claims
                .get("preferred_username")
                .and_then(|e| e.as_str())
        })
        .unwrap_or("")
        .to_lowercase();

    let sub = claims
        .get("sub")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    let name = claims
        .get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    let picture = claims
        .get("picture")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());

    if email.is_empty() || sub.is_empty() {
        tracing::error!(
            provider = %provider,
            email = %email,
            sub = %sub,
            "SSO id_token missing email or sub"
        );
        return Redirect::temporary(&format!(
            "{}/login?error=missing_claims",
            state.public_url
        ))
        .into_response();
    }

    tracing::info!(
        provider = %provider,
        email = %email,
        sub = %sub,
        name = ?name,
        "SSO callback: processing user"
    );

    // DB lookup / creation
    let db_client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in SSO callback");
            return Redirect::temporary(&format!(
                "{}/login?error=internal",
                state.public_url
            ))
            .into_response();
        }
    };

    // Step 1: Check if returning SSO user
    let user = match crate::db::sso::find_by_sso(&db_client, &provider, &sub).await {
        Ok(Some(user)) => {
            // Update last login
            crate::db::sso::update_last_login(&db_client, &user.user_id)
                .await
                .ok();
            user
        }
        Ok(None) => {
            // Step 2: Check if email matches a local account
            match crate::db::sso::find_by_email(&db_client, &email).await {
                Ok(Some(existing)) => {
                    if existing.auth_provider == "local" {
                        // Auto-link with warning
                        tracing::warn!(
                            user_id = %existing.user_id,
                            email = %email,
                            provider = %provider,
                            "Auto-linking local account to SSO provider (no password verification)"
                        );
                        if let Err(e) = crate::db::sso::link_sso_to_local(
                            &db_client,
                            &existing.user_id,
                            &provider,
                            &sub,
                        )
                        .await
                        {
                            tracing::error!(error = %e, "Failed to link SSO to local account");
                            return Redirect::temporary(&format!(
                                "{}/login?error=link_failed",
                                state.public_url
                            ))
                            .into_response();
                        }
                        crate::db::sso::update_last_login(&db_client, &existing.user_id)
                            .await
                            .ok();
                        existing
                    } else {
                        // Already linked to a different SSO provider
                        crate::db::sso::update_last_login(&db_client, &existing.user_id)
                            .await
                            .ok();
                        existing
                    }
                }
                Ok(None) => {
                    // Step 3: Create new SSO user
                    match crate::db::sso::create_sso_user(
                        &db_client,
                        &email,
                        &provider,
                        &sub,
                        name.as_deref(),
                        picture.as_deref(),
                    )
                    .await
                    {
                        Ok(user_id) => {
                            // Fetch the created user
                            match crate::db::sso::find_by_sso(&db_client, &provider, &sub).await {
                                Ok(Some(u)) => u,
                                _ => {
                                    tracing::error!(
                                        user_id = %user_id,
                                        "Failed to fetch newly created SSO user"
                                    );
                                    return Redirect::temporary(&format!(
                                        "{}/login?error=internal",
                                        state.public_url
                                    ))
                                    .into_response();
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to create SSO user");
                            return Redirect::temporary(&format!(
                                "{}/login?error=create_failed",
                                state.public_url
                            ))
                            .into_response();
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "DB error looking up email in SSO callback");
                    return Redirect::temporary(&format!(
                        "{}/login?error=internal",
                        state.public_url
                    ))
                    .into_response();
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "DB error in SSO callback");
            return Redirect::temporary(&format!(
                "{}/login?error=internal",
                state.public_url
            ))
            .into_response();
        }
    };

    // Generate one-time exchange code
    let raw_code = generate_random_hex(32);
    let code_hash = hash_string(&raw_code);

    // Store in the in-memory map with 30s TTL
    {
        let expires = chrono::Utc::now() + chrono::Duration::seconds(30);
        let mut codes = state.sso_codes.write().await;

        // Prune expired codes while we're here
        let now = chrono::Utc::now();
        codes.retain(|_, (_, exp)| *exp > now);

        codes.insert(code_hash, (user.user_id, expires));
    }

    // Redirect to frontend with the one-time code; clear sso_state cookie
    let clear_cookie = "sso_state=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0";
    let redirect_url = format!(
        "{}/auth/sso-complete?code={raw_code}",
        state.public_url
    );

    (
        [(header::SET_COOKIE, clear_cookie.to_string())],
        Redirect::temporary(&redirect_url),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ExchangeCodeRequest {
    code: String,
}

/// Exchange a one-time SSO code for a JWT token.
async fn exchange_code(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExchangeCodeRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let code_hash = hash_string(&req.code);
    let now = chrono::Utc::now();

    // Look up and remove the code
    let user_id = {
        let mut codes = state.sso_codes.write().await;
        match codes.remove(&code_hash) {
            Some((uid, expires)) if expires > now => Some(uid),
            Some(_) => {
                tracing::warn!("SSO exchange code expired");
                None
            }
            None => None,
        }
    };

    let user_id = match user_id {
        Some(uid) => uid,
        None => return Err(StatusCode::BAD_REQUEST),
    };

    // Fetch user info from DB
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in exchange_code");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let row = client
        .query_opt(
            "SELECT email, role, status FROM dash_user WHERE user_id = $1",
            &[&user_id],
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "DB query error in exchange_code");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let row = match row {
        Some(r) => r,
        None => return Err(StatusCode::BAD_REQUEST),
    };

    let email: String = row.get("email");
    let role: String = row.get("role");
    let status: String = row.get("status");

    let token = crate::auth::create_token(user_id, &email, &role, &state.jwt_secret)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "token": token,
        "email": email,
        "role": role,
        "status": status,
    })))
}

// ─── SSO helpers ──────────────────────────────────────────────────────

/// Decode the payload (middle segment) of a JWT without signature verification.
fn decode_jwt_payload(jwt: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = parts[1];
    // JWT uses base64url without padding
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// URL-encode a string (percent encoding).
fn urlencoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Generate a random hex string of the given byte length.
fn generate_random_hex(bytes: usize) -> String {
    use rand::Rng;
    let random_bytes: Vec<u8> = (0..bytes).map(|_| rand::thread_rng().gen()).collect();
    random_bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash a string with SHA-256, returning hex.
fn hash_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extract a cookie value from the request headers.
fn extract_cookie_value(req: &axum::extract::Request, name: &str) -> Option<String> {
    let cookie_header = req
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?;
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/forgot-password", post(forgot_password))
        .route("/auth/reset-password", post(reset_password))
        .route("/auth/providers", get(get_providers))
        .route("/auth/check-email", post(check_email))
        .route("/auth/sso/:provider", get(sso_redirect))
        .route("/auth/callback/:provider", get(sso_callback))
        .route("/auth/exchange-code", post(exchange_code))
        .with_state(state)
}

/// Protected routes (require valid JWT).
pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/change-password", post(change_password))
        .route("/auth/profile", get(get_profile))
        .with_state(state)
}
