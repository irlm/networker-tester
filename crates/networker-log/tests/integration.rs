//! Integration test — requires PostgreSQL with TimescaleDB.
//!
//! Run:
//!   docker compose -f docker-compose.dashboard.yml up postgres -d
//!   LOGS_DB_URL="postgres://networker:networker@127.0.0.1:5432/networker_logs" \
//!     cargo test -p networker-log --test integration -- --include-ignored --nocapture

use std::sync::Arc;

/// Test the batch writer directly (bypasses tracing).
#[tokio::test]
#[ignore]
async fn batch_writer_inserts_to_db() {
    let db_url = std::env::var("LOGS_DB_URL")
        .unwrap_or_else(|_| "postgres://networker:networker@127.0.0.1:5432/networker_logs".into());

    // Create pool
    let mut cfg = deadpool_postgres::Config::new();
    cfg.url = Some(db_url);
    let pool = cfg
        .create_pool(
            Some(deadpool_postgres::Runtime::Tokio1),
            tokio_postgres::NoTls,
        )
        .unwrap();

    // Ensure table exists
    let client = pool.get().await.expect("DB connect failed");
    networker_log::schema::ensure_table(&client)
        .await
        .expect("ensure_table failed");
    drop(client);

    // Spawn batch writer
    let metrics = Arc::new(networker_log::LogPipelineMetrics::default());
    let handle = networker_log::batch::spawn_batch_writer(pool.clone(), metrics.clone());

    // Send entries through the channel
    for i in 0..3 {
        let entry = networker_log::LogEntry::new(
            "batch-test",
            networker_log::Level::Info,
            format!("Test entry {i}"),
        );
        handle
            .sender()
            .send(entry)
            .await
            .expect("channel send failed");
    }

    // Wait for flush (500ms interval + margin)
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Check metrics
    let snap = metrics.snapshot();
    eprintln!(
        "Metrics: written={} dropped={} errors={}",
        snap.entries_written, snap.entries_dropped, snap.flush_errors
    );
    assert!(
        snap.entries_written >= 3,
        "expected >=3 writes, got {}",
        snap.entries_written
    );
    assert_eq!(snap.entries_dropped, 0);

    // Verify in DB
    let client = pool.get().await.unwrap();
    let count: i64 = client
        .query_one(
            "SELECT count(*) FROM service_log WHERE service = 'batch-test'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(count >= 3, "expected >=3 rows in DB, got {count}");

    // Cleanup
    client
        .execute("DELETE FROM service_log WHERE service = 'batch-test'", &[])
        .await
        .unwrap();
}

/// Test the full LogBuilder pipeline (tracing → DbLayer → batch → DB).
#[tokio::test]
#[ignore]
async fn log_builder_end_to_end() {
    let db_url = std::env::var("LOGS_DB_URL")
        .unwrap_or_else(|_| "postgres://networker:networker@127.0.0.1:5432/networker_logs".into());

    let guard = networker_log::LogBuilder::new("e2e-test")
        .with_console(networker_log::Stream::Stderr)
        .with_db(&db_url)
        .with_context("config_id", "00000000-0000-0000-0000-000000000001")
        .init()
        .await
        .expect("failed to init logging");

    // Emit log entries via tracing macros
    tracing::info!(testbed_id = "tb-1", "Test log entry one");
    tracing::warn!("Test warning entry");
    tracing::error!(language = "rust", "Test error entry");

    // Wait for batch flush
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Check metrics
    let snap = guard.metrics().snapshot();
    eprintln!(
        "E2E Metrics: written={} dropped={} errors={}",
        snap.entries_written, snap.entries_dropped, snap.flush_errors
    );
    assert!(
        snap.entries_written >= 3,
        "expected >=3 writes, got {}",
        snap.entries_written
    );

    // Query back
    let pool = {
        let mut cfg = deadpool_postgres::Config::new();
        cfg.url = Some(std::env::var("LOGS_DB_URL").unwrap_or_else(|_| {
            "postgres://networker:networker@127.0.0.1:5432/networker_logs".into()
        }));
        cfg.create_pool(
            Some(deadpool_postgres::Runtime::Tokio1),
            tokio_postgres::NoTls,
        )
        .unwrap()
    };
    let client = pool.get().await.unwrap();

    let q = networker_log::query::LogQuery {
        service: Some("e2e-test".into()),
        min_level: None,
        config_id: None,
        project_id: None,
        search: None,
        from: chrono::Utc::now() - chrono::Duration::minutes(5),
        to: chrono::Utc::now() + chrono::Duration::minutes(1),
        limit: 10,
        offset: 0,
    };
    let result = networker_log::query::list(&client, &q).await.unwrap();
    assert!(
        result.total >= 3,
        "expected >=3 entries, got {}",
        result.total
    );

    // Cleanup
    client
        .execute("DELETE FROM service_log WHERE service = 'e2e-test'", &[])
        .await
        .unwrap();

    // Shutdown the log pipeline cleanly (DbLayer sender cleared first, then
    // BatchHandle dropped, ensuring the channel closes and the writer exits).
    guard.shutdown().await;
}
