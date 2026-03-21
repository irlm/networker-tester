use chrono::{DateTime, Utc};
use tokio_postgres::Client;
use uuid::Uuid;

/// Seed the admin user if no users exist.
/// Sets must_change_password = TRUE so the user is forced to set their own password.
pub async fn seed_admin(client: &Client, password: &str) -> anyhow::Result<()> {
    let count: i64 = client
        .query_one("SELECT COUNT(*) FROM dash_user", &[])
        .await?
        .get(0);

    if count == 0 {
        let hash =
            bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;
        client
            .execute(
                "INSERT INTO dash_user (user_id, username, password_hash, role, must_change_password) VALUES ($1, $2, $3, $4, $5)",
                &[&Uuid::new_v4(), &"admin", &hash, &"admin", &true],
            )
            .await?;
        tracing::info!("Seeded admin user (username: admin, must_change_password: true)");
    }
    Ok(())
}

/// Authenticate a user by username and password.
/// Returns (user_id, role, must_change_password) on success.
pub async fn authenticate(
    client: &Client,
    username: &str,
    password: &str,
) -> anyhow::Result<Option<(Uuid, String, bool)>> {
    let row = client
        .query_opt(
            "SELECT user_id, password_hash, role, disabled, must_change_password FROM dash_user WHERE username = $1",
            &[&username],
        )
        .await?;

    match row {
        Some(row) => {
            let disabled: bool = row.get("disabled");
            if disabled {
                return Ok(None);
            }
            let hash: String = row.get("password_hash");
            let valid = bcrypt::verify(password, &hash).map_err(|e| anyhow::anyhow!("{e}"))?;
            if valid {
                let user_id: Uuid = row.get("user_id");
                let role: String = row.get("role");
                let must_change: bool = row.get("must_change_password");
                // Update last login
                client
                    .execute(
                        "UPDATE dash_user SET last_login_at = now() WHERE user_id = $1",
                        &[&user_id],
                    )
                    .await?;
                Ok(Some((user_id, role, must_change)))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

/// Change a user's password and optionally set their email.
/// Clears must_change_password flag.
pub async fn change_password(
    client: &Client,
    user_id: &Uuid,
    current_password: &str,
    new_password: &str,
    email: Option<&str>,
) -> anyhow::Result<Result<(), &'static str>> {
    let row = client
        .query_opt(
            "SELECT password_hash FROM dash_user WHERE user_id = $1 AND disabled = FALSE",
            &[user_id],
        )
        .await?;

    let row = match row {
        Some(r) => r,
        None => return Ok(Err("User not found")),
    };

    let hash: String = row.get("password_hash");
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

    if let Some(email_val) = email {
        client
            .execute(
                "UPDATE dash_user SET password_hash = $1, must_change_password = FALSE, email = $2 WHERE user_id = $3",
                &[&new_hash, &email_val, user_id],
            )
            .await?;
    } else {
        client
            .execute(
                "UPDATE dash_user SET password_hash = $1, must_change_password = FALSE WHERE user_id = $2",
                &[&new_hash, user_id],
            )
            .await?;
    }

    Ok(Ok(()))
}

/// Create a password reset token for the user with the given email.
/// Returns (username, token) if the email exists. Token is valid for 1 hour.
pub async fn create_reset_token(
    client: &Client,
    email: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let row = client
        .query_opt(
            "SELECT user_id, username FROM dash_user WHERE email = $1 AND disabled = FALSE",
            &[&email],
        )
        .await?;

    let row = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    let user_id: Uuid = row.get("user_id");
    let username: String = row.get("username");

    // Generate secure random token
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();

    let expires: DateTime<Utc> = Utc::now() + chrono::Duration::hours(1);

    client
        .execute(
            "UPDATE dash_user SET password_reset_token = $1, password_reset_expires = $2 WHERE user_id = $3",
            &[&token, &expires, &user_id],
        )
        .await?;

    Ok(Some((username, token)))
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

    let row = client
        .query_opt(
            "SELECT user_id, password_reset_expires FROM dash_user WHERE password_reset_token = $1 AND disabled = FALSE",
            &[&token],
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

/// Get the email for a user (for display on change-password page).
pub async fn get_email(client: &Client, user_id: &Uuid) -> anyhow::Result<Option<String>> {
    let row = client
        .query_opt("SELECT email FROM dash_user WHERE user_id = $1", &[user_id])
        .await?;
    Ok(row.and_then(|r| r.get::<_, Option<String>>("email")))
}
