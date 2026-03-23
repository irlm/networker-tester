mod api;
mod auth;
mod config;
mod db;
mod deploy;
mod email;
mod scheduler;
mod ws;

use anyhow::Context;
use axum::http::{HeaderValue, Method};
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

/// A short-lived SSO exchange code entry.
pub struct SsoCodeEntry {
    pub email: String,
    pub role: String,
    pub user_id: uuid::Uuid,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct AppState {
    pub db: deadpool_postgres::Pool,
    pub database_url: String,
    pub jwt_secret: String,
    pub dashboard_port: u16,
    pub public_url: String,
    /// Broadcast channel for dashboard events (agent → browser fan-out).
    pub events_tx: broadcast::Sender<networker_common::messages::DashboardEvent>,
    /// Connected agents registry.
    pub agents: ws::agent_hub::AgentHub,
    /// Spawned tester processes: agent_id → PID (so we can kill them on delete).
    pub tester_processes: RwLock<HashMap<uuid::Uuid, u32>>,
    // SSO config
    pub microsoft_client_id: Option<String>,
    pub microsoft_client_secret: Option<String>,
    pub microsoft_tenant_id: String,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    /// Temporary SSO exchange codes: code → SsoCodeEntry
    pub sso_codes: std::sync::Mutex<HashMap<String, SsoCodeEntry>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install ring as the default TLS crypto provider (required by reqwest/rustls).
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // CLI setup subcommand: `networker-dashboard setup`
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("setup") {
        let db_url = std::env::var("DASHBOARD_DB_URL").unwrap_or_else(|_| {
            "postgres://networker:networker@localhost:5432/networker_dashboard".into()
        });

        let pool = db::create_pool(&db_url).await?;
        let client = pool.get().await?;
        db::migrations::run(&client).await?;

        // Prompt for admin email
        eprint!("Admin email: ");
        std::io::Write::flush(&mut std::io::stderr())?;
        let mut email = String::new();
        std::io::stdin().read_line(&mut email)?;
        let email = email.trim();

        // Prompt for password
        let password = rpassword::prompt_password("Admin password: ")?;
        let confirm = rpassword::prompt_password("Confirm password: ")?;
        if password != confirm {
            eprintln!("Passwords do not match");
            std::process::exit(1);
        }
        if password.len() < 8 {
            eprintln!("Password must be at least 8 characters");
            std::process::exit(1);
        }

        db::users::seed_admin(&client, email, &password).await?;
        eprintln!("Admin user created: {email}");
        eprintln!("Start the dashboard with:");
        eprintln!(
            "  DASHBOARD_JWT_SECRET=<secret> DASHBOARD_ADMIN_EMAIL={email} networker-dashboard"
        );
        return Ok(());
    }

    let cfg = config::DashboardConfig::from_env()?;
    let db_pool = db::create_pool(&cfg.database_url).await?;

    // Run migrations
    {
        let client = db_pool.get().await.context("db connection for migration")?;
        db::migrations::run(&client).await?;
    }

    // Check if setup is needed: no users and no DASHBOARD_ADMIN_EMAIL
    let needs_setup = {
        let client = db_pool.get().await?;
        let count: i64 = client
            .query_one("SELECT COUNT(*) FROM dash_user", &[])
            .await?
            .get(0);
        count == 0 && cfg.admin_email.is_none()
    };

    if needs_setup {
        tracing::error!("DASHBOARD_ADMIN_EMAIL is not set and no users exist. Cannot start.");
        tracing::error!("Set DASHBOARD_ADMIN_EMAIL env var, or run: networker-dashboard setup");
        // Serve a static error page on all routes
        let app = axum::Router::new()
            .fallback(|| async { axum::response::Html(include_str!("config_error.html")) });
        let addr = format!("{}:{}", cfg.bind_addr, cfg.port);
        tracing::info!("Serving setup-required page on {addr}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;
        return Ok(());
    }

    // Seed admin user if needed (requires DASHBOARD_ADMIN_EMAIL)
    if let Some(ref email) = cfg.admin_email {
        let client = db_pool.get().await?;
        db::users::seed_admin(&client, email, &cfg.admin_password).await?;
    }

    let (events_tx, _) = broadcast::channel(1024);

    let state = Arc::new(AppState {
        db: db_pool,
        database_url: cfg.database_url.clone(),
        jwt_secret: cfg.jwt_secret.clone(),
        dashboard_port: cfg.port,
        public_url: cfg.public_url.clone(),
        events_tx,
        agents: ws::agent_hub::AgentHub::new(),
        tester_processes: RwLock::new(HashMap::new()),
        microsoft_client_id: cfg.microsoft_client_id.clone(),
        microsoft_client_secret: cfg.microsoft_client_secret.clone(),
        microsoft_tenant_id: cfg.microsoft_tenant_id.clone(),
        google_client_id: cfg.google_client_id.clone(),
        google_client_secret: cfg.google_client_secret.clone(),
        sso_codes: std::sync::Mutex::new(HashMap::new()),
    });

    let cors = {
        let origin = cfg
            .cors_origin
            .as_deref()
            .unwrap_or("http://localhost:5173");
        CorsLayer::new()
            .allow_origin(
                origin
                    .parse::<HeaderValue>()
                    .context("invalid DASHBOARD_CORS_ORIGIN value")?,
            )
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
            ])
    };

