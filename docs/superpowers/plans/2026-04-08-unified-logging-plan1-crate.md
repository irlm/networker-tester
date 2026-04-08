# Unified Logging — Plan 1: `networker-log` Crate + Infrastructure

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the shared `networker-log` crate with LogBuilder API, console layer, batched DB layer writing to TimescaleDB, pipeline metrics, and schema migration.

**Architecture:** A new workspace crate `crates/networker-log` that composes tracing layers via a builder. The DB layer collects log entries in an mpsc channel and flushes them in batches to a `service_log` TimescaleDB hypertable. Pipeline health is tracked via atomic counters.

**Tech Stack:** Rust, tracing/tracing-subscriber, tokio, deadpool-postgres/tokio-postgres, TimescaleDB (PostgreSQL extension)

**Spec:** `docs/superpowers/specs/2026-04-08-unified-logging-design.md`

---

## File Structure

```
crates/networker-log/
  Cargo.toml
  src/
    lib.rs              — public API: LogBuilder, Stream, LogGuard, LogPipelineMetrics
    builder.rs          — LogBuilder implementation, composes Registry + layers
    db_layer.rs         — tracing Layer that sends entries to the batch writer
    batch.rs            — background task: receives entries via channel, flushes to DB
    schema.rs           — service_log table creation + TimescaleDB hypertable setup
    query.rs            — query functions for dashboard API (list, stats, pipeline-status)
    metrics.rs          — LogPipelineMetrics (atomic counters)
    types.rs            — LogEntry struct, Level mapping

Modify:
  Cargo.toml                     — add networker-log to workspace members + dependencies
  docker-compose.dashboard.yml   — switch to timescale/timescaledb-ha:pg16.6-ts2.17.2
  scripts/init-logs-db.sql       — add service_log table + hypertable + retention policy
```

---

### Task 1: Create crate skeleton with types and metrics

**Files:**
- Create: `crates/networker-log/Cargo.toml`
- Create: `crates/networker-log/src/lib.rs`
- Create: `crates/networker-log/src/types.rs`
- Create: `crates/networker-log/src/metrics.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/networker-log/Cargo.toml
[package]
name = "networker-log"
version.workspace = true
edition.workspace = true

[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "registry"] }
tokio = { version = "1", features = ["sync", "rt", "time"] }
tokio-postgres = { version = "0.7", features = ["with-chrono-0_4", "with-uuid-1", "with-serde_json-1"] }
deadpool-postgres = { version = "0.14", features = ["serde"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
```

- [ ] **Step 2: Create types.rs**

```rust
// crates/networker-log/src/types.rs
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Severity level, maps to tracing levels. Stored as SMALLINT in DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i16)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    pub fn from_tracing(level: &tracing::Level) -> Self {
        match *level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::TRACE => Self::Trace,
        }
    }

    pub fn as_i16(self) -> i16 {
        self as i16
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }
}

/// A single log entry ready for DB insertion.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ts: DateTime<Utc>,
    pub service: String,
    pub level: Level,
    pub message: String,
    pub config_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub trace_id: Option<Uuid>,
    pub fields: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_from_tracing_maps_correctly() {
        assert_eq!(Level::from_tracing(&tracing::Level::ERROR).as_i16(), 1);
        assert_eq!(Level::from_tracing(&tracing::Level::WARN).as_i16(), 2);
        assert_eq!(Level::from_tracing(&tracing::Level::INFO).as_i16(), 3);
        assert_eq!(Level::from_tracing(&tracing::Level::DEBUG).as_i16(), 4);
        assert_eq!(Level::from_tracing(&tracing::Level::TRACE).as_i16(), 5);
    }

    #[test]
    fn level_ordering() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
    }

    #[test]
    fn level_from_str_loose_case_insensitive() {
        assert_eq!(Level::from_str_loose("ERROR"), Some(Level::Error));
        assert_eq!(Level::from_str_loose("warning"), Some(Level::Warn));
        assert_eq!(Level::from_str_loose("unknown"), None);
    }
}
```

- [ ] **Step 3: Create metrics.rs**

