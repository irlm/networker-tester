use crate::db::users::UserRow;
use tokio_postgres::Client;
use uuid::Uuid;

/// Find a user by SSO provider and subject ID.
pub async fn find_by_sso(
    client: &Client,
    provider: &str,
    subject_id: &str,
) -> anyhow::Result<Option<UserRow>> {
    let row = client
        .query_opt(
            "SELECT user_id, email, role, status, \
                    COALESCE(auth_provider, 'local') AS auth_provider, \
                    display_name, last_login_at, created_at \
             FROM dash_user \
             WHERE auth_provider = $1 AND sso_subject_id = $2",
            &[&provider, &subject_id],
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

/// Find a user by email (for account linking).
pub async fn find_by_email(client: &Client, email: &str) -> anyhow::Result<Option<UserRow>> {
    let row = client
        .query_opt(
            "SELECT user_id, email, role, status, \
                    COALESCE(auth_provider, 'local') AS auth_provider, \
                    display_name, last_login_at, created_at \
             FROM dash_user WHERE email = $1",
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

/// Create a new SSO user with status='pending' and no password.
pub async fn create_sso_user(
    client: &Client,
    email: &str,
    provider: &str,
    subject_id: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) -> anyhow::Result<Uuid> {
    let user_id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO dash_user (user_id, email, role, status, auth_provider, sso_subject_id, \
             display_name, avatar_url, must_change_password) \
             VALUES ($1, $2, 'viewer', 'pending', $3, $4, $5, $6, FALSE)",
            &[
                &user_id,
                &email,
                &provider,
                &subject_id,
                &display_name,
                &avatar_url,
            ],
        )
        .await?;
    tracing::info!(
        user_id = %user_id,
        email = %email,
        provider = %provider,
        "Created new SSO user (status: pending)"
    );
    Ok(user_id)
}

/// Link an existing local account to an SSO provider.
pub async fn link_sso_to_local(
    client: &Client,
    user_id: &Uuid,
    provider: &str,
    subject_id: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE dash_user SET auth_provider = $1, sso_subject_id = $2 WHERE user_id = $3",
            &[&provider, &subject_id, user_id],
        )
        .await?;
    tracing::info!(
        user_id = %user_id,
        provider = %provider,
        "Linked SSO provider to existing local account"
    );
    Ok(())
}

/// Update last_login_at for a user.
pub async fn update_last_login(client: &Client, user_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE dash_user SET last_login_at = now() WHERE user_id = $1",
            &[user_id],
        )
        .await?;
    Ok(())
}
