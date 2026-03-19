pub mod agents;
pub mod deployments;
pub mod jobs;
pub mod migrations;
pub mod runs;
pub mod url_tests;
pub mod users;

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
