mod api;
mod auth;
mod config;
mod db;
mod deploy;
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

pub struct AppState {
    pub db: deadpool_postgres::Pool,
    pub database_url: String,
    pub jwt_secret: String,
    pub dashboard_port: u16,
    /// Broadcast channel for dashboard events (agent → browser fan-out).
    pub events_tx: broadcast::Sender<networker_common::messages::DashboardEvent>,
    /// Connected agents registry.
    pub agents: ws::agent_hub::AgentHub,
    /// Spawned tester processes: agent_id → PID (so we can kill them on delete).
    pub tester_processes: RwLock<HashMap<uuid::Uuid, u32>>,
    /// SSO one-time exchange codes: hash -> (user_id, expires_at)
    #[allow(clippy::type_complexity)]
    pub sso_codes: Arc<tokio::sync::RwLock<HashMap<String, (uuid::Uuid, chrono::DateTime<chrono::Utc>)>>>,
    // SSO config
    pub microsoft_client_id: Option<String>,
    pub microsoft_client_secret: Option<String>,
    pub microsoft_tenant_id: String,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub public_url: String,
    pub hide_sso_domains: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install ring as the default TLS crypto provider (required by reqwest/rustls).
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = config::DashboardConfig::from_env()?;
    let db_pool = db::create_pool(&cfg.database_url).await?;

    // Run migrations
    {
        let client = db_pool.get().await.context("db connection for migration")?;
        db::migrations::run(&client).await?;
    }

    // Seed admin user if needed (requires DASHBOARD_ADMIN_EMAIL)
    if let Some(ref email) = cfg.admin_email {
        let client = db_pool.get().await?;
        db::users::seed_admin(&client, email, &cfg.admin_password).await?;
    } else {
        let client = db_pool.get().await?;
        let count: i64 = client
            .query_one("SELECT COUNT(*) FROM dash_user", &[])
            .await?
            .get(0);
        if count == 0 {
            tracing::warn!(
                "DASHBOARD_ADMIN_EMAIL is not set and no users exist. \
                 Set this env var to create the initial admin account."
            );
        }
    }

    let (events_tx, _) = broadcast::channel(1024);

    let state = Arc::new(AppState {
        db: db_pool,
        database_url: cfg.database_url.clone(),
        jwt_secret: cfg.jwt_secret.clone(),
        dashboard_port: cfg.port,
        events_tx,
        agents: ws::agent_hub::AgentHub::new(),
        tester_processes: RwLock::new(HashMap::new()),
        sso_codes: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        microsoft_client_id: cfg.microsoft_client_id.clone(),
        microsoft_client_secret: cfg.microsoft_client_secret.clone(),
        microsoft_tenant_id: cfg.microsoft_tenant_id.clone(),
        google_client_id: cfg.google_client_id.clone(),
        google_client_secret: cfg.google_client_secret.clone(),
        public_url: cfg.public_url.clone(),
        hide_sso_domains: cfg.hide_sso_domains,
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
        let deployments = db::deployments::list(&client, 100, 0).await?;
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

    // Ensure a local tester exists and is running as a managed subprocess.
    // On startup: find or create the local-tester DB record, kill any orphaned
    // process from a previous dashboard run, then spawn a fresh subprocess.
    let local_tester_api_key = {
        let client = state.db.get().await?;
        let agents = db::agents::list(&client).await?;

        // Find existing local tester or create one
        let (agent_id, api_key) = if let Some(local) = agents
            .iter()
            .find(|a| a.provider.as_deref() == Some("local") || a.name.contains("local"))
        {
            // Get API key from DB
            let row = client
                .query_opt(
                    "SELECT api_key FROM agent WHERE agent_id = $1",
                    &[&local.agent_id],
                )
                .await?;
            let key: String = row.map(|r| r.get("api_key")).unwrap_or_default();
            tracing::info!(
                agent_id = %local.agent_id,
                "Found existing local tester"
            );
            // Reset status to offline (will go online when subprocess connects)
            db::agents::update_status(&client, &local.agent_id, "offline")
                .await
                .ok();
            (local.agent_id, key)
        } else {
            let key = format!("agent-{}", uuid::Uuid::new_v4());
            let id = db::agents::create(&client, "local-tester", &key, None, Some("local")).await?;
            tracing::info!(agent_id = %id, "Created local tester");
            (id, key)
        };

        // Kill any orphaned tester processes from previous runs
        #[cfg(unix)]
        {
            use std::process::Command as StdCommand;
            if let Ok(output) = StdCommand::new("pgrep")
                .args(["-f", "networker-agent"])
                .output()
            {
                if output.status.success() {
                    let pids = String::from_utf8_lossy(&output.stdout);
                    for pid_str in pids.trim().lines() {
                        if let Ok(pid) = pid_str.trim().parse::<i32>() {
                            // Don't kill ourselves
                            if pid != std::process::id() as i32 {
                                tracing::info!(pid, "Killing orphaned tester process");
                                unsafe { libc::kill(pid, libc::SIGTERM) };
                            }
                        }
                    }
                }
            }
        }

        // Spawn the local tester subprocess
        let dashboard_url = format!("ws://127.0.0.1:{}/ws/agent", cfg.port);
        let state_clone = state.clone();
        let api_key_clone = api_key.clone();
        tokio::spawn(async move {
            // Wait for the HTTP server to start listening
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let mut current_pid =
                deploy::agent_provisioner::spawn_local_agent(&api_key_clone, &dashboard_url).await;

            if let Some(pid) = current_pid {
                state_clone
                    .tester_processes
                    .write()
                    .await
                    .insert(agent_id, pid);
                tracing::info!(pid, "Local tester subprocess started");
            }

            // Monitor loop: check every 10s, respawn if dead
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let pid = match current_pid {
                    Some(p) => p,
                    None => {
                        // Previous spawn failed, retry
                        current_pid = deploy::agent_provisioner::spawn_local_agent(
                            &api_key_clone,
                            &dashboard_url,
                        )
                        .await;
                        if let Some(p) = current_pid {
                            state_clone
                                .tester_processes
                                .write()
                                .await
                                .insert(agent_id, p);
                        }
                        continue;
                    }
                };

                let alive = {
                    #[cfg(unix)]
                    {
                        unsafe { libc::kill(pid as i32, 0) == 0 }
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                };

                if !alive {
                    // Check if the DB record still exists (user may have deleted it)
                    let still_exists = if let Ok(client) = state_clone.db.get().await {
                        db::agents::get_by_api_key(&client, &api_key_clone)
                            .await
                            .ok()
                            .flatten()
                            .is_some()
                    } else {
                        false
                    };

                    if !still_exists {
                        tracing::info!("Local tester DB record deleted — stopping monitor");
                        state_clone.tester_processes.write().await.remove(&agent_id);
                        break;
                    }

                    tracing::warn!(pid, "Local tester subprocess died, respawning...");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    current_pid = deploy::agent_provisioner::spawn_local_agent(
                        &api_key_clone,
                        &dashboard_url,
                    )
                    .await;
                    if let Some(new_pid) = current_pid {
                        state_clone
                            .tester_processes
                            .write()
                            .await
                            .insert(agent_id, new_pid);
                        tracing::info!(pid = new_pid, "Local tester respawned");
                    }
                }
            }
        });

        api_key
    };
    let _ = local_tester_api_key; // suppress unused warning

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
