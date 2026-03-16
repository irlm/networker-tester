use std::io::Write;

/// Dashboard configuration loaded from environment variables.
/// DASHBOARD_ADMIN_PASSWORD is required — prompted interactively if not set.
/// DASHBOARD_JWT_SECRET is required — startup fails with a helpful message if unset.
pub struct DashboardConfig {
    pub database_url: String,
    pub jwt_secret: String,
    pub admin_password: String,
    pub port: u16,
    pub bind_addr: String,
    pub cors_origin: Option<String>,
}

impl DashboardConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let admin_password = match std::env::var("DASHBOARD_ADMIN_PASSWORD") {
            Ok(p) if !p.is_empty() => p,
            _ => prompt_password("Enter admin password for dashboard: ")?,
        };

        let jwt_secret = std::env::var("DASHBOARD_JWT_SECRET").map_err(|_| {
            anyhow::anyhow!(
                "DASHBOARD_JWT_SECRET must be set. Generate one with: openssl rand -base64 32"
            )
        })?;
        if jwt_secret.len() < 32 {
            tracing::warn!(
                "DASHBOARD_JWT_SECRET is shorter than 32 bytes — consider using a longer secret"
            );
        }

        Ok(Self {
            database_url: std::env::var("DASHBOARD_DB_URL").unwrap_or_else(|_| {
                "postgres://networker:networker@localhost:5432/networker_dashboard".into()
            }),
            jwt_secret,
            admin_password,
            port: match std::env::var("DASHBOARD_PORT") {
                Ok(p) if !p.is_empty() => p.parse::<u16>().map_err(|e| {
                    anyhow::anyhow!("DASHBOARD_PORT={p:?} is not a valid port number: {e}")
                })?,
                _ => 3000,
            },
            bind_addr: std::env::var("DASHBOARD_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1".into()),
            cors_origin: std::env::var("DASHBOARD_CORS_ORIGIN").ok(),
        })
    }
}

/// Prompt for a password on stderr (so it works even when stdout is piped).
/// Reads from /dev/tty on Unix for interactive input.
fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;

    let password =
        rpassword::read_password().map_err(|e| anyhow::anyhow!("Failed to read password: {e}"))?;

    if password.is_empty() {
        anyhow::bail!("Admin password cannot be empty. Set DASHBOARD_ADMIN_PASSWORD or enter it when prompted.");
    }

    Ok(password)
}
