pub mod agent_commands;
pub mod agents;
pub mod benchmark_configs;
pub mod benchmark_presets;
pub mod benchmark_progress;
pub mod benchmark_testbeds;
pub mod benchmark_vm_catalog;
pub mod benchmarks;
pub mod cloud_accounts;
pub mod cloud_connections;
pub mod command_approvals;
pub mod deployments;
pub mod invites;
pub mod jobs;
pub mod migrations;
pub mod perf_log;
pub mod project_testers;
pub mod projects;
pub mod runs;
pub mod schedules;
pub mod share_links;
pub mod sso_providers;
pub mod system_health;
pub mod tls_profiles;
pub mod url_tests;
pub mod users;
pub mod visibility;
pub mod vm_lifecycle;
pub mod workspace_warnings;
pub mod zones;

use deadpool_postgres::{Config, Pool, Runtime};
use tokio_postgres::NoTls;

pub async fn create_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.into());
    cfg.pool = Some(deadpool_postgres::PoolConfig {
        max_size: 16,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(std::time::Duration::from_secs(5)),
            create: Some(std::time::Duration::from_secs(5)),
            recycle: Some(std::time::Duration::from_secs(5)),
        },
        ..Default::default()
    });
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    // Test connectivity
    let _client = pool.get().await?;
    tracing::info!("Connected to PostgreSQL");
    Ok(pool)
}

/// Create a connection pool for the logs database (smaller pool, same timeouts).
pub async fn create_logs_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.into());
    cfg.pool = Some(deadpool_postgres::PoolConfig {
        max_size: 8,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(std::time::Duration::from_secs(5)),
            create: Some(std::time::Duration::from_secs(5)),
            recycle: Some(std::time::Duration::from_secs(5)),
        },
        ..Default::default()
    });
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    let _client = pool.get().await?;
    tracing::info!("Connected to logs PostgreSQL");
    Ok(pool)
}