    let app = Router::new()
        .nest("/api", api::router(state.clone()))
        .merge(ws::router(state.clone()))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors);

    // Serve static frontend files (React SPA).
    // In production, DASHBOARD_STATIC_DIR points to the built frontend assets.
    // Falls back to index.html for SPA client-side routing.
    let static_dir =
        std::env::var("DASHBOARD_STATIC_DIR").unwrap_or_else(|_| "./dashboard/dist".into());
    let app = if std::path::Path::new(&static_dir).exists() {
        app.fallback_service(
            ServeDir::new(&static_dir).fallback(ServeFile::new(format!("{static_dir}/index.html"))),
        )
    } else {
        app
    };

    // Start the scheduler background task (checks for due schedules every 30s)
    scheduler::spawn(state.clone());

    // Resolve bare IPs to FQDNs for existing deployments
    {
        let client = state.db.get().await?;
        let deployments = db::deployments::list_all(&client, 100, 0).await?;
        for dep in &deployments {
            if let Some(ref ips_val) = dep.endpoint_ips {
                let ips: Vec<String> = serde_json::from_value(ips_val.clone()).unwrap_or_default();
                let mut updated = false;
                let mut new_ips = ips.clone();
                for (i, ip) in ips.iter().enumerate() {
                    // Skip if already a hostname
                    if ip.parse::<std::net::IpAddr>().is_err() {
                        continue;
                    }
                    // Try to get FQDN from the deployment config
                    if let Some(fqdn) = resolve_ip_to_fqdn(ip, &dep.config).await {
                        new_ips[i] = fqdn;
                        updated = true;
                    }
                }
                if updated {
                    let new_val = serde_json::to_value(&new_ips).unwrap_or_default();
                    db::deployments::set_endpoint_ips(&client, &dep.deployment_id, &new_val)
                        .await
                        .ok();
                    tracing::info!(
                        deployment_id = %dep.deployment_id,
                        endpoints = ?new_ips,
                        "Updated deployment endpoints with FQDNs"
                    );
                }
            }
        }
    }

    let addr = format!("{}:{}", cfg.bind_addr, cfg.port);
    tracing::info!("Dashboard listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Shutdown signal received, draining connections...");
        })
        .await?;

    // Cleanup: kill managed tester processes on shutdown
    {
        let processes = state.tester_processes.read().await;
        for (_, pid) in processes.iter() {
            tracing::info!(pid, "Killing managed tester on shutdown");
            #[cfg(unix)]
            unsafe {
                libc::kill(*pid as i32, libc::SIGTERM);
            }
        }
    }

    Ok(())
}

/// Try to resolve a bare IP to an FQDN using the deployment config and endpoint /info.
async fn resolve_ip_to_fqdn(ip: &str, config: &serde_json::Value) -> Option<String> {
    // Strategy 1: Try endpoint /info to get hostname + construct FQDN from provider
    let info_url = format!("https://{ip}:8443/info");
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    if let Ok(resp) = client.get(&info_url).send().await {
        if let Ok(info) = resp.json::<serde_json::Value>().await {
            // Best case: endpoint reports its own public_dns
            let public_dns = info
                .get("system")
                .and_then(|s| s.get("public_dns"))
                .and_then(|d| d.as_str());
            if let Some(dns) = public_dns {
                if !dns.is_empty() {
                    return Some(dns.to_string());
                }
            }

            // Use public_ip from /info for more reliable AWS FQDN construction
            let public_ip = info
                .get("system")
                .and_then(|s| s.get("public_ip"))
                .and_then(|d| d.as_str());

            let hostname = info
                .get("system")
                .and_then(|s| s.get("hostname"))
                .and_then(|h| h.as_str());
            let region_str = info
                .get("region")
                .or_else(|| info.get("system").and_then(|s| s.get("region")))
                .and_then(|r| r.as_str())
                .unwrap_or("");

            // Azure: use hostname from /info + region
            if region_str.starts_with("azure/") {
                if let Some(host) = hostname {
                    let azure_region = region_str.strip_prefix("azure/").unwrap_or("eastus");
                    return Some(format!("{host}.{azure_region}.cloudapp.azure.com"));
                }
            }

            // AWS: construct public DNS from IP (internal hostname is useless)
            // GCP: use hostname from /info
            if let Some(endpoints) = config.get("endpoints").and_then(|e| e.as_array()) {
                for ep in endpoints {
                    let provider = ep.get("provider").and_then(|p| p.as_str()).unwrap_or("");
                    match provider {
                        "aws" => {
                            let region = ep
                                .get("aws")
                                .and_then(|a| a.get("region"))
                                .and_then(|r| r.as_str())
                                .unwrap_or("us-east-1");
                            // Prefer public_ip from /info over the IP passed to us
                            let resolved_ip = public_ip.unwrap_or(ip);
                            let ip_dashed = resolved_ip.replace('.', "-");
                            if region == "us-east-1" {
                                return Some(format!("ec2-{ip_dashed}.compute-1.amazonaws.com"));
                            } else {
                                return Some(format!(
                                    "ec2-{ip_dashed}.{region}.compute.amazonaws.com"
                                ));
                            }
                        }
                        "gcp" => {
                            if let Some(host) = hostname {
                                return Some(host.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    None
}
