//! Async batch writer — buffers [`LogEntry`] values from an in-memory channel
//! and flushes them to PostgreSQL in bulk.
//!
//! # Design
//! - A `tokio::sync::mpsc` channel decouples producers from the DB writer.
//! - The writer task accumulates up to [`BATCH_SIZE`] entries or waits at most
//!   [`FLUSH_INTERVAL`] before flushing, whichever comes first.
//! - On channel close the task drains the remaining buffer and exits cleanly.
//! - All errors are printed to `eprintln!` — **not** `tracing::error!` — to
//!   avoid recursive log emission.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deadpool_postgres::Pool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_postgres::types::ToSql;

use crate::metrics::LogPipelineMetrics;
use crate::types::LogEntry;

/// Maximum number of entries accumulated before an automatic flush.
const BATCH_SIZE: usize = 100;

/// Maximum time between flushes even when the batch is not full.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Capacity of the in-process channel (back-pressure limit for producers).
const CHANNEL_CAPACITY: usize = 10_000;

// ── Public handle ─────────────────────────────────────────────────────────────

/// A handle to the running batch-writer task.
///
/// Drop this (or call [`BatchHandle::shutdown`]) to stop accepting new entries
/// and wait for the writer to finish flushing.
pub struct BatchHandle {
    tx: mpsc::Sender<LogEntry>,
    handle: JoinHandle<()>,
}

impl BatchHandle {
    /// Return a cloned sender so that multiple producers can share the channel.
    pub fn sender(&self) -> mpsc::Sender<LogEntry> {
        self.tx.clone()
    }

    /// Gracefully shut down: signal end-of-stream and wait for the task to
    /// drain the remaining buffer and exit.
    pub async fn shutdown(self) {
        // Drop the sender to close the channel from the producer side.
        drop(self.tx);
        // Wait for the writer to finish.
        if let Err(e) = self.handle.await {
            eprintln!("networker-log: batch writer task panicked: {e:?}");
        }
    }
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

/// Spawn a background task that writes log entries to the database in batches.
///
/// Returns a [`BatchHandle`] that the caller can use to send entries and
/// eventually shut the writer down.
pub fn spawn_batch_writer(pool: Pool, metrics: Arc<LogPipelineMetrics>) -> BatchHandle {
    let (tx, rx) = mpsc::channel::<LogEntry>(CHANNEL_CAPACITY);
    let handle = tokio::spawn(writer_loop(rx, pool, metrics));
    BatchHandle { tx, handle }
}

// ── Internal writer loop ──────────────────────────────────────────────────────

async fn writer_loop(
    mut rx: mpsc::Receiver<LogEntry>,
    pool: Pool,
    metrics: Arc<LogPipelineMetrics>,
) {
    let mut buffer: Vec<LogEntry> = Vec::with_capacity(BATCH_SIZE);
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    // The first tick fires immediately; skip it so we don't flush an empty
    // buffer right away.
    interval.tick().await;

    loop {
        tokio::select! {
            // Bias toward receiving entries to fill the batch quickly.
            biased;

            msg = rx.recv() => {
                match msg {
                    Some(entry) => {
                        buffer.push(entry);
                        metrics.queue_depth.store(buffer.len() as u32, Ordering::Relaxed);

                        if buffer.len() >= BATCH_SIZE {
                            flush(&mut buffer, &pool, &metrics).await;
                        }
                    }
                    None => {
                        // Channel closed — drain remaining entries and exit.
                        if !buffer.is_empty() {
                            flush(&mut buffer, &pool, &metrics).await;
                        }
                        return;
                    }
                }
            }

            _ = interval.tick() => {
                if !buffer.is_empty() {
                    flush(&mut buffer, &pool, &metrics).await;
                }
            }
        }
    }
}

// ── Flush helpers ─────────────────────────────────────────────────────────────

/// Flush `buffer` to the database, update metrics, and clear the buffer.
async fn flush(buffer: &mut Vec<LogEntry>, pool: &Pool, metrics: &Arc<LogPipelineMetrics>) {
    if buffer.is_empty() {
        return;
    }

    let count = buffer.len() as u64;
    let start = Instant::now();
    metrics.flush_count.fetch_add(1, Ordering::Relaxed);

    match pool.get().await {
        Ok(client) => match insert_batch(&client, buffer).await {
            Ok(()) => {
                let elapsed = start.elapsed().as_millis() as u64;
                metrics.entries_written.fetch_add(count, Ordering::Relaxed);
                metrics.last_flush_ms.store(elapsed, Ordering::Relaxed);
            }
            Err(e) => {
                eprintln!("networker-log: insert_batch failed ({count} entries dropped): {e}");
                metrics.entries_dropped.fetch_add(count, Ordering::Relaxed);
                metrics.flush_errors.fetch_add(1, Ordering::Relaxed);
            }
        },
        Err(e) => {
            eprintln!(
                "networker-log: failed to acquire DB connection ({count} entries dropped): {e}"
            );
            metrics.entries_dropped.fetch_add(count, Ordering::Relaxed);
            metrics.flush_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    metrics.queue_depth.store(0, Ordering::Relaxed);
    buffer.clear();
}

/// Build and execute a single multi-row INSERT for all entries in `batch`.
///
/// Returns immediately (no-op) if `batch` is empty.
async fn insert_batch(
    client: &deadpool_postgres::Object,
    batch: &[LogEntry],
) -> Result<(), tokio_postgres::Error> {
    if batch.is_empty() {
        return Ok(());
    }

    // Build the SQL:
    //   INSERT INTO service_log (ts, service, level, message, config_id,
    //                            project_id, trace_id, fields)
    //   VALUES ($1,$2,$3,$4,$5,$6,$7,$8), ($9,...), ...
    const COLS: usize = 8;
    let mut sql = String::from(
        "INSERT INTO service_log \
         (ts, service, level, message, config_id, project_id, trace_id, fields) \
         VALUES ",
    );

    for (i, _) in batch.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let base = i * COLS + 1;
        sql.push('(');
        for col in 0..COLS {
            if col > 0 {
                sql.push(',');
            }
            sql.push('$');
            sql.push_str(&(base + col).to_string());
        }
        sql.push(')');
    }

    // Build the parameter list using boxed trait objects so that heterogeneous
    // types (DateTime, String, i16, Option<Uuid>, …) can live in one Vec.
    let mut params: Vec<Box<dyn ToSql + Sync + Send>> = Vec::with_capacity(batch.len() * COLS);

    for entry in batch {
        params.push(Box::new(entry.ts));
        params.push(Box::new(entry.service.clone()));
        params.push(Box::new(entry.level.as_db()));
        params.push(Box::new(entry.message.clone()));
        params.push(Box::new(entry.config_id));
        params.push(Box::new(entry.project_id.clone()));
        params.push(Box::new(entry.trace_id));
        params.push(Box::new(entry.fields.clone()));
    }

    // tokio-postgres `query_raw` / `execute` expects `&[&(dyn ToSql + Sync)]`.
    let param_refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|b| -> &(dyn ToSql + Sync) { b.as_ref() })
        .collect();

    client.execute(sql.as_str(), param_refs.as_slice()).await?;
    Ok(())
}
