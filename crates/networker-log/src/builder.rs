//! High-level builder for composing a tracing subscriber with optional
//! console and database layers.
//!
//! # Quick start
//!
//! ```rust,no_run
//! # async fn run() -> anyhow::Result<()> {
//! use networker_log::LogBuilder;
//!
//! let guard = LogBuilder::new("my-service")
//!     .with_console(networker_log::Stream::Stderr)
//!     .with_db("postgres://user:pass@localhost/db")
//!     .init()
//!     .await?;
//!
//! // Use tracing macros normally ...
//!
//! guard.shutdown().await;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use deadpool_postgres::{Config as PoolConfig, ManagerConfig, Pool, RecyclingMethod, Runtime};
use deadpool_postgres::PoolConfig as DpPoolConfig;
use deadpool_postgres::Timeouts;
use tokio_postgres::NoTls;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer};

use crate::batch::{spawn_batch_writer, BatchHandle};
use crate::db_layer::DbLayer;
use crate::metrics::LogPipelineMetrics;
use crate::schema;

// ── Stream ────────────────────────────────────────────────────────────────────

/// Which standard stream the console log layer writes to.
#[derive(Debug, Clone, Copy)]
pub enum Stream {
    Stdout,
    Stderr,
}

// ── LogGuard ──────────────────────────────────────────────────────────────────

/// Returned by [`LogBuilder::init`].
///
/// Holds the optional batch-writer handle and the pipeline metrics handle.
/// Call [`LogGuard::shutdown`] at the end of `main` to flush any remaining
/// buffered log entries to the database before the process exits.
pub struct LogGuard {
    batch_handle: Option<BatchHandle>,
    metrics: Arc<LogPipelineMetrics>,
}

impl LogGuard {
    /// Access the shared pipeline metrics (entries written, dropped, etc.).
    pub fn metrics(&self) -> &Arc<LogPipelineMetrics> {
        &self.metrics
    }

    /// Gracefully shut down the batch writer, flushing all remaining entries.
    ///
    /// Must be called explicitly — `Drop` cannot perform async work.
    pub async fn shutdown(self) {
        if let Some(handle) = self.batch_handle {
            handle.shutdown().await;
        }
    }
}

// ── LogBuilder ────────────────────────────────────────────────────────────────

/// Composable builder for the tracing subscriber used by networker services.
///
/// Supports an optional pretty-printing console layer and an optional
/// structured-log database layer backed by PostgreSQL.
pub struct LogBuilder {
    service: String,
    console: Option<Stream>,
    db_url: Option<String>,
    context: HashMap<String, String>,
    env_filter: Option<String>,
}

