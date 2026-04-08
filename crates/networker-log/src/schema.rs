//! Database schema management for the `service_log` table.
//!
//! Provides two public async functions:
//! - [`ensure_table`] — creates the table and indexes if they do not exist.
//! - [`ensure_hypertable`] — upgrades the table to a TimescaleDB hypertable
//!   with 1-day chunks and 7-day retention policy; no-op if TimescaleDB is
//!   not available.

use tokio_postgres::Client;

/// DDL for the `service_log` table.
const CREATE_TABLE: &str = "
CREATE TABLE IF NOT EXISTS service_log (
    ts          TIMESTAMPTZ     NOT NULL DEFAULT clock_timestamp(),
    service     TEXT            NOT NULL,
    level       SMALLINT        NOT NULL,
    message     TEXT            NOT NULL,
    config_id   UUID,
    project_id  CHAR(14),
    trace_id    UUID,
    fields      JSONB
);
";

/// Indexes on columns used by common query patterns.
const CREATE_INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS service_log_service_ts_idx
         ON service_log (service, ts DESC);",
    "CREATE INDEX IF NOT EXISTS service_log_level_ts_idx
         ON service_log (level, ts DESC);",
    "CREATE INDEX IF NOT EXISTS service_log_config_id_ts_idx
         ON service_log (config_id, ts DESC)
         WHERE config_id IS NOT NULL;",
    "CREATE INDEX IF NOT EXISTS service_log_project_id_ts_idx
         ON service_log (project_id, ts DESC)
         WHERE project_id IS NOT NULL;",
];

/// Ensure the `service_log` table and all supporting indexes exist.
///
/// Safe to call multiple times — all statements use `IF NOT EXISTS`.
pub async fn ensure_table(client: &Client) -> Result<(), tokio_postgres::Error> {
    client.execute(CREATE_TABLE, &[]).await?;
    for ddl in CREATE_INDEXES {
        client.execute(*ddl, &[]).await?;
    }
    Ok(())
}

/// Optionally convert `service_log` to a TimescaleDB hypertable.
///
/// If the TimescaleDB extension is not installed this function logs a warning
/// and returns `Ok(())` without modifying the schema.  When TimescaleDB **is**
/// available the function:
///
/// 1. Ensures the extension is created in the current database.
/// 2. Converts the table to a hypertable partitioned on `ts` with 1-day chunks
///    (idempotent — errors from a pre-existing hypertable are silently ignored).
/// 3. Adds a 7-day data-retention policy via `add_retention_policy` (also
///    idempotent).
pub async fn ensure_hypertable(client: &Client) -> Result<(), tokio_postgres::Error> {
    // Check whether the TimescaleDB extension is available in pg_available_extensions.
    let row = client
        .query_opt(
            "SELECT 1 FROM pg_available_extensions WHERE name = 'timescaledb'",
            &[],
        )
        .await?;

    if row.is_none() {
        eprintln!(
            "networker-log: TimescaleDB extension not available; \
             service_log will remain a plain table"
        );
        return Ok(());
    }

    // Create the extension if it is not already present.
    client
        .execute("CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE", &[])
        .await?;

    // Convert to hypertable. The function returns an error if the table is
    // already a hypertable; catch that specific error and continue.
    let result = client
        .execute(
            "SELECT create_hypertable('service_log', 'ts', \
                   chunk_time_interval => INTERVAL '1 day', \
                   if_not_exists => TRUE)",
            &[],
        )
        .await;

    if let Err(ref e) = result {
        eprintln!("networker-log: create_hypertable warning (may already exist): {e}");
    }

    // Add a 7-day retention policy (idempotent via if_not_exists).
    let retention = client
        .execute(
            "SELECT add_retention_policy('service_log', \
                   INTERVAL '7 days', if_not_exists => TRUE)",
            &[],
        )
        .await;

    if let Err(ref e) = retention {
        eprintln!("networker-log: add_retention_policy warning: {e}");
    }

    Ok(())
}