```rust
// crates/networker-log/src/metrics.rs
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Pipeline health counters. Shared between DbLayer and the metrics API.
#[derive(Debug, Default)]
pub struct LogPipelineMetrics {
    pub entries_written: AtomicU64,
    pub entries_dropped: AtomicU64,
    pub flush_count: AtomicU64,
    pub flush_errors: AtomicU64,
    pub last_flush_ms: AtomicU64,
    pub queue_depth: AtomicU32,
}

impl LogPipelineMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            entries_written: self.entries_written.load(Ordering::Relaxed),
            entries_dropped: self.entries_dropped.load(Ordering::Relaxed),
            flush_count: self.flush_count.load(Ordering::Relaxed),
            flush_errors: self.flush_errors.load(Ordering::Relaxed),
            last_flush_ms: self.last_flush_ms.load(Ordering::Relaxed),
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot of pipeline metrics (serializable).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub entries_written: u64,
    pub entries_dropped: u64,
    pub flush_count: u64,
    pub flush_errors: u64,
    pub last_flush_ms: u64,
    pub queue_depth: u32,
}

impl MetricsSnapshot {
    pub fn status(&self) -> &'static str {
        if self.flush_errors > 0 && self.last_flush_ms > 5000 {
            "failing"
        } else if self.entries_dropped > 0 {
            "degraded"
        } else {
            "healthy"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_status_healthy() {
        let m = LogPipelineMetrics::new();
        assert_eq!(m.snapshot().status(), "healthy");
    }

    #[test]
    fn snapshot_status_degraded_on_drops() {
        let m = LogPipelineMetrics::new();
        m.entries_dropped.store(5, Ordering::Relaxed);
        assert_eq!(m.snapshot().status(), "degraded");
    }

    #[test]
    fn snapshot_status_failing_on_errors() {
        let m = LogPipelineMetrics::new();
        m.flush_errors.store(1, Ordering::Relaxed);
        m.last_flush_ms.store(6000, Ordering::Relaxed);
        assert_eq!(m.snapshot().status(), "failing");
    }
}
```

- [ ] **Step 4: Create lib.rs with public exports**

```rust
// crates/networker-log/src/lib.rs
pub mod types;
pub mod metrics;

pub use metrics::{LogPipelineMetrics, MetricsSnapshot};
pub use types::{Level, LogEntry};
```

- [ ] **Step 5: Add to workspace**

In `Cargo.toml` (root), add `"crates/networker-log"` to the `members` array.

- [ ] **Step 6: Build and test**

Run: `cargo build -p networker-log && cargo test -p networker-log`
Expected: builds, 6 tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/networker-log/ Cargo.toml Cargo.lock
git commit -m "feat(log): networker-log crate skeleton with types and metrics"
```

---

### Task 2: Batch writer (channel → DB flush)

**Files:**
- Create: `crates/networker-log/src/batch.rs`
- Create: `crates/networker-log/src/schema.rs`
- Modify: `crates/networker-log/src/lib.rs`

- [ ] **Step 1: Create schema.rs with table creation**

```rust
// crates/networker-log/src/schema.rs
use tokio_postgres::Client;

/// SQL to create the service_log table. Safe to run repeatedly (IF NOT EXISTS).
/// TimescaleDB hypertable conversion is separate — call ensure_hypertable() after.
const CREATE_TABLE: &str = r#"
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

CREATE INDEX IF NOT EXISTS ix_service_log_service ON service_log (service, ts DESC);
CREATE INDEX IF NOT EXISTS ix_service_log_level   ON service_log (level, ts DESC);
CREATE INDEX IF NOT EXISTS ix_service_log_config  ON service_log (config_id, ts DESC) WHERE config_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_service_log_project ON service_log (project_id, ts DESC) WHERE project_id IS NOT NULL;
"#;

/// Create the service_log table (idempotent).
pub async fn ensure_table(client: &Client) -> anyhow::Result<()> {
    client.batch_execute(CREATE_TABLE).await?;
    Ok(())
}