impl LogBuilder {
    /// Create a new builder for the given `service` name.
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
            console: None,
            db_url: None,
            context: HashMap::new(),
            env_filter: None,
        }
    }

    /// Enable a console layer writing to `stream`.
    pub fn with_console(mut self, stream: Stream) -> Self {
        self.console = Some(stream);
        self
    }

    /// Enable the database layer by supplying a PostgreSQL connection URL.
    pub fn with_db(mut self, url: &str) -> Self {
        self.db_url = Some(url.to_owned());
        self
    }

    /// Attach an arbitrary key–value pair to every log entry emitted by the
    /// database layer (e.g. `project_id`, `config_id`).
    pub fn with_context(mut self, key: &str, value: &str) -> Self {
        self.context.insert(key.to_owned(), value.to_owned());
        self
    }

    /// Set a default `EnvFilter` directive used when `RUST_LOG` is not set.
    pub fn with_filter(mut self, filter: &str) -> Self {
        self.env_filter = Some(filter.to_owned());
        self
    }

    /// Compose and install the global tracing subscriber.
    ///
    /// Order of precedence for the log filter:
    /// 1. `RUST_LOG` environment variable
    /// 2. The directive passed to [`LogBuilder::with_filter`]
    /// 3. `"info"` (built-in default)
    ///
    /// If a DB URL was supplied but the database is unreachable, the error is
    /// printed to stderr and the builder falls back to console-only logging.
    pub async fn init(self) -> anyhow::Result<LogGuard> {
        // ── 1. EnvFilter ──────────────────────────────────────────────────────
        let default_directive = self
            .env_filter
            .as_deref()
            .unwrap_or("info")
            .to_owned();

        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(default_directive));

        // ── 2. Metrics ────────────────────────────────────────────────────────
        let metrics: Arc<LogPipelineMetrics> = Arc::new(LogPipelineMetrics::default());

        // ── 3. Optional DB layer ──────────────────────────────────────────────
        let mut batch_handle: Option<BatchHandle> = None;

        let db_layer: Option<DbLayer> = if let Some(ref url) = self.db_url {
            match setup_db(url, &self.service, Arc::clone(&metrics), self.context.clone()).await {
                Ok((layer, handle)) => {
                    batch_handle = Some(handle);
                    Some(layer)
                }
                Err(e) => {
                    eprintln!("networker-log: DB layer disabled — {e:#}");
                    None
                }
            }
        } else {
            None
        };

        // ── 4. Optional console layer ─────────────────────────────────────────
        let console_layer: Option<Box<dyn Layer<_> + Send + Sync>> = match self.console {
            Some(Stream::Stderr) => Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .boxed(),
            ),
            Some(Stream::Stdout) => Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stdout)
                    .boxed(),
            ),
            None => None,
        };

        // ── 5. Compose and install ────────────────────────────────────────────
        tracing_subscriber::registry()
            .with(filter)
            .with(console_layer)
            .with(db_layer)
            .init();

        Ok(LogGuard {
            batch_handle,
            metrics,
        })
    }
}

// ── setup_db ──────────────────────────────────────────────────────────────────

/// Attempt to connect to the database, run schema migrations, and start the
/// batch writer.  Returns both a [`DbLayer`] and the [`BatchHandle`] so that
/// the caller can shut the writer down gracefully.
async fn setup_db(
    url: &str,
    service: &str,
    metrics: Arc<LogPipelineMetrics>,
    context: HashMap<String, String>,
) -> anyhow::Result<(DbLayer, BatchHandle)> {
    // ── Build pool ────────────────────────────────────────────────────────────
    let pg_config: tokio_postgres::Config = url
        .parse()
        .context("invalid PostgreSQL connection URL")?;

    let mut pool_cfg = PoolConfig::new();
    pool_cfg.host = pg_config.get_hosts().first().and_then(|h| match h {
        tokio_postgres::config::Host::Tcp(host) => Some(host.clone()),
        #[cfg(unix)]
        tokio_postgres::config::Host::Unix(path) => {
            path.to_str().map(str::to_owned)
        }
    });
    pool_cfg.port = pg_config.get_ports().first().copied();
    pool_cfg.user = pg_config.get_user().map(str::to_owned);
    pool_cfg.password = pg_config
        .get_password()
        .map(|b| String::from_utf8_lossy(b).into_owned());
    pool_cfg.dbname = pg_config.get_dbname().map(str::to_owned);
    pool_cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    let timeout = Duration::from_secs(3);
    pool_cfg.pool = Some(DpPoolConfig {
        max_size: 4,
        timeouts: Timeouts {
            wait: Some(timeout),
            create: Some(timeout),
            recycle: Some(timeout),
        },
        ..DpPoolConfig::default()
    });

    let pool: Pool = pool_cfg
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .context("failed to create connection pool")?;

    // ── Test connectivity ─────────────────────────────────────────────────────
    let client = pool
        .get()
        .await
        .context("failed to connect to PostgreSQL")?;

    // ── Schema migrations ─────────────────────────────────────────────────────
    schema::ensure_table(&client)
        .await
        .context("ensure_table failed")?;

    schema::ensure_hypertable(&client)
        .await
        .context("ensure_hypertable failed")?;

    // Return the pooled connection — the pool will recycle it automatically.
    drop(client);

    // ── Batch writer ──────────────────────────────────────────────────────────
    let handle = spawn_batch_writer(pool, Arc::clone(&metrics));
    let tx = handle.sender();
    let layer = DbLayer::new(tx, service, metrics, context);

    Ok((layer, handle))
}
