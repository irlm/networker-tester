/// Database abstraction layer.
///
/// Each backend implements `DatabaseBackend` and is gated behind a Cargo feature.
/// The `connect()` factory auto-detects the backend from the URL scheme.
use crate::metrics::{TestRun, UrlTestRun};
use crate::tls_profile::TlsEndpointProfile;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait DatabaseBackend: Send + Sync {
    /// Run any pending schema migrations.
    async fn migrate(&self) -> anyhow::Result<()>;

    /// Insert a complete test run (header + attempts + sub-results).
    async fn save(&self, run: &TestRun) -> anyhow::Result<()>;

    /// Insert a URL page-load diagnostic run and related child records.
    async fn save_url_test(&self, run: &UrlTestRun) -> anyhow::Result<()>;

    /// Insert a TLS endpoint profile run.
    async fn save_tls_profile(
        &self,
        run: &TlsEndpointProfile,
        project_id: Option<&str>,
    ) -> anyhow::Result<Uuid>;

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

#[cfg(test)]
mod tests {
    // ── URL-scheme detection tests ─────────────────────────────────────────────
    //
    // These tests exercise `connect()` using unsupported or not-compiled-in
    // URL schemes.  All tests run without a live database connection.
    //
    // Note: `Box<dyn DatabaseBackend>` does not implement Debug, so we cannot
    // use `unwrap_err()` / `expect_err()`.  We match on the Result instead.

    /// Helper: assert a Result is Err and return the error message.
    fn require_err<T>(r: anyhow::Result<T>) -> String {
        match r {
            Ok(_) => panic!("expected Err, got Ok"),
            Err(e) => e.to_string(),
        }
    }

    /// `mysql://` URLs should always bail with "not yet implemented".
    #[tokio::test]
    async fn connect_mysql_url_not_implemented() {
        let msg = require_err(super::connect("mysql://user:pass@localhost/db").await);
        assert!(
            msg.contains("not yet implemented"),
            "expected 'not yet implemented', got: {msg}"
        );
    }

    /// `mongodb://` URLs should always bail with "not yet implemented".
    #[tokio::test]
    async fn connect_mongodb_url_not_implemented() {
        let msg = require_err(super::connect("mongodb://localhost/db").await);
        assert!(
            msg.contains("not yet implemented"),
            "expected 'not yet implemented', got: {msg}"
        );
    }

    /// `mongodb+srv://` (Atlas-style) URLs should also bail with "not yet implemented".
    #[tokio::test]
    async fn connect_mongodb_srv_url_not_implemented() {
        let msg =
            require_err(super::connect("mongodb+srv://user:pass@cluster.mongodb.net/db").await);
        assert!(
            msg.contains("not yet implemented"),
            "expected 'not yet implemented', got: {msg}"
        );
    }

    /// `postgres://` URLs when the `db-postgres` feature is NOT compiled in
    /// should bail with a message that mentions "not compiled in".
    #[cfg(not(feature = "db-postgres"))]
    #[tokio::test]
    async fn connect_postgres_url_not_compiled_in() {
        let msg = require_err(super::connect("postgres://localhost/db").await);
        assert!(
            msg.contains("not compiled in"),
            "expected 'not compiled in', got: {msg}"
        );
        assert!(
            msg.contains("db-postgres"),
            "error should name the missing feature, got: {msg}"
        );
    }

    /// Same as above for the `postgresql://` scheme alias.
    #[cfg(not(feature = "db-postgres"))]
    #[tokio::test]
    async fn connect_postgresql_scheme_alias_not_compiled_in() {
        let msg = require_err(super::connect("postgresql://localhost/db").await);
        assert!(
            msg.contains("not compiled in"),
            "expected 'not compiled in', got: {msg}"
        );
        assert!(
            msg.contains("db-postgres"),
            "error should name the missing feature, got: {msg}"
        );
    }

    /// An ADO.NET-style connection string when `db-mssql` feature is NOT
    /// compiled in should bail with a message about "not compiled in".
    ///
    /// When the feature IS compiled in, we get a TCP connection error to the
    /// (non-existent) server — which is also an error, just a different one.
    #[cfg(not(feature = "db-mssql"))]
    #[tokio::test]
    async fn connect_ado_string_not_compiled_in() {
        let msg =
            require_err(super::connect("Server=localhost;Database=D;User Id=sa;Password=P;").await);
        assert!(
            msg.contains("not compiled in"),
            "expected 'not compiled in', got: {msg}"
        );
        assert!(
            msg.contains("db-mssql"),
            "error should name the missing feature, got: {msg}"
        );
    }

    /// Ensure that the URL-scheme branching is case-sensitive and that
    /// `POSTGRES://` (uppercase) does NOT match the postgres branch.
    /// It should fall through to the SQL Server branch (or "not compiled in").
    ///
    /// This is a deliberate design decision: URL schemes are defined as lowercase.
    #[tokio::test]
    async fn connect_uppercase_postgres_does_not_match_postgres_branch() {
        // Regardless of which features are compiled, an uppercase scheme must
        // not silently succeed against a non-existent server — it should either
        // return a "not compiled in" error (mssql branch, feature absent) or a
        // TCP connection error (mssql branch, feature present).  Either way it
        // must be an Err, not Ok.
        assert!(
            super::connect("POSTGRES://localhost/db").await.is_err(),
            "uppercase scheme should not successfully connect"
        );
    }

    /// `http://` is not a recognised scheme — falls through to the SQL Server
    /// branch, which either says "not compiled in" or fails to parse the ADO.NET
    /// string.  Either way: must be Err.
    #[tokio::test]
    async fn connect_unrecognised_scheme_returns_error() {
        assert!(
            super::connect("http://localhost/db").await.is_err(),
            "unrecognised scheme should return an error"
        );
    }
}
