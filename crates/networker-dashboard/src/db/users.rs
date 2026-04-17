use chrono::{DateTime, Utc};
use rand::RngExt;
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

/// Hash a token with SHA-256 so we never store plaintext reset tokens in the DB.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Seed the admin user if no users exist, or resync the admin's password and
/// platform-admin flag on every boot when the account already exists.
///
/// Rationale: the original implementation skipped everything when any user
/// existed, so changing `DASHBOARD_ADMIN_PASSWORD` between boots had no
/// effect and admins who pre-existed without `is_platform_admin` stayed
/// locked out. Resyncing on every boot makes the env var the source of
/// truth for the bootstrap identity — which is what operators expect in
/// both dev reseeding and ops-driven password rotation.
pub async fn seed_admin(client: &Client, email: &str, password: &str) -> anyhow::Result<()> {
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;

    let existing = client
        .query_opt("SELECT user_id FROM dash_user WHERE email = $1", &[&email])
        .await?;

    let user_id = if let Some(row) = existing {
        let user_id: Uuid = row.get("user_id");
        client
            .execute(
                "UPDATE dash_user
                    SET password_hash        = $2,
                        status               = 'active',
                        must_change_password = FALSE,
                        is_platform_admin    = TRUE
                  WHERE user_id = $1",
                &[&user_id, &hash],
            )
            .await?;
        tracing::info!(email = %email, "Resynced bootstrap admin password + platform-admin flag");
        user_id
    } else {
        let new_id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO dash_user (user_id, email, password_hash, role, status, must_change_password, is_platform_admin) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &new_id,
                    &email,
                    &hash,
                    &"admin",
                    &"active",
                    &false,
                    &true,
                ],
            )
            .await?;
        tracing::info!(email = %email, "Seeded admin user (is_platform_admin: true)");
        new_id
    };

    // Ensure the admin owns an admin membership on every existing project.
    // Platform admins bypass membership checks in middleware, but the UI
    // (member lists, project role display) still reads project_member. A
    // missing row would show the admin as a non-member of their own
    // workspace after a DB reseed.
    client
        .execute(
            "INSERT INTO project_member (project_id, user_id, role, status, joined_at) \
             SELECT p.project_id, $1, 'admin', 'active', NOW() \
             FROM project p \
             WHERE p.deleted_at IS NULL \
             ON CONFLICT (project_id, user_id) DO NOTHING",
            &[&user_id],
        )
        .await?;

    Ok(())
}

