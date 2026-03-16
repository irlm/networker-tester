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

/// Change a user's password. Clears must_change_password flag.
pub async fn change_password(
    client: &Client,
    user_id: &Uuid,
    current_password: &str,
    new_password: &str,
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