/// Convert service_log to a TimescaleDB hypertable with 1-day chunks.
/// No-op if already a hypertable or if TimescaleDB is not installed.
pub async fn ensure_hypertable(client: &Client) -> anyhow::Result<()> {
    // Check if TimescaleDB extension is available
    let ext = client
        .query_opt(
            "SELECT 1 FROM pg_extension WHERE extname = 'timescaledb'",
            &[],
        )
        .await?;

    if ext.is_none() {
        // Try to create the extension (may fail if not installed)
        if let Err(e) = client
            .batch_execute("CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE")
            .await
        {
            tracing::warn!("TimescaleDB not available ({e}), using plain table");
            return Ok(());
        }
    }

    // Check if already a hypertable
    let is_hyper = client
        .query_opt(
            "SELECT 1 FROM timescaledb_information.hypertables WHERE hypertable_name = 'service_log'",
            &[],
        )
        .await?;

    if is_hyper.is_none() {
        client
            .batch_execute(
                "SELECT create_hypertable('service_log', 'ts', chunk_time_interval => INTERVAL '1 day', if_not_exists => TRUE)",
            )
            .await?;
        client
            .batch_execute(
                "SELECT add_retention_policy('service_log', INTERVAL '7 days', if_not_exists => TRUE)",
            )
            .await?;
        tracing::info!("service_log converted to TimescaleDB hypertable with 7-day retention");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Integration tests require a running PostgreSQL — tested in Task 6
}
```

- [ ] **Step 2: Create batch.rs**

```rust
// crates/networker-log/src/batch.rs
use crate::metrics::LogPipelineMetrics;
use crate::types::LogEntry;
use deadpool_postgres::Pool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const BATCH_SIZE: usize = 100;
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const CHANNEL_CAPACITY: usize = 10_000;

/// Handle returned by spawn_batch_writer. Send log entries through the sender.
pub struct BatchHandle {
    pub tx: mpsc::Sender<LogEntry>,
    task: tokio::task::JoinHandle<()>,
}

impl BatchHandle {
    /// Flush remaining entries and stop the background task.
    pub async fn shutdown(self) {
        drop(self.tx); // close channel — writer loop exits
        let _ = self.task.await;
    }
}

/// Spawn the background batch writer. Returns a channel sender + join handle.
pub fn spawn_batch_writer(
    pool: Pool,
    metrics: Arc<LogPipelineMetrics>,
) -> BatchHandle {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let task = tokio::spawn(batch_writer_loop(rx, pool, metrics));
    BatchHandle { tx, task }
}

async fn batch_writer_loop(
    mut rx: mpsc::Receiver<LogEntry>,
    pool: Pool,
    metrics: Arc<LogPipelineMetrics>,
) {
    let mut buf: Vec<LogEntry> = Vec::with_capacity(BATCH_SIZE);
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            entry = rx.recv() => {
                match entry {
                    Some(e) => {
                        buf.push(e);
                        metrics.queue_depth.store(buf.len() as u32, Ordering::Relaxed);
                        if buf.len() >= BATCH_SIZE {
                            flush(&pool, &mut buf, &metrics).await;
                        }
                    }
                    None => {
                        // Channel closed — flush remaining and exit
                        if !buf.is_empty() {
                            flush(&pool, &mut buf, &metrics).await;
                        }
                        return;
                    }
                }
            }
            _ = interval.tick() => {
                if !buf.is_empty() {
                    flush(&pool, &mut buf, &metrics).await;
                }
            }
        }
    }
}

async fn flush(pool: &Pool, buf: &mut Vec<LogEntry>, metrics: &LogPipelineMetrics) {
    let start = std::time::Instant::now();
    let count = buf.len();

    let result = insert_batch(pool, buf).await;
    let elapsed = start.elapsed().as_millis() as u64;
    metrics.last_flush_ms.store(elapsed, Ordering::Relaxed);
    metrics.flush_count.fetch_add(1, Ordering::Relaxed);

    match result {
        Ok(()) => {
            metrics.entries_written.fetch_add(count as u64, Ordering::Relaxed);
        }
        Err(e) => {
            metrics.flush_errors.fetch_add(1, Ordering::Relaxed);
            metrics.entries_dropped.fetch_add(count as u64, Ordering::Relaxed);
            // Log to console (always available) — NOT through tracing to avoid recursion
            eprintln!("[networker-log] batch flush failed ({count} entries dropped): {e}");
        }
    }

    buf.clear();
    metrics.queue_depth.store(0, Ordering::Relaxed);
}

