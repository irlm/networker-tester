use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct UserRow {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
    pub status: String,
    pub auth_provider: String,
    pub display_name: Option<String>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// List all users ordered by creation date.
pub async fn list_users(client: &Client) -> anyhow::Result<Vec<UserRow>> {
    let rows = client
        .query(
            "SELECT user_id, email, role, status, \
                    COALESCE(auth_provider, 'local') AS auth_provider, \
                    display_name, last_login_at, created_at \
             FROM dash_user ORDER BY created_at",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| UserRow {
            user_id: r.get("user_id"),
            email: r.get("email"),
            role: r.get("role"),
            status: r.get("status"),
            auth_provider: r.get("auth_provider"),
            display_name: r.get("display_name"),
            last_login_at: r.get("last_login_at"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// List users with status = 'pending'.
pub async fn list_pending(client: &Client) -> anyhow::Result<Vec<UserRow>> {
    let rows = client
        .query(
            "SELECT user_id, email, role, status, \
                    COALESCE(auth_provider, 'local') AS auth_provider, \
                    display_name, last_login_at, created_at \
             FROM dash_user WHERE status = 'pending' ORDER BY created_at",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| UserRow {
            user_id: r.get("user_id"),
            email: r.get("email"),
            role: r.get("role"),
            status: r.get("status"),
            auth_provider: r.get("auth_provider"),
            display_name: r.get("display_name"),
            last_login_at: r.get("last_login_at"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Approve a pending user by setting their status to 'active' and role.
pub async fn approve_user(client: &Client, user_id: &Uuid, role: &str) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE dash_user SET status = 'active', role = $1 WHERE user_id = $2 AND status = 'pending'",
            &[&role, user_id],
        )
        .await?;
    Ok(n > 0)
}

/// Deny a pending user by setting their status to 'denied'.
pub async fn deny_user(client: &Client, user_id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE dash_user SET status = 'denied' WHERE user_id = $1 AND status = 'pending'",
            &[user_id],
        )
        .await?;
    Ok(n > 0)
}

/// Change a user's role.
pub async fn set_role(client: &Client, user_id: &Uuid, role: &str) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE dash_user SET role = $1 WHERE user_id = $2 AND status = 'active'",
            &[&role, user_id],
        )
        .await?;
    Ok(n > 0)
}

/// Disable an active user.
pub async fn disable_user(client: &Client, user_id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE dash_user SET status = 'disabled' WHERE user_id = $1 AND status = 'active'",
            &[user_id],
        )
        .await?;
    Ok(n > 0)
}

/// Count pending users.
pub async fn get_pending_count(client: &Client) -> anyhow::Result<i64> {
    let row = client
        .query_one(
            "SELECT COUNT(*) FROM dash_user WHERE status = 'pending'",
            &[],
        )
        .await?;
    Ok(row.get(0))
}

/// Hash a token with SHA-256 so we never store plaintext reset tokens in the DB.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Seed the admin user if no users exist.
/// Sets must_change_password = TRUE so the user is forced to set their own password.
/// Uses email as the primary identity (V008+).
pub async fn seed_admin(client: &Client, email: &str, password: &str) -> anyhow::Result<()> {
    let count: i64 = client
        .query_one("SELECT COUNT(*) FROM dash_user", &[])
        .await?
        .get(0);

    if count == 0 {
        let hash =
            bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;
        client
            .execute(
                "INSERT INTO dash_user (user_id, email, password_hash, role, status, must_change_password) VALUES ($1, $2, $3, $4, $5, $6)",
                &[&Uuid::new_v4(), &email, &hash, &"admin", &"active", &true],
            )
            .await?;
        tracing::info!(email = %email, "Seeded admin user (must_change_password: true)");
    }
    Ok(())
}

/// Authenticate a user by email and password.
/// Returns (user_id, email, role, must_change_password, status) on success.
pub async fn authenticate(
    client: &Client,
    email: &str,
    password: &str,
) -> anyhow::Result<Option<(Uuid, String, String, bool, String)>> {
    let row = client
        .query_opt(
            "SELECT user_id, email, password_hash, role, status, must_change_password, sso_only FROM dash_user WHERE email = $1",
            &[&email],
        )
        .await?;

    match row {
        Some(row) => {
            let status: String = row.get("status");
            if status != "active" {
                return Ok(None);
            }
            let sso_only: bool = row.get("sso_only");
            if sso_only {
                return Ok(None);
            }
            let hash: Option<String> = row.get("password_hash");
            let hash = match hash {
                Some(h) => h,
                None => return Ok(None), // SSO account with no password
            };
            let valid = bcrypt::verify(password, &hash).map_err(|e| anyhow::anyhow!("{e}"))?;
            if valid {
                let user_id: Uuid = row.get("user_id");
                let user_email: String = row.get("email");
                let role: String = row.get("role");
                let must_change: bool = row.get("must_change_password");
                // Update last login
                client
                    .execute(
                        "UPDATE dash_user SET last_login_at = now() WHERE user_id = $1",
                        &[&user_id],
                    )
                    .await?;
                Ok(Some((user_id, user_email, role, must_change, status)))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

/// Change a user's password.
/// Clears must_change_password flag.
/// Email changes are NOT allowed here — email is the primary identity
/// and should only be changed through a separate verified flow.
pub async fn change_password(
    client: &Client,
    user_id: &Uuid,
    current_password: &str,
    new_password: &str,
) -> anyhow::Result<Result<(), &'static str>> {
    let row = client
        .query_opt(
            "SELECT password_hash FROM dash_user WHERE user_id = $1 AND status = 'active'",
            &[user_id],
        )
        .await?;

    let row = match row {
        Some(r) => r,
        None => return Ok(Err("User not found")),
    };

    let hash: Option<String> = row.get("password_hash");
    let hash = match hash {
        Some(h) => h,
        None => return Ok(Err("SSO accounts cannot change password here")),
    };
    let valid = bcrypt::verify(current_password, &hash).map_err(|e| anyhow::anyhow!("{e}"))?;
    if !valid {
        return Ok(Err("Current password is incorrect"));
    }

    if new_password.len() < 8 {
        return Ok(Err("New password must be at least 8 characters"));
    }

    if current_password == new_password {
        return Ok(Err("New password must be different from current password"));
    }

    let new_hash =
        bcrypt::hash(new_password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;

    client
        .execute(
            "UPDATE dash_user SET password_hash = $1, must_change_password = FALSE WHERE user_id = $2",
            &[&new_hash, user_id],
        )
        .await?;

    Ok(Ok(()))
}

/// Create a password reset token for the user with the given email.
/// Returns (email, token) if the email exists. Token is valid for 1 hour.
pub async fn create_reset_token(
    client: &Client,
    email: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let row = client
        .query_opt(
            "SELECT user_id, email FROM dash_user WHERE email = $1 AND status = 'active'",
            &[&email],
        )
        .await?;

    let row = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    let user_id: Uuid = row.get("user_id");
    let user_email: String = row.get("email");

    // Generate secure random token
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();

    let expires: DateTime<Utc> = Utc::now() + chrono::Duration::hours(1);
    let token_hash = hash_token(&token);

    client
        .execute(
            "UPDATE dash_user SET password_reset_token = $1, password_reset_expires = $2 WHERE user_id = $3",
            &[&token_hash, &expires, &user_id],
        )
        .await?;

    // Return raw token (for the email link); only the hash is stored in DB
    Ok(Some((user_email, token)))
}

/// Reset password using a token. Clears the token and must_change_password flag.
pub async fn reset_password_with_token(
    client: &Client,
    token: &str,
    new_password: &str,
) -> anyhow::Result<Result<(), &'static str>> {
    if new_password.len() < 8 {
        return Ok(Err("Password must be at least 8 characters"));
    }

    let token_hash = hash_token(token);
    let row = client
        .query_opt(
            "SELECT user_id, password_reset_expires FROM dash_user WHERE password_reset_token = $1 AND status = 'active'",
            &[&token_hash],
        )
        .await?;

    let row = match row {
        Some(r) => r,
        None => return Ok(Err("Invalid or expired reset link")),
    };

    let expires: Option<DateTime<Utc>> = row.get("password_reset_expires");
    if let Some(exp) = expires {
        if exp < Utc::now() {
            return Ok(Err("Reset link has expired. Request a new one."));
        }
    } else {
        return Ok(Err("Invalid reset link"));
    }

    let user_id: Uuid = row.get("user_id");
    let new_hash =
        bcrypt::hash(new_password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;

    client
        .execute(
            "UPDATE dash_user SET password_hash = $1, must_change_password = FALSE,
             password_reset_token = NULL, password_reset_expires = NULL
             WHERE user_id = $2",
            &[&new_hash, &user_id],
        )
        .await?;

    Ok(Ok(()))
}

/// Get the email and status for a user (for display on profile / change-password page).
pub async fn get_profile_info(
    client: &Client,
    user_id: &Uuid,
) -> anyhow::Result<Option<(String, String)>> {
    let row = client
        .query_opt(
            "SELECT email, status FROM dash_user WHERE user_id = $1",
            &[user_id],
        )
        .await?;
    Ok(row.map(|r| (r.get::<_, String>("email"), r.get::<_, String>("status"))))
}
