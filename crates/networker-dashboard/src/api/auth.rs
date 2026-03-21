use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
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
            let reset_url = format!("{}/reset-password?token={token}", state.public_url);
            let body = format!(
                "Hi {user_email},\n\n\
                 A password reset was requested for your Networker Dashboard account.\n\n\
                 Click the link below to set a new password (valid for 1 hour):\n\n\
                 {reset_url}\n\n\
                 If you did not request this, ignore this email.\n\n\
                 — Networker Dashboard"
            );

            if let Err(e) =
                crate::email::send_email(&req.email, "Networker Dashboard — Password Reset", &body)
                    .await
            {
                tracing::warn!(error = %e, "Failed to send password reset email");
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

// ---------------------------------------------------------------------------
// SSO: OAuth 2.0 Authorization Code flow (Microsoft Entra ID + Google)
// ---------------------------------------------------------------------------

/// Return SSO provider configuration to the frontend.
async fn sso_providers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let mut providers = Vec::new();
    if state.microsoft_client_id.is_some() {
        providers.push("microsoft");
    }
    if state.google_client_id.is_some() {
        providers.push("google");
    }
    Json(serde_json::json!({ "providers": providers }))
}

#[derive(Deserialize)]
pub struct SsoInitQuery {
    provider: String,
}