async fn insert_batch(pool: &Pool, entries: &[LogEntry]) -> anyhow::Result<()> {
    let client = pool.get().await?;

    // Build a single multi-row INSERT for efficiency
    let mut query = String::from(
        "INSERT INTO service_log (ts, service, level, message, config_id, project_id, trace_id, fields) VALUES "
    );
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = Vec::new();
    let mut idx = 1u32;

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            query.push(',');
        }
        query.push_str(&format!(
            "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
            idx, idx + 1, idx + 2, idx + 3, idx + 4, idx + 5, idx + 6, idx + 7
        ));
        params.push(Box::new(entry.ts));
        params.push(Box::new(entry.service.clone()));
        params.push(Box::new(entry.level.as_i16()));
        params.push(Box::new(entry.message.clone()));
        params.push(Box::new(entry.config_id));
        params.push(Box::new(entry.project_id.clone()));
        params.push(Box::new(entry.trace_id));
        params.push(Box::new(entry.fields.clone()));
        idx += 8;
    }

    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|p| p.as_ref()).collect();
    client.execute(&query, &param_refs).await?;
    Ok(())
}
```

- [ ] **Step 3: Update lib.rs**

```rust
// crates/networker-log/src/lib.rs
pub mod batch;
pub mod metrics;
pub mod schema;
pub mod types;

pub use metrics::{LogPipelineMetrics, MetricsSnapshot};
pub use types::{Level, LogEntry};
```

- [ ] **Step 4: Build**

Run: `cargo build -p networker-log`
Expected: compiles without errors

- [ ] **Step 5: Commit**

```bash
git add crates/networker-log/src/batch.rs crates/networker-log/src/schema.rs crates/networker-log/src/lib.rs
git commit -m "feat(log): batch writer and schema for service_log hypertable"
```

---

### Task 3: DB tracing layer

**Files:**
- Create: `crates/networker-log/src/db_layer.rs`
- Modify: `crates/networker-log/src/lib.rs`

- [ ] **Step 1: Create db_layer.rs**

```rust
// crates/networker-log/src/db_layer.rs
use crate::batch::BatchHandle;
use crate::metrics::LogPipelineMetrics;
use crate::types::{Level, LogEntry};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Tracing layer that sends log entries to the batch writer channel.
pub struct DbLayer {
    tx: mpsc::Sender<LogEntry>,
    service: String,
    metrics: Arc<LogPipelineMetrics>,
    /// Process-wide context fields (config_id, project_id, trace_id).
    context: HashMap<String, String>,
}

impl DbLayer {
    pub fn new(
        handle: &BatchHandle,
        service: &str,
        metrics: Arc<LogPipelineMetrics>,
        context: HashMap<String, String>,
    ) -> Self {
        Self {
            tx: handle.tx.clone(),
            service: service.to_string(),
            metrics,
            context,
        }
    }
}

/// Visitor that extracts the message + all structured fields from a tracing event.
struct FieldVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: serde_json::Map::new(),
        }
    }
}

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
            // Remove surrounding quotes from Debug formatting
            if self.message.starts_with('"') && self.message.ends_with('"') {
                self.message = self.message[1..self.message.len() - 1].to_string();
            }
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Value::String(format!("{:?}", value)),
            );
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Bool(value),
        );
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for DbLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let level = Level::from_tracing(metadata.level());

        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);

        // Extract well-known fields from context or event fields
        let config_id = self
            .context
            .get("config_id")
            .or_else(|| visitor.fields.get("config_id").and_then(|v| v.as_str()).map(|_| visitor.fields.get("config_id").unwrap()).and_then(|v| v.as_str()))
            .and_then(|s| Uuid::parse_str(s).ok())
            .or_else(|| {
                visitor.fields.remove("config_id")
                    .and_then(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
            });

        let project_id = self
            .context
            .get("project_id")
            .cloned()
            .or_else(|| visitor.fields.remove("project_id").and_then(|v| v.as_str().map(String::from)));

        let trace_id = self
            .context
            .get("trace_id")
            .and_then(|s| Uuid::parse_str(s).ok())
            .or_else(|| {
                visitor.fields.remove("trace_id")
                    .and_then(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
            });

        let fields = if visitor.fields.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(visitor.fields))
        };

        let entry = LogEntry {
            ts: Utc::now(),
            service: self.service.clone(),
            level,
            message: visitor.message,
            config_id,
            project_id,
            trace_id,
            fields,
        };

        // Non-blocking send — drop entry if channel full
        match self.tx.try_send(entry) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.metrics.entries_dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Writer shut down — silently drop
            }
        }
    }
}
```

- [ ] **Step 2: Update lib.rs**

Add `pub mod db_layer;` and re-export `DbLayer`:

```rust
// crates/networker-log/src/lib.rs
pub mod batch;
pub mod db_layer;
pub mod metrics;
pub mod schema;
pub mod types;