/// Authenticate a user by email and password.
/// Returns (user_id, email, role, must_change_password, status, is_platform_admin) on success.
pub async fn authenticate(
    client: &Client,
    email: &str,
    password: &str,
) -> anyhow::Result<Option<(Uuid, String, String, bool, String, bool)>> {
    let row = client
        .query_opt(
            "SELECT user_id, email, password_hash, role, status, must_change_password, sso_only, is_platform_admin FROM dash_user WHERE email = $1",
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
                let is_platform_admin: bool = row
                    .get::<_, Option<bool>>("is_platform_admin")
                    .unwrap_or(false);
                // Update last login
                client
                    .execute(
                        "UPDATE dash_user SET last_login_at = now() WHERE user_id = $1",
                        &[&user_id],
                    )
                    .await?;
                Ok(Some((
                    user_id,
                    user_email,
                    role,
                    must_change,
                    status,
                    is_platform_admin,
                )))
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
            "SELECT password_hash FROM dash_user WHERE user_id = $1 AND status IN ('active', 'pending')",
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
    let token: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
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

/// Invite a user by creating a pending account with a setup token.
/// Returns Ok(Ok((user_id, raw_token))) on success, Ok(Err(msg)) if email exists or role invalid.
pub async fn invite_user(
    client: &Client,
    email: &str,
    role: &str,
) -> anyhow::Result<Result<(Uuid, String), &'static str>> {
    // Validate role
    if !["admin", "operator", "viewer"].contains(&role) {
        return Ok(Err("Invalid role (must be admin, operator, or viewer)"));
    }

    // Check if email already exists
    let existing = client
        .query_opt(
            "SELECT user_id FROM dash_user WHERE LOWER(email) = LOWER($1)",
            &[&email],
        )
        .await?;
    if existing.is_some() {
        return Ok(Err("Email already registered"));
    }

    let user_id = Uuid::new_v4();

    // Generate setup token (stored hashed, 24h expiry)
    let token: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let token_hash = hash_token(&token);
    let expires: DateTime<Utc> = Utc::now() + chrono::Duration::hours(24);

    client
        .execute(
            "INSERT INTO dash_user (user_id, email, role, status, auth_provider, must_change_password, \
             password_reset_token, password_reset_expires) \
             VALUES ($1, $2, $3, 'pending', 'local', TRUE, $4, $5)",
            &[&user_id, &email, &role, &token_hash, &expires],
        )
        .await?;

    tracing::info!(
        email = %email,
        role = %role,
        "User invited — setup token generated"
    );

    Ok(Ok((user_id, token)))
}

/// Create a local user with email and password (used by invite acceptance).
pub async fn create_local_user(
    client: &Client,
    email: &str,
    password: &str,
) -> anyhow::Result<Uuid> {
    let user_id = Uuid::new_v4();
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;
    client
        .execute(
            "INSERT INTO dash_user (user_id, email, password_hash, role, status, auth_provider, must_change_password, is_platform_admin) \
             VALUES ($1, $2, $3, 'viewer', 'active', 'local', FALSE, FALSE)",
            &[&user_id, &email, &hash],
        )
        .await?;
    Ok(user_id)
}

/// Create a placeholder user account for CSV import.
/// If a user with the given email already exists, returns their user_id.
/// Otherwise creates a new active user with no password (SSO-pending style).
pub async fn create_placeholder_user(client: &Client, email: &str) -> anyhow::Result<Uuid> {
    // Check if user already exists
    if let Some(existing) = find_by_email(client, email).await? {
        return Ok(existing.user_id);
    }

    let user_id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO dash_user (user_id, email, status, must_change_password) \
             VALUES ($1, $2, 'active', FALSE)",
            &[&user_id, &email],
        )
        .await?;
    Ok(user_id)
}

/// Find a user by email (for SSO account lookup).
pub async fn find_by_email(client: &Client, email: &str) -> anyhow::Result<Option<UserRow>> {
    let row = client
        .query_opt(
            "SELECT user_id, email, role, status, \
                    COALESCE(auth_provider, 'local') AS auth_provider, \
                    display_name, last_login_at, created_at \
             FROM dash_user WHERE LOWER(email) = LOWER($1)",
            &[&email],
        )
        .await?;
    Ok(row.map(|r| UserRow {
        user_id: r.get("user_id"),
        email: r.get("email"),
        role: r.get("role"),
        status: r.get("status"),
        auth_provider: r.get("auth_provider"),
        display_name: r.get("display_name"),
        last_login_at: r.get("last_login_at"),
        created_at: r.get("created_at"),
    }))
}

/// Link an existing local account to an SSO provider.
pub async fn link_sso_to_local(
    client: &Client,
    user_id: &Uuid,
    provider: &str,
    subject_id: &str,
    display_name: Option<&str>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE dash_user SET auth_provider = $1, sso_subject_id = $2, display_name = COALESCE($3, display_name), last_login_at = now() WHERE user_id = $4",
            &[&provider, &subject_id, &display_name, user_id],
        )
        .await?;
    Ok(())
}

/// Create a new SSO-only user (no password).
pub async fn create_sso_user(
    client: &Client,
    email: &str,
    provider: &str,
    subject_id: &str,
    display_name: Option<&str>,
) -> anyhow::Result<(Uuid, String)> {
    let user_id = Uuid::new_v4();
    let role = "viewer";
    let status = "pending"; // new SSO users require approval
    client
        .execute(
            "INSERT INTO dash_user (user_id, email, role, status, auth_provider, sso_subject_id, sso_only, display_name, must_change_password) \
             VALUES ($1, $2, $3, $4, $5, $6, TRUE, $7, FALSE)",
            &[&user_id, &email, &role, &status, &provider, &subject_id, &display_name],
        )
        .await?;
    Ok((user_id, role.to_string()))
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
