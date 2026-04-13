mod api;
mod auth;
mod benchmark_worker;
mod config;
mod crypto;
mod db;
mod deploy;
mod email;
#[allow(dead_code)]
mod project_id;
mod regression;
mod scheduler;
mod system_metrics;
mod ws;

use anyhow::Context;
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

/// Middleware that measures server processing time and exposes it
/// via the standard `Server-Timing` header (readable from JS).
async fn server_timing_middleware(
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let start = std::time::Instant::now();
    let mut response = next.run(req).await;
    let elapsed = start.elapsed();
    let ms = elapsed.as_secs_f64() * 1000.0;

    // Server-Timing header: standard, parsed natively by Chrome DevTools
    // Also add X-Process-Time-Ms for easy programmatic access
    if let Ok(v) = HeaderValue::from_str(&format!("server;dur={ms:.2}")) {
        response.headers_mut().insert("server-timing", v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("{ms:.2}")) {
        response.headers_mut().insert("x-process-time-ms", v);
    }
    response
}

/// Supervise a long-running DB-bound background loop (RR-003).
///
/// Each outer iteration:
///   1. Opens a fresh `tokio_postgres::Client` for the loop.
///   2. Spawns the connection driver task.
///   3. Spawns the loop future itself.
///   4. Uses `tokio::select!` to detect whichever side ends first — if the
///      connection dies, the loop future is aborted; if the loop returns on
///      its own, the connection task is dropped.
///   5. Backs off 5s, then reconnects.
///
/// The existing loops (`auto_shutdown_loop`, `sweep_loop`) are `async fn` that
/// return `()` and are intended to run forever, so in practice supervision
/// only fires when the connection driver task terminates (e.g. PG restart).
async fn spawn_supervised_loop<F, Fut>(name: &'static str, db_url: String, loop_fn: F)
where
    F: Fn(Arc<tokio_postgres::Client>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    loop {
        tracing::info!(supervised_loop = name, "connecting DB client");
        match tokio_postgres::connect(&db_url, tokio_postgres::NoTls).await {
            Ok((client, conn)) => {
                let client = Arc::new(client);
                let conn_task = tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        tracing::warn!(error = ?e, "supervised DB connection driver exited");
                    }
                });
                let mut loop_task = tokio::spawn(loop_fn(client));
                tokio::select! {
                    r = conn_task => {
                        tracing::warn!(
                            supervised_loop = name,
                            join = ?r,
                            "DB connection dropped; aborting loop and reconnecting"
                        );
                        loop_task.abort();
                        let _ = (&mut loop_task).await;
                    }
                    r = &mut loop_task => {
                        tracing::warn!(
                            supervised_loop = name,
                            join = ?r,
                            "loop task returned; reconnecting"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!(supervised_loop = name, error = ?e, "DB connect failed");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// A short-lived SSO exchange code entry.
pub struct SsoCodeEntry {
    pub email: String,
    pub role: String,
    pub user_id: uuid::Uuid,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct AppState {
    pub db: deadpool_postgres::Pool,
    pub logs_db: deadpool_postgres::Pool,
    pub database_url: String,
    pub jwt_secret: String,
    pub dashboard_port: u16,
    pub public_url: String,
    /// Broadcast channel for dashboard events (agent → browser fan-out).
    pub events_tx: broadcast::Sender<networker_common::messages::DashboardEvent>,
    /// Connected agents registry.
    pub agents: ws::agent_hub::AgentHub,
    /// In-process pub/sub for tester queue updates (publishers: dispatcher/
    /// scheduler; subscribers: `/ws/testers` connections).
    pub tester_queue_hub: Arc<networker_dashboard::services::tester_queue_hub::TesterQueueHub>,
    /// Spawned tester processes: agent_id → PID (so we can kill them on delete).
    pub tester_processes: RwLock<HashMap<uuid::Uuid, u32>>,
    // SSO config
    // DEPRECATED: used only for env-var-to-DB migration
    pub microsoft_client_id: Option<String>,
    // DEPRECATED: used only for env-var-to-DB migration
    pub microsoft_client_secret: Option<String>,
    // DEPRECATED: used only for env-var-to-DB migration
    pub microsoft_tenant_id: String,
    // DEPRECATED: used only for env-var-to-DB migration
    pub google_client_id: Option<String>,
    // DEPRECATED: used only for env-var-to-DB migration
    pub google_client_secret: Option<String>,
    /// Temporary SSO exchange codes: code → SsoCodeEntry
    pub sso_codes: std::sync::Mutex<HashMap<String, SsoCodeEntry>>,
    /// Cached enabled SSO providers (loaded from DB, refreshed on CRUD ops).
    pub sso_provider_cache: tokio::sync::RwLock<Vec<crate::db::sso_providers::SsoProviderRow>>,
    // Cloud account credential encryption
    pub credential_key: Option<[u8; 32]>,
    pub credential_key_old: Option<[u8; 32]>,
    // Shared report links
    pub share_base_url: String,
    pub share_max_days: u32,
    // SSE broadcast for command approval events
    pub approval_tx: broadcast::Sender<String>,
    // Workspace invite expiry
    pub invite_expiry_days: u32,
    // Log pipeline metrics (TimescaleDB-backed via networker-log)
    pub log_metrics: std::sync::Arc<networker_log::LogPipelineMetrics>,
    // Run tester-side TLS profile migrations only once per dashboard process.
    pub tls_profile_db_migrated: Mutex<bool>,
    /// PostgreSQL URL for the logs database (passed to orchestrator workers).
    pub logs_database_url: String,
    /// Instant at which the dashboard process started (used for uptime reporting).
    pub started_at: std::time::Instant,
    /// Shared cache for the latest networker-tester GitHub release version.
    /// Populated by `services::version_refresh::refresh_latest_version_loop`
    /// and read by the manual refresh handler in `api::testers`.
    pub latest_version_cache: Arc<RwLock<String>>,
}

/// Refresh the in-memory SSO provider cache from the database.
/// Called on startup and after every SSO admin CRUD operation.
pub async fn refresh_sso_cache(state: &AppState) -> anyhow::Result<()> {
    let client = state.db.get().await?;
    let providers = crate::db::sso_providers::list_enabled(&client).await?;
    let mut cache = state.sso_provider_cache.write().await;
    *cache = providers;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install ring as the default TLS crypto provider (required by reqwest/rustls).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // CLI subcommands (run locally on the server, then exit)
    let args: Vec<String> = std::env::args().collect();

    // `networker-dashboard reset-password` — reset a user's password
    if args.get(1).map(|s| s.as_str()) == Some("reset-password") {
        let _log_guard = networker_log::LogBuilder::new("dashboard")
            .with_console(networker_log::Stream::Stderr)
            .init()
            .await?;
        let db_url = std::env::var("DASHBOARD_DB_URL").unwrap_or_else(|_| {
            "postgres://networker:networker@localhost:5432/networker_core".into()
        });
        let pool = db::create_pool(&db_url).await?;
        let client = pool.get().await?;

        // Email: from arg or prompt
        let email = if let Some(e) = args.get(2) {
            e.clone()
        } else {
            eprint!("Email: ");
            std::io::Write::flush(&mut std::io::stderr())?;
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf)?;
            buf.trim().to_string()
        };

        // Verify user exists
        let row = client
            .query_opt(
                "SELECT user_id, status, is_platform_admin FROM dash_user WHERE email = $1",
                &[&email],
            )
            .await?;
        let row = match row {
            Some(r) => r,
            None => {
                eprintln!("No user found with email: {email}");
                std::process::exit(1);
            }
        };
        let status: String = row.get("status");
        let is_admin: Option<bool> = row.get("is_platform_admin");
        eprintln!(
            "User: {email}  status={status}  platform_admin={}",
            is_admin.unwrap_or(false)
        );

        // Prompt for new password
        let password = rpassword::prompt_password("New password: ")?;
        let confirm = rpassword::prompt_password("Confirm password: ")?;
        if password != confirm {
            eprintln!("Passwords do not match");
            std::process::exit(1);
        }
        if password.len() < 8 {
            eprintln!("Password must be at least 8 characters");
            std::process::exit(1);
        }

        let hash =
            bcrypt::hash(&password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;
        client
            .execute(
                "UPDATE dash_user SET password_hash = $1, must_change_password = FALSE WHERE email = $2",
                &[&hash, &email],
            )
            .await?;
        eprintln!("Password reset for {email}");
        return Ok(());
    }

    // `networker-dashboard setup` — initial admin creation
    if args.get(1).map(|s| s.as_str()) == Some("setup") {
        // Console-only logging for the interactive setup wizard (exits before server starts).
        let _setup_log_guard = networker_log::LogBuilder::new("dashboard")
            .with_console(networker_log::Stream::Stderr)
            .init()
            .await?;
        let db_url = std::env::var("DASHBOARD_DB_URL").unwrap_or_else(|_| {
            "postgres://networker:networker@localhost:5432/networker_core".into()
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

    // Initialize tracing with console + TimescaleDB backend.
    // The guard must live for the duration of the process to keep the batch writer alive.
    let _log_guard = networker_log::LogBuilder::new("dashboard")
        .with_console(networker_log::Stream::Stderr)
        .with_db(&cfg.logs_database_url)
        .init()
        .await?;

    let db_pool = db::create_pool(&cfg.database_url).await?;
    let logs_pool = match db::create_logs_pool(&cfg.logs_database_url).await {
        Ok(pool) => pool,
        Err(e) => {
            tracing::warn!(
                "Logs database unavailable ({e:#}), falling back to main database. \
                 Run scripts/migrate-to-split.sh to create the logs database."
            );
            db::create_logs_pool(&cfg.database_url).await?
        }
    };

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

    // ── Auto-migrate SSO env vars to DB (one-time) ────────────────────────
    {
        let client = db_pool
            .get()
            .await
            .context("db connection for SSO migration")?;
        if let Some(ref ms_client_id) = cfg.microsoft_client_id {
            let existing = client
                .query_opt(
                    "SELECT provider_id FROM sso_provider WHERE provider_type = 'microsoft'",
                    &[],
                )
                .await
                .ok()
                .flatten();
            if existing.is_none() {
                if let (Some(ref ms_secret), Some(ref cred_key)) =
                    (&cfg.microsoft_client_secret, &cfg.credential_key)
                {
                    let (enc, nonce) = crate::crypto::encrypt(ms_secret.as_bytes(), cred_key)?;
                    client
                        .execute(
                            "INSERT INTO sso_provider \
                             (provider_id, name, provider_type, client_id, \
                              client_secret_enc, client_secret_nonce, tenant_id) \
                             VALUES (gen_random_uuid(), 'Microsoft Entra ID', 'microsoft', $1, $2, $3, $4)",
                            &[ms_client_id, &enc, &nonce.to_vec(), &cfg.microsoft_tenant_id],
                        )
                        .await
                        .ok();
                    tracing::info!("Migrated Microsoft SSO config from env vars to database");
                }
            }
        }
        if let Some(ref g_client_id) = cfg.google_client_id {
            let existing = client
                .query_opt(
                    "SELECT provider_id FROM sso_provider WHERE provider_type = 'google'",
                    &[],
                )
                .await
                .ok()
                .flatten();
            if existing.is_none() {
                if let (Some(ref g_secret), Some(ref cred_key)) =
                    (&cfg.google_client_secret, &cfg.credential_key)
                {
                    let (enc, nonce) = crate::crypto::encrypt(g_secret.as_bytes(), cred_key)?;
                    client
                        .execute(
                            "INSERT INTO sso_provider \
                             (provider_id, name, provider_type, client_id, \
                              client_secret_enc, client_secret_nonce) \
                             VALUES (gen_random_uuid(), 'Google', 'google', $1, $2, $3)",
                            &[g_client_id, &enc, &nonce.to_vec()],
                        )
                        .await
                        .ok();
                    tracing::info!("Migrated Google SSO config from env vars to database");
                }
            }
        }
    }

    // Load SSO provider cache from DB
    let sso_providers = {
        let client = db_pool.get().await.context("db connection for SSO cache")?;
        crate::db::sso_providers::list_enabled(&client)
            .await
            .unwrap_or_default()
    };

    let (events_tx, _) = broadcast::channel(1024);
    let (approval_tx, _) = broadcast::channel(100);

    let state = Arc::new(AppState {
        db: db_pool,
        logs_db: logs_pool,
        database_url: cfg.database_url.clone(),
        jwt_secret: cfg.jwt_secret.clone(),
        dashboard_port: cfg.port,
        public_url: cfg.public_url.clone(),
        events_tx,
        agents: ws::agent_hub::AgentHub::new(),
        tester_queue_hub: Arc::new(
            networker_dashboard::services::tester_queue_hub::TesterQueueHub::new(),
        ),
        tester_processes: RwLock::new(HashMap::new()),
        microsoft_client_id: cfg.microsoft_client_id.clone(),
        microsoft_client_secret: cfg.microsoft_client_secret.clone(),
        microsoft_tenant_id: cfg.microsoft_tenant_id.clone(),
        google_client_id: cfg.google_client_id.clone(),
        google_client_secret: cfg.google_client_secret.clone(),
        sso_codes: std::sync::Mutex::new(HashMap::new()),
        sso_provider_cache: tokio::sync::RwLock::new(sso_providers),
        credential_key: cfg.credential_key,
        credential_key_old: cfg.credential_key_old,
        share_base_url: cfg.share_base_url.clone(),
        share_max_days: cfg.share_max_days,
        approval_tx,
        invite_expiry_days: cfg.invite_expiry_days,
        log_metrics: _log_guard.metrics().clone(),
        tls_profile_db_migrated: Mutex::new(false),
        logs_database_url: cfg.logs_database_url.clone(),
        started_at: std::time::Instant::now(),
        latest_version_cache: Arc::new(RwLock::new(env!("CARGO_PKG_VERSION").to_string())),
    });

    if cfg.credential_key.is_none() {
        tracing::error!(
            "DASHBOARD_CREDENTIAL_KEY could not be loaded or auto-generated — \
             cloud account management is disabled. Check file permissions on \
             /var/lib/networker/credential.key or set DASHBOARD_CREDENTIAL_KEY env var."
        );
    }

    // ── Tester persistent-lifecycle background services ──────────────────
    //
    // Each DB-bound loop runs under a per-task supervisor (`spawn_supervised_loop`)
    // that owns its own dedicated `tokio_postgres::Client`. If the connection
    // driver dies (network blip, PG restart), the supervisor logs and
    // reconnects after a short backoff instead of the loop silently running
    // against a dead handle. See RR-003.
    tracing::info!("spawning supervised tester_scheduler::auto_shutdown_loop");
    tokio::spawn(spawn_supervised_loop(
        "tester_scheduler",
        cfg.database_url.clone(),
        networker_dashboard::services::tester_scheduler::auto_shutdown_loop,
    ));

    tracing::info!("spawning supervised tester_dispatcher::sweep_loop");
    tokio::spawn(spawn_supervised_loop(
        "tester_dispatcher",
        cfg.database_url.clone(),
        networker_dashboard::services::tester_dispatcher::sweep_loop,
    ));

    // `recover_on_startup` is a one-shot that sleeps 5min then runs a single
    // scan and returns. Putting it under `spawn_supervised_loop` would cause
    // it to re-run on every reconnect, which is not what we want. Instead we
    // spawn it once with its own dedicated client; if the connection dies
    // before the scan completes, the recovery simply doesn't happen (best
    // effort — the next dashboard restart retries).
    tracing::info!("spawning tester_recovery::recover_on_startup (one-shot)");
    {
        let db_url = cfg.database_url.clone();
        tokio::spawn(async move {
            match tokio_postgres::connect(&db_url, tokio_postgres::NoTls).await {
                Ok((client, conn)) => {
                    tokio::spawn(async move {
                        if let Err(e) = conn.await {
                            tracing::warn!(error = ?e, "recovery DB connection dropped");
                        }
                    });
                    networker_dashboard::services::tester_recovery::recover_on_startup(Arc::new(
                        client,
                    ))
                    .await;
                }
                Err(e) => {
                    tracing::error!(error = ?e, "failed to connect for recover_on_startup");
                }
            }
        });
    }

    // `refresh_latest_version_loop` doesn't touch the DB — it polls GitHub
    // and updates an in-memory RwLock cache. No supervision needed.
    tracing::info!("spawning version_refresh::refresh_latest_version_loop");
    tokio::spawn(
        networker_dashboard::services::version_refresh::refresh_latest_version_loop(
            state.latest_version_cache.clone(),
        ),
    );

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
            .expose_headers([
                "server-timing".parse().unwrap(),
                "x-process-time-ms".parse().unwrap(),
            ])
    };

    let app = Router::new()
        .nest("/api", api::router(state.clone()))
        .merge(ws::router(state.clone()))
        .layer(axum::middleware::from_fn(server_timing_middleware))
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

    // Start the benchmark worker background task (polls for queued configs every 5s)
    benchmark_worker::spawn(state.clone());

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