pub use db_layer::DbLayer;
pub use metrics::{LogPipelineMetrics, MetricsSnapshot};
pub use types::{Level, LogEntry};
```

- [ ] **Step 3: Build**

Run: `cargo build -p networker-log`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/networker-log/src/db_layer.rs crates/networker-log/src/lib.rs
git commit -m "feat(log): DB tracing layer with structured field extraction"
```

---

### Task 4: LogBuilder API

**Files:**
- Create: `crates/networker-log/src/builder.rs`
- Modify: `crates/networker-log/src/lib.rs`

- [ ] **Step 1: Create builder.rs**

```rust
// crates/networker-log/src/builder.rs
use crate::batch;
use crate::db_layer::DbLayer;
use crate::metrics::LogPipelineMetrics;
use crate::schema;
use deadpool_postgres::{Config, Pool, Runtime};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_postgres::NoTls;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Console output destination.
#[derive(Debug, Clone, Copy)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Guard that flushes the batch writer on drop.
/// Must be held for the lifetime of the application.
pub struct LogGuard {
    _batch_handle: Option<batch::BatchHandle>,
    metrics: Arc<LogPipelineMetrics>,
}

impl LogGuard {
    pub fn metrics(&self) -> &Arc<LogPipelineMetrics> {
        &self.metrics
    }
}

/// Builder for initializing the logging pipeline.
pub struct LogBuilder {
    service: String,
    console: Option<Stream>,
    db_url: Option<String>,
    context: HashMap<String, String>,
    env_filter: Option<String>,
}

impl LogBuilder {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
            console: None,
            db_url: None,
            context: HashMap::new(),
            env_filter: None,
        }
    }

    /// Enable console output to stdout or stderr.
    pub fn with_console(mut self, stream: Stream) -> Self {
        self.console = Some(stream);
        self
    }

    /// Enable database persistence. URL points to the logs database.
    pub fn with_db(mut self, url: &str) -> Self {
        self.db_url = Some(url.to_string());
        self
    }

    /// Attach a context field to every log entry from this process.
    pub fn with_context(mut self, key: &str, value: &str) -> Self {
        self.context.insert(key.to_string(), value.to_string());
        self
    }

    /// Override the default env filter (default: RUST_LOG or "info").
    pub fn with_filter(mut self, filter: &str) -> Self {
        self.env_filter = Some(filter.to_string());
        self
    }

    /// Initialize the logging pipeline. Returns a guard that must be held.
    pub async fn init(self) -> anyhow::Result<LogGuard> {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(self.env_filter.as_deref().unwrap_or("info"))
        });

        let metrics = Arc::new(LogPipelineMetrics::new());

        // Build DB layer if URL provided
        let (db_layer, batch_handle) = if let Some(url) = &self.db_url {
            match Self::setup_db(url, &self.service, &metrics, &self.context).await {
                Ok((layer, handle)) => (Some(layer), Some(handle)),
                Err(e) => {
                    eprintln!(
                        "[networker-log] DB layer unavailable ({e:#}), console only. \
                         Service: {}", self.service
                    );
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        // Build console layer
        let console_layer = self.console.map(|stream| {
            let fmt = tracing_subscriber::fmt::layer();
            match stream {
                Stream::Stderr => fmt.with_writer(std::io::stderr).boxed(),
                Stream::Stdout => fmt.with_writer(std::io::stdout).boxed(),
            }
        });

        // Compose the subscriber
        tracing_subscriber::registry()
            .with(filter)
            .with(console_layer)
            .with(db_layer)
            .init();

        Ok(LogGuard {
            _batch_handle: batch_handle,
            metrics,
        })
    }

    async fn setup_db(
        url: &str,
        service: &str,
        metrics: &Arc<LogPipelineMetrics>,
        context: &HashMap<String, String>,
    ) -> anyhow::Result<(DbLayer, batch::BatchHandle)> {
        let mut cfg = Config::new();
        cfg.url = Some(url.into());
        cfg.pool = Some(deadpool_postgres::PoolConfig {
            max_size: 4,
            timeouts: deadpool_postgres::Timeouts {
                wait: Some(std::time::Duration::from_secs(3)),
                create: Some(std::time::Duration::from_secs(3)),
                recycle: Some(std::time::Duration::from_secs(3)),
            },
            ..Default::default()
        });
        let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;

        // Test connectivity + ensure schema
        let client = pool.get().await?;
        schema::ensure_table(&client).await?;
        schema::ensure_hypertable(&client).await?;
        drop(client);

        let handle = batch::spawn_batch_writer(pool, metrics.clone());
        let layer = DbLayer::new(&handle, service, metrics.clone(), context.clone());

        Ok((layer, handle))
    }
}
```

