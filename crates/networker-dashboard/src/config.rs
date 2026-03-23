use std::io::Write;

/// Dashboard configuration loaded from environment variables.
/// DASHBOARD_ADMIN_PASSWORD is optional — prompted interactively or random temp password generated.
/// DASHBOARD_JWT_SECRET is required — startup fails with a helpful message if unset.
pub struct DashboardConfig {
    pub database_url: String,
    pub jwt_secret: String,
    pub admin_password: String,
    pub admin_email: Option<String>,
    pub port: u16,
    pub bind_addr: String,
    pub cors_origin: Option<String>,
    pub public_url: String,
    // SSO: Microsoft (Entra ID / Azure AD)
    pub microsoft_client_id: Option<String>,
    pub microsoft_client_secret: Option<String>,
    pub microsoft_tenant_id: String,
    // SSO: Google
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    // Email: Azure Communication Services (read by email.rs at send time;
    // stored here for config validation / future use)
    #[allow(dead_code)]
    pub acs_connection_string: Option<String>,
    #[allow(dead_code)]
    pub acs_sender: Option<String>,
    // Cloud account credential encryption
    pub credential_key: Option<[u8; 32]>,
    pub credential_key_old: Option<[u8; 32]>,
    // Shared report links
    pub share_base_url: String,
    pub share_max_days: u32,
}

impl DashboardConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let admin_password = match std::env::var("DASHBOARD_ADMIN_PASSWORD") {
            Ok(p) if !p.is_empty() => p,
            _ => {
                // Check if stdin is a TTY — if so, prompt interactively
                if atty_is_tty() {
                    prompt_password("Enter admin password for dashboard: ")?
                } else {
                    // Non-interactive: generate a random temp password
                    let temp = generate_temp_password();
                    eprintln!();
                    eprintln!("╔══════════════════════════════════════════════════════════╗");
                    eprintln!("║  Temporary admin password (change on first login):       ║");
                    eprintln!("║  {:<55}║", temp);
                    eprintln!("╚══════════════════════════════════════════════════════════╝");
                    eprintln!();
                    temp
                }
            }
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

        let admin_email = std::env::var("DASHBOARD_ADMIN_EMAIL")
            .ok()
            .filter(|s| !s.is_empty());

        let port: u16 = match std::env::var("DASHBOARD_PORT") {
            Ok(p) if !p.is_empty() => p.parse::<u16>().map_err(|e| {
                anyhow::anyhow!("DASHBOARD_PORT={p:?} is not a valid port number: {e}")
            })?,
            _ => 3000,
        };

        let public_url = std::env::var("DASHBOARD_PUBLIC_URL")
            .unwrap_or_else(|_| format!("http://localhost:{port}"));

        let share_base_url = std::env::var("DASHBOARD_SHARE_URL")
            .unwrap_or_else(|_| public_url.clone());

        let share_max_days: u32 = std::env::var("DASHBOARD_SHARE_MAX_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(365);

        let credential_key = std::env::var("DASHBOARD_CREDENTIAL_KEY")
            .ok()
            .filter(|s| s.len() == 64)
            .and_then(|s| hex::decode(&s).ok())
            .and_then(|v| <[u8; 32]>::try_from(v).ok());

        let credential_key_old = std::env::var("DASHBOARD_CREDENTIAL_KEY_OLD")
            .ok()
            .filter(|s| s.len() == 64)
            .and_then(|s| hex::decode(&s).ok())
            .and_then(|v| <[u8; 32]>::try_from(v).ok());

        Ok(Self {
            database_url: std::env::var("DASHBOARD_DB_URL").unwrap_or_else(|_| {
                "postgres://networker:networker@localhost:5432/networker_dashboard".into()
            }),
            jwt_secret,
            admin_password,
            admin_email,
            port,
            bind_addr: std::env::var("DASHBOARD_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1".into()),
            cors_origin: std::env::var("DASHBOARD_CORS_ORIGIN").ok(),
            public_url,
            microsoft_client_id: std::env::var("SSO_MICROSOFT_CLIENT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            microsoft_client_secret: std::env::var("SSO_MICROSOFT_CLIENT_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            microsoft_tenant_id: std::env::var("SSO_MICROSOFT_TENANT_ID")
                .unwrap_or_else(|_| "common".into()),
            google_client_id: std::env::var("SSO_GOOGLE_CLIENT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            google_client_secret: std::env::var("SSO_GOOGLE_CLIENT_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            acs_connection_string: std::env::var("DASHBOARD_ACS_CONNECTION_STRING")
                .ok()
                .filter(|s| !s.is_empty()),
            acs_sender: std::env::var("DASHBOARD_ACS_SENDER")
                .ok()
                .filter(|s| !s.is_empty()),
            credential_key,
            credential_key_old,
            share_base_url,
            share_max_days,
        })
    }
}

/// Check if stderr is a terminal (for deciding whether to prompt interactively).
fn atty_is_tty() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        unsafe { libc::isatty(std::io::stderr().as_raw_fd()) != 0 }
    }
    #[cfg(not(unix))]
    {
        true // Assume TTY on non-Unix (Windows will prompt)
    }
}

/// Generate a random temporary password (16 chars, alphanumeric).
fn generate_temp_password() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 24];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    // Base64-like encoding using only URL-safe chars
    let charset = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    bytes
        .iter()
        .take(16)
        .map(|b| charset[(*b as usize) % charset.len()] as char)
        .collect()
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
