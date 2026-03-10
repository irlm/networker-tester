/// Database abstraction layer.
///
/// Each backend implements `DatabaseBackend` and is gated behind a Cargo feature.
/// The `connect()` factory auto-detects the backend from the URL scheme.
use crate::metrics::TestRun;
use async_trait::async_trait;

#[async_trait]
pub trait DatabaseBackend: Send + Sync {
    /// Run any pending schema migrations.
    async fn migrate(&self) -> anyhow::Result<()>;

    /// Insert a complete test run (header + attempts + sub-results).
    async fn save(&self, run: &TestRun) -> anyhow::Result<()>;

    /// Lightweight connectivity check (`SELECT 1`).
    async fn ping(&self) -> anyhow::Result<()>;
}

/// Connect to a database by URL scheme detection.
pub async fn connect(url: &str) -> anyhow::Result<Box<dyn DatabaseBackend>> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        #[cfg(feature = "db-postgres")]
        {
            return Ok(Box::new(postgres::PostgresBackend::connect(url).await?));
        }
        #[cfg(not(feature = "db-postgres"))]
        anyhow::bail!("PostgreSQL support not compiled in (enable feature 'db-postgres')");
    } else if url.starts_with("mysql://") {
        anyhow::bail!("MySQL support not yet implemented");
    } else if url.starts_with("mongodb://") || url.starts_with("mongodb+srv://") {
        anyhow::bail!("MongoDB support not yet implemented");
    } else {
        // ADO.NET-style connection string → SQL Server
        #[cfg(feature = "db-mssql")]
        {
            return Ok(Box::new(mssql::MssqlBackend::connect(url).await?));
        }
        #[cfg(not(feature = "db-mssql"))]
        anyhow::bail!("SQL Server support not compiled in (enable feature 'db-mssql')");
    }
}

#[cfg(feature = "db-mssql")]
pub mod mssql;
#[cfg(feature = "db-postgres")]
pub mod postgres;

// Shared test fixtures
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_fixtures;