- [ ] **Step 2: Update lib.rs with full public API**

```rust
// crates/networker-log/src/lib.rs
pub mod batch;
pub mod builder;
pub mod db_layer;
pub mod metrics;
pub mod schema;
pub mod types;

pub use builder::{LogBuilder, LogGuard, Stream};
pub use db_layer::DbLayer;
pub use metrics::{LogPipelineMetrics, MetricsSnapshot};
pub use types::{Level, LogEntry};
```

- [ ] **Step 3: Build**

Run: `cargo build -p networker-log`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/networker-log/src/builder.rs crates/networker-log/src/lib.rs
git commit -m "feat(log): LogBuilder API — compose console + DB layers"
```

---

### Task 5: Query functions for dashboard API

**Files:**
- Create: `crates/networker-log/src/query.rs`
- Modify: `crates/networker-log/src/lib.rs`

- [ ] **Step 1: Create query.rs**

```rust
// crates/networker-log/src/query.rs
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

/// Query parameters for listing logs.
pub struct LogQuery {
    pub service: Option<String>,
    pub min_level: Option<i16>,     // 1=ERROR..5=TRACE
    pub config_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub search: Option<String>,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub limit: i64,
    pub offset: i64,
}

/// A log entry returned from a query.
#[derive(Debug, Serialize)]
pub struct LogRow {
    pub ts: DateTime<Utc>,
    pub service: String,
    pub level: i16,
    pub message: String,
    pub config_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub trace_id: Option<Uuid>,
    pub fields: Option<serde_json::Value>,
}

/// Response for the logs list endpoint.
#[derive(Debug, Serialize)]
pub struct LogQueryResponse {
    pub entries: Vec<LogRow>,
    pub total: i64,
    pub truncated: bool,
}

