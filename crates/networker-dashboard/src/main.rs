mod api;
mod auth;
mod config;
mod db;
mod ws;

use anyhow::Context;
use axum::Router;
use axum::http::{HeaderValue, Method};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub db: deadpool_postgres::Pool,
    pub jwt_secret: String,
    /// Broadcast channel for dashboard events (agent → browser fan-out).
    pub events_tx: broadcast::Sender<networker_common::messages::DashboardEvent>,
    /// Connected agents registry.
    pub agents: ws::agent_hub::AgentHub,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    // Seed admin user if needed
    {
        let client = db_pool.get().await?;
        db::users::seed_admin(&client, &cfg.admin_password).await?;
    }

    let (events_tx, _) = broadcast::channel(1024);

    let state = Arc::new(AppState {
        db: db_pool,
        jwt_secret: cfg.jwt_secret.clone(),
        events_tx,
        agents: ws::agent_hub::AgentHub::new(),
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

    let addr = format!("{}:{}", cfg.bind_addr, cfg.port);
    tracing::info!("Dashboard listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Shutdown signal received, draining connections...");
        })
        .await?;

    Ok(())
}
