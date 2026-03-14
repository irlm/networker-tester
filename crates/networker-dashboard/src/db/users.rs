use tokio_postgres::Client;
use uuid::Uuid;

/// Seed the admin user if no users exist.
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
                "INSERT INTO dash_user (user_id, username, password_hash, role) VALUES ($1, $2, $3, $4)",
                &[&Uuid::new_v4(), &"admin", &hash, &"admin"],
            )
            .await?;
        tracing::info!("Seeded admin user (username: admin)");
    }
    Ok(())
}

/// Authenticate a user by username and password. Returns (user_id, role) on success.
pub async fn authenticate(
    client: &Client,
    username: &str,
    password: &str,
) -> anyhow::Result<Option<(Uuid, String)>> {
    let row = client
        .query_opt(
            "SELECT user_id, password_hash, role, disabled FROM dash_user WHERE username = $1",
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
                // Update last login
                client
                    .execute(
                        "UPDATE dash_user SET last_login_at = now() WHERE user_id = $1",
                        &[&user_id],
                    )
                    .await?;
                Ok(Some((user_id, role)))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}