/// Query service_log with filters. Returns entries + total count.
pub async fn list(client: &Client, q: &LogQuery) -> anyhow::Result<LogQueryResponse> {
    let mut where_clauses = vec!["ts >= $1".to_string(), "ts <= $2".to_string()];
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = Vec::new();
    params.push(Box::new(q.from));
    params.push(Box::new(q.to));
    let mut idx = 3u32;

    if let Some(ref svc) = q.service {
        where_clauses.push(format!("service = ${idx}"));
        params.push(Box::new(svc.clone()));
        idx += 1;
    }
    if let Some(lvl) = q.min_level {
        where_clauses.push(format!("level <= ${idx}"));
        params.push(Box::new(lvl));
        idx += 1;
    }
    if let Some(cid) = q.config_id {
        where_clauses.push(format!("config_id = ${idx}"));
        params.push(Box::new(cid));
        idx += 1;
    }
    if let Some(ref pid) = q.project_id {
        where_clauses.push(format!("project_id = ${idx}"));
        params.push(Box::new(pid.clone()));
        idx += 1;
    }
    if let Some(ref search) = q.search {
        where_clauses.push(format!("message ILIKE ${idx}"));
        params.push(Box::new(format!("%{search}%")));
        idx += 1;
    }

    let where_sql = where_clauses.join(" AND ");

    // Count
    let count_sql = format!("SELECT COUNT(*) FROM service_log WHERE {where_sql}");
    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|p| p.as_ref()).collect();
    let total: i64 = client.query_one(&count_sql, &param_refs).await?.get(0);
    let truncated = total > 10_000;

    // Fetch
    let fetch_sql = format!(
        "SELECT ts, service, level, message, config_id, project_id, trace_id, fields \
         FROM service_log WHERE {where_sql} ORDER BY ts DESC LIMIT ${idx} OFFSET ${}",
        idx + 1
    );
    params.push(Box::new(q.limit));
    params.push(Box::new(q.offset));
    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|p| p.as_ref()).collect();

    let rows = client.query(&fetch_sql, &param_refs).await?;
    let entries = rows
        .iter()
        .map(|r| LogRow {
            ts: r.get("ts"),
            service: r.get("service"),
            level: r.get("level"),
            message: r.get("message"),
            config_id: r.get("config_id"),
            project_id: r.get("project_id"),
            trace_id: r.get("trace_id"),
            fields: r.get("fields"),
        })
        .collect();

    Ok(LogQueryResponse {
        entries,
        total,
        truncated,
    })
}

/// Per-service, per-level counts for a time window.
#[derive(Debug, Serialize)]
pub struct LogStats {
    pub by_service: std::collections::HashMap<String, ServiceStats>,
    pub total: i64,
}

#[derive(Debug, Serialize, Default)]
pub struct ServiceStats {
    pub error: i64,
    pub warn: i64,
    pub info: i64,
    pub debug: i64,
    pub trace: i64,
}