/// Redirect user to the SSO provider's authorization endpoint.
/// Sets an `sso_state` cookie for CSRF protection.
async fn sso_init(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SsoInitQuery>,
) -> impl IntoResponse {
    let provider = &query.provider;

    let (auth_url, client_id) = match provider.as_str() {
        "microsoft" => {
            let cid = match &state.microsoft_client_id {
                Some(c) => c.clone(),
                None => {
                    return (StatusCode::BAD_REQUEST, "Microsoft SSO not configured")
                        .into_response()
                }
            };
            let url = format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
                state.microsoft_tenant_id
            );
            (url, cid)
        }
        "google" => {
            let cid = match &state.google_client_id {
                Some(c) => c.clone(),
                None => {
                    return (StatusCode::BAD_REQUEST, "Google SSO not configured").into_response()
                }
            };
            (
                "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                cid,
            )
        }
        _ => return (StatusCode::BAD_REQUEST, "Unknown SSO provider").into_response(),
    };

    // Generate CSRF state token
    use rand::Rng;
    let state_value: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let redirect_uri = format!("{}/api/auth/sso/callback", state.public_url);
    let scope = match provider.as_str() {
        "microsoft" => "openid email profile",
        "google" => "openid email profile",
        _ => "openid email",
    };

    let full_state = format!("{provider}:{state_value}");

    let redirect_url = format!(
        "{auth_url}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(scope),
        urlencoding::encode(&full_state),
    );

    // Fix 3: Add Secure flag when using HTTPS
    let secure_flag = if state.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let cookie =
        format!("sso_state={full_state}; HttpOnly; SameSite=Lax; Path=/; Max-Age=300{secure_flag}");

    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            ("location", redirect_url.as_str()),
            ("set-cookie", cookie.as_str()),
        ],
        "",
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct SsoCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Handle the OAuth callback from the SSO provider.
async fn sso_callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SsoCallbackQuery>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    // Check for OAuth error
    if let Some(ref err) = query.error {
        tracing::warn!(error = %err, "SSO provider returned error");
        return redirect_to_login_with_error(&state.public_url, "provider_error");
    }

    let code = match &query.code {
        Some(c) => c.clone(),
        None => return redirect_to_login_with_error(&state.public_url, "missing_code"),
    };
    let callback_state = match &query.state {
        Some(s) => s.clone(),
        None => return redirect_to_login_with_error(&state.public_url, "missing_state"),
    };

    // Validate CSRF state from cookie
    let cookie_header = req
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookie_state = extract_cookie(cookie_header, "sso_state");
    if cookie_state.as_deref() != Some(callback_state.as_str()) {
        tracing::warn!("SSO state mismatch (CSRF check failed)");
        return redirect_to_login_with_error(&state.public_url, "state_mismatch");
    }

    // Parse provider from state (format: "provider:random")
    let provider = callback_state.split(':').next().unwrap_or("").to_string();

    let (client_id, client_secret, token_url) = match provider.as_str() {
        "microsoft" => {
            let cid = match &state.microsoft_client_id {
                Some(c) => c.clone(),
                None => return redirect_to_login_with_error(&state.public_url, "not_configured"),
            };
            let csec = match &state.microsoft_client_secret {
                Some(s) => s.clone(),
                None => return redirect_to_login_with_error(&state.public_url, "not_configured"),
            };
            let url = format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                state.microsoft_tenant_id
            );
            (cid, csec, url)
        }
        "google" => {
            let cid = match &state.google_client_id {
                Some(c) => c.clone(),
                None => return redirect_to_login_with_error(&state.public_url, "not_configured"),
            };
            let csec = match &state.google_client_secret {
                Some(s) => s.clone(),
                None => return redirect_to_login_with_error(&state.public_url, "not_configured"),
            };
            (cid, csec, "https://oauth2.googleapis.com/token".to_string())
        }
        _ => return redirect_to_login_with_error(&state.public_url, "unknown_provider"),
    };

    // Exchange authorization code for tokens
    let redirect_uri = format!("{}/api/auth/sso/callback", state.public_url);
    let http_client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return redirect_to_login_with_error(&state.public_url, "internal_error"),
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
            tracing::error!(error = %e, "SSO token exchange failed");
            return redirect_to_login_with_error(&state.public_url, "token_exchange_failed");
        }
    };

    let token_json: serde_json::Value = match token_resp.json().await {
        Ok(j) => j,
        Err(e) => {
            tracing::error!(error = %e, "SSO token response parse failed");
            return redirect_to_login_with_error(&state.public_url, "token_parse_failed");
        }
    };

    let id_token = match token_json.get("id_token").and_then(|t| t.as_str()) {
        Some(t) => t.to_string(),
        None => {
            // Fix 5: Don't log token response body — only log the keys
            let keys: Vec<&String> = token_json
                .as_object()
                .map(|o| o.keys().collect())
                .unwrap_or_default();
            tracing::error!(provider = %provider, ?keys, "No id_token in SSO token response");
            return redirect_to_login_with_error(&state.public_url, "no_id_token");
        }
    };

    // Decode ID token (JWT) payload without cryptographic verification
    // (we trust the token because it came directly from the provider over HTTPS
    // in the authorization code flow — the code was just exchanged server-side)
    let claims = match decode_jwt_payload(&id_token) {
        Some(c) => c,
        None => {
            tracing::error!("Failed to decode SSO id_token payload");
            return redirect_to_login_with_error(&state.public_url, "id_token_decode_failed");
        }
    };

    // Fix 1: Validate ID token issuer and audience
    let issuer = claims.get("iss").and_then(|i| i.as_str()).unwrap_or("");
    let audience = claims.get("aud").and_then(|a| a.as_str()).unwrap_or("");

    match provider.as_str() {
        "microsoft" => {
            if !issuer.starts_with("https://login.microsoftonline.com/") || audience != client_id {
                tracing::error!(
                    %issuer, %audience, expected_aud = %client_id,
                    "Microsoft ID token iss/aud mismatch"
                );
                return redirect_to_login_with_error(&state.public_url, "id_token_invalid");
            }
        }
        "google" => {
            if issuer != "https://accounts.google.com" || audience != client_id {
                tracing::error!(
                    %issuer, %audience, expected_aud = %client_id,
                    "Google ID token iss/aud mismatch"
                );
                return redirect_to_login_with_error(&state.public_url, "id_token_invalid");
            }
        }
        _ => {}
    }

    // Extract user info from claims
    let email = claims
        .get("email")
        .and_then(|e| e.as_str())
        .or_else(|| claims.get("preferred_username").and_then(|u| u.as_str()))
        .unwrap_or("")
        .to_lowercase();
    let subject_id = claims
        .get("sub")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let display_name = claims
        .get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    if email.is_empty() || subject_id.is_empty() {
        tracing::error!("SSO id_token missing email or sub claim");
        return redirect_to_login_with_error(&state.public_url, "missing_claims");
    }

    // Look up or create user
    let client = match state.db.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "DB pool error in SSO callback");
            return redirect_to_login_with_error(&state.public_url, "internal_error");
        }
    };

    let existing = crate::db::users::find_by_email(&client, &email)
        .await
        .ok()
        .flatten();

    let (user_id, role, user_status) = if let Some(existing) = existing {
        if existing.auth_provider == "local" {
            // Fix 2: Don't auto-link admin accounts
            if existing.role == "admin" {
                tracing::warn!(
                    email = %email,
                    provider = %provider,
                    "Refusing to auto-link admin account to SSO — manual linking required"
                );
                return redirect_to_login_with_error(&state.public_url, "admin_link_blocked");
            }

            // Auto-link local account to SSO
            if let Err(e) = crate::db::users::link_sso_to_local(
                &client,
                &existing.user_id,
                &provider,
                &subject_id,
                display_name.as_deref(),
            )
            .await
            {
                tracing::error!(error = %e, "Failed to link SSO to local account");
                return redirect_to_login_with_error(&state.public_url, "internal_error");
            }
            tracing::info!(email = %email, provider = %provider, "Linked SSO to existing local account");
            (existing.user_id, existing.role, existing.status)
        } else {
            // Existing SSO user — update last login
            client
                .execute(
                    "UPDATE dash_user SET last_login_at = now() WHERE user_id = $1",
                    &[&existing.user_id],
                )
                .await
                .ok();
            (existing.user_id, existing.role, existing.status)
        }
    } else {
        // New SSO user — create with pending status
        match crate::db::users::create_sso_user(
            &client,
            &email,
            &provider,
            &subject_id,
            display_name.as_deref(),
        )
        .await
        {
            Ok((uid, role)) => {
                tracing::info!(email = %email, provider = %provider, "Created new SSO user (pending approval)");
                (uid, role, "pending".to_string())
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to create SSO user");
                return redirect_to_login_with_error(&state.public_url, "internal_error");
            }
        }
    };

    if user_status != "active" && user_status != "pending" {
        return redirect_to_login_with_error(&state.public_url, "account_disabled");
    }

    // Generate a short-lived exchange code that the frontend can use to get a JWT.
    // This avoids putting the JWT in a URL query parameter.
    use rand::Rng;
    let exchange_code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();

    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(2);

    {
        let mut codes = state.sso_codes.lock().unwrap();
        // Prune expired codes
        let now = chrono::Utc::now();
        codes.retain(|_, entry| entry.expires_at > now);

        // Fix 4: Bound the SSO codes map
        if codes.len() > 10_000 {
            tracing::warn!(count = codes.len(), "SSO code map too large");
            drop(codes);
            return redirect_to_login_with_error(&state.public_url, "service_busy");
        }

        codes.insert(
            exchange_code.clone(),
            crate::SsoCodeEntry {
                email: email.clone(),
                role,
                user_id,
                expires_at,
            },
        );
    }

    // Clear sso_state cookie and redirect to frontend with exchange code
    // Fix 3: Include Secure flag when clearing cookie too
    let secure_flag = if state.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let clear_cookie =
        format!("sso_state=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure_flag}");

    let redirect_url = format!("{}/sso-complete?code={exchange_code}", state.public_url);

    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            ("location", redirect_url.as_str()),
            ("set-cookie", clear_cookie.as_str()),
        ],
        "",
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct SsoExchangeRequest {
    code: String,
}

