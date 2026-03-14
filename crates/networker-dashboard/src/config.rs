use std::io::Write;

/// Dashboard configuration loaded from environment variables.
/// DASHBOARD_ADMIN_PASSWORD is required — prompted interactively if not set.
pub struct DashboardConfig {
    pub database_url: String,
    pub jwt_secret: String,
    pub admin_password: String,
    pub port: u16,
}

impl DashboardConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let admin_password = match std::env::var("DASHBOARD_ADMIN_PASSWORD") {
            Ok(p) if !p.is_empty() => p,
            _ => prompt_password("Enter admin password for dashboard: ")?,
        };

        Ok(Self {
            database_url: std::env::var("DASHBOARD_DB_URL")
                .unwrap_or_else(|_| "postgres://networker:networker@localhost:5432/networker_dashboard".into()),
            jwt_secret: std::env::var("DASHBOARD_JWT_SECRET")
                .unwrap_or_else(|_| "dev-secret-change-in-production".into()),
            admin_password,
            port: std::env::var("DASHBOARD_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
        })
    }
}

/// Prompt for a password on stderr (so it works even when stdout is piped).
/// Reads from /dev/tty on Unix for interactive input.
fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;

    let password = rpassword::read_password()
        .map_err(|e| anyhow::anyhow!("Failed to read password: {e}"))?;

    if password.is_empty() {
        anyhow::bail!("Admin password cannot be empty. Set DASHBOARD_ADMIN_PASSWORD or enter it when prompted.");
    }

    Ok(password)
}