pub async fn stats(
    client: &Client,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> anyhow::Result<LogStats> {
    let rows = client
        .query(
            "SELECT service, level, COUNT(*) as cnt \
             FROM service_log WHERE ts >= $1 AND ts <= $2 \
             GROUP BY service, level ORDER BY service, level",
            &[&from, &to],
        )
        .await?;

    let mut by_service: std::collections::HashMap<String, ServiceStats> =
        std::collections::HashMap::new();
    let mut total = 0i64;

    for row in &rows {
        let svc: String = row.get("service");
        let level: i16 = row.get("level");
        let cnt: i64 = row.get("cnt");
        total += cnt;

        let stats = by_service.entry(svc).or_default();
        match level {
            1 => stats.error = cnt,
            2 => stats.warn = cnt,
            3 => stats.info = cnt,
            4 => stats.debug = cnt,
            5 => stats.trace = cnt,
            _ => {}
        }
    }

    Ok(LogStats { by_service, total })
}
```

- [ ] **Step 2: Update lib.rs**

Add `pub mod query;` to lib.rs.

- [ ] **Step 3: Build**

Run: `cargo build -p networker-log`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/networker-log/src/query.rs crates/networker-log/src/lib.rs
git commit -m "feat(log): query functions for logs list and stats"
```

---

### Task 6: Docker infrastructure + integration test

**Files:**
- Modify: `docker-compose.dashboard.yml`
- Modify: `scripts/init-logs-db.sql`

- [ ] **Step 1: Update docker-compose.dashboard.yml**

Change the postgres image from `postgres:16-alpine` to `timescale/timescaledb-ha:pg16.6-ts2.17.2`:

```yaml
services:
  postgres:
    image: timescale/timescaledb-ha:pg16.6-ts2.17.2
    environment:
      POSTGRES_DB: networker_core
      POSTGRES_USER: networker
      POSTGRES_PASSWORD: networker
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./scripts/init-logs-db.sql:/docker-entrypoint-initdb.d/01-logs-db.sql
```

- [ ] **Step 2: Update init-logs-db.sql**

```sql
-- Creates the logs database and enables TimescaleDB on first initialization.
-- Mounted as /docker-entrypoint-initdb.d/01-logs-db.sql in docker-compose.
CREATE DATABASE networker_logs OWNER networker;

-- Enable TimescaleDB on the logs database
\c networker_logs
CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE;
```

- [ ] **Step 3: Write integration test**

Create `crates/networker-log/tests/integration.rs`:

```rust
//! Integration test — requires PostgreSQL with TimescaleDB.
//! Run: LOGS_DB_URL="postgres://networker:networker@127.0.0.1:5432/networker_logs" cargo test -p networker-log --test integration

use networker_log::{LogBuilder, Stream};

#[tokio::test]
#[ignore] // requires running PostgreSQL
async fn log_builder_writes_to_db_and_queries_back() {
    let db_url = std::env::var("LOGS_DB_URL")
        .unwrap_or_else(|_| "postgres://networker:networker@127.0.0.1:5432/networker_logs".into());

    let guard = LogBuilder::new("integration-test")
        .with_console(Stream::Stderr)
        .with_db(&db_url)
        .with_context("config_id", "00000000-0000-0000-0000-000000000001")
        .init()
        .await
        .expect("failed to init logging");

    // Emit some log entries
    tracing::info!(testbed_id = "tb-1", "Test log entry one");
    tracing::warn!("Test warning entry");
    tracing::error!(language = "rust", "Test error entry");

    // Wait for batch flush
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Check metrics
    let snap = guard.metrics().snapshot();
    assert!(snap.entries_written >= 3, "expected >=3 writes, got {}", snap.entries_written);
    assert_eq!(snap.entries_dropped, 0);
    assert_eq!(snap.status(), "healthy");

    // Query back via DB
    let pool = {
        let mut cfg = deadpool_postgres::Config::new();
        cfg.url = Some(db_url.into());
        cfg.create_pool(Some(deadpool_postgres::Runtime::Tokio1), tokio_postgres::NoTls).unwrap()
    };
    let client = pool.get().await.unwrap();

    let q = networker_log::query::LogQuery {
        service: Some("integration-test".into()),
        min_level: None,
        config_id: None,
        project_id: None,
        search: Some("Test log entry".into()),
        from: chrono::Utc::now() - chrono::Duration::minutes(1),
        to: chrono::Utc::now() + chrono::Duration::minutes(1),
        limit: 10,
        offset: 0,
    };
    let result = networker_log::query::list(&client, &q).await.unwrap();
    assert!(result.total >= 1, "expected to find log entries, got {}", result.total);
    assert_eq!(result.entries[0].service, "integration-test");

    // Cleanup test entries
    client
        .execute("DELETE FROM service_log WHERE service = 'integration-test'", &[])
        .await
        .unwrap();
}
```

- [ ] **Step 4: Run integration test (requires docker up)**

```bash
docker compose -f docker-compose.dashboard.yml up postgres -d
sleep 3
LOGS_DB_URL="postgres://networker:networker@127.0.0.1:5432/networker_logs" \
  cargo test -p networker-log --test integration -- --include-ignored
```

Expected: 1 test passes

- [ ] **Step 5: Commit**

```bash
git add docker-compose.dashboard.yml scripts/init-logs-db.sql crates/networker-log/tests/
git commit -m "feat(log): TimescaleDB docker + integration test"
```

---

## Summary

After completing all 6 tasks, the `networker-log` crate is ready for integration:
- `LogBuilder::new("service").with_console(Stderr).with_db(url).init()` — full API
- Batch writer flushes to `service_log` hypertable every 500ms or 100 entries
- Pipeline metrics (drops, errors, queue depth) via `LogGuard::metrics()`
- Query functions for dashboard API (list with filters, stats with aggregates)
- TimescaleDB with 7-day automatic retention
- Integration test proving the full pipeline works

**Next plans:**
- Plan 2: Integrate into each crate (dashboard, orchestrator, agent, endpoint, tester)
- Plan 3: Dashboard API endpoints + UI (System > Logs page rewrite)
- Plan 4: Deployment safety (health check, smoke test, rollback)