/// Exchange a short-lived SSO code for a JWT token.
async fn sso_exchange(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SsoExchangeRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let entry = {
        let mut codes = state.sso_codes.lock().unwrap();
        codes.remove(&req.code)
    };

    let entry = match entry {
        Some(e) => e,
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let email = entry.email;
    let role = entry.role;
    let user_id = entry.user_id;

    if chrono::Utc::now() > entry.expires_at {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = crate::auth::create_token(user_id, &email, &role, &state.jwt_secret)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Determine status for the response
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let status = client
        .query_opt(
            "SELECT status FROM dash_user WHERE user_id = $1",
            &[&user_id],
        )
        .await
        .ok()
        .flatten()
        .map(|r| r.get::<_, String>("status"))
        .unwrap_or_else(|| "active".to_string());

    Ok(Json(LoginResponse {
        token,
        role,
        email,
        status,
        must_change_password: false,
    }))
}

/// Decode the payload section of a JWT without verifying the signature.
/// Used for ID tokens received directly from the provider over HTTPS.
fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    serde_json::from_slice(&payload_bytes).ok()
}

/// Extract a cookie value by name from the Cookie header.
fn extract_cookie(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

/// Redirect to the login page with an error query parameter.
fn redirect_to_login_with_error(public_url: &str, error: &str) -> axum::response::Response {
    let url = format!("{public_url}/login?error={error}");
    (
        StatusCode::TEMPORARY_REDIRECT,
        [("location", url.as_str())],
        "",
    )
        .into_response()
}

/// URL-encode helper (minimal, for query parameters).
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for byte in s.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(byte as char);
                }
                _ => {
                    result.push('%');
                    result.push_str(&format!("{byte:02X}"));
                }
            }
        }
        result
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/forgot-password", post(forgot_password))
        .route("/auth/reset-password", post(reset_password))
        .route("/auth/sso/providers", get(sso_providers))
        .route("/auth/sso/init", get(sso_init))
        .route("/auth/sso/callback", get(sso_callback))
        .route("/auth/sso/exchange", post(sso_exchange))
        .with_state(state)
}

/// Protected routes (require valid JWT).
pub fn protected_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/change-password", post(change_password))
        .route("/auth/profile", get(get_profile))
        .with_state(state)
}
