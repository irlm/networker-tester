pub mod agents;
pub mod deployments;
pub mod jobs;
pub mod migrations;
pub mod runs;
pub mod users;

use deadpool_postgres::{Config, Pool, Runtime};
use tokio_postgres::NoTls;

pub async fn create_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.into());
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    // Test connectivity
    let _client = pool.get().await?;
    tracing::info!("Connected to PostgreSQL");
    Ok(pool)
}
