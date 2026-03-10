/// Backward-compatible SQL Server persistence layer.
/// New code should use `output::db` directly.
#[cfg(feature = "db-mssql")]
pub async fn save(run: &crate::metrics::TestRun, connection_string: &str) -> anyhow::Result<()> {
    let backend = super::db::mssql::MssqlBackend::connect(connection_string).await?;
    use super::db::DatabaseBackend;
    backend.save(run).await
}

#[cfg(not(feature = "db-mssql"))]
pub async fn save(_run: &crate::metrics::TestRun, _connection_string: &str) -> anyhow::Result<()> {
    anyhow::bail!("SQL Server support not compiled in (enable feature 'db-mssql')")
}
