//! Tracing subscriber [`Layer`] that captures events and forwards them to the
//! batch-writer channel as [`LogEntry`] values.
//!
//! # Reentrancy
//! This layer deliberately avoids all `tracing::*` macros internally — any
//! tracing call inside a layer callback would recurse infinitely.  Use
//! `eprintln!` for internal diagnostics instead.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use uuid::Uuid;

use crate::metrics::LogPipelineMetrics;
use crate::types::{Level, LogEntry};

// ── DbLayer ───────────────────────────────────────────────────────────────────

/// A [`tracing_subscriber::Layer`] that serialises tracing events into
/// [`LogEntry`] structs and sends them to the async batch writer via an
/// `mpsc` channel.
pub struct DbLayer {
    tx: mpsc::Sender<LogEntry>,
    service: String,
    metrics: Arc<LogPipelineMetrics>,
    /// Process-wide context injected into every entry.
    /// Recognised keys: `config_id`, `project_id`, `trace_id`.
    context: HashMap<String, String>,
}

impl DbLayer {
    pub fn new(
        tx: mpsc::Sender<LogEntry>,
        service: &str,
        metrics: Arc<LogPipelineMetrics>,
        context: HashMap<String, String>,
    ) -> Self {
        Self {
            tx,
            service: service.to_owned(),
            metrics,
            context,
        }
    }
}

// ── FieldVisitor ──────────────────────────────────────────────────────────────

/// Accumulates a tracing event's fields into a `message` string and a
/// catch-all JSON map for every other field.
struct FieldVisitor {
    message: String,
    fields: Map<String, Value>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: Map::new(),
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let raw = format!("{value:?}");
        if field.name() == "message" {
            // Strip surrounding double-quotes that Debug adds to strings.
            self.message = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
                raw[1..raw.len() - 1].to_owned()
            } else {
                raw
            };
        } else {
            self.fields
                .insert(field.name().to_owned(), Value::String(raw));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields
                .insert(field.name().to_owned(), Value::String(value.to_owned()));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_owned(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_owned(), Value::Number(value.into()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_owned(), Value::Bool(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // serde_json::Number doesn't accept NaN/Inf; fall back to string.
        let json_val = serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(value.to_string()));
        self.fields.insert(field.name().to_owned(), json_val);
    }
}

// ── Layer<S> implementation ───────────────────────────────────────────────────

impl<S> Layer<S> for DbLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let level = Level::from_tracing(metadata.level());

        // --- Collect fields ---------------------------------------------------
        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);
        let mut extra_fields = visitor.fields;

        // --- Resolve well-known IDs: context takes priority over event fields --

        // config_id
        let config_id: Option<Uuid> = self
            .context
            .get("config_id")
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                extra_fields
                    .remove("config_id")
                    .and_then(|v| v.as_str().and_then(|s| s.parse().ok()))
            });

        // project_id
        let project_id: Option<String> = self.context.get("project_id").cloned().or_else(|| {
            extra_fields
                .remove("project_id")
                .and_then(|v| v.as_str().map(str::to_owned))
        });

        // trace_id
        let trace_id: Option<Uuid> = self
            .context
            .get("trace_id")
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                extra_fields
                    .remove("trace_id")
                    .and_then(|v| v.as_str().and_then(|s| s.parse().ok()))
            });

        // --- Build the entry --------------------------------------------------
        let fields = if extra_fields.is_empty() {
            None
        } else {
            Some(Value::Object(extra_fields))
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

        // --- Send (non-blocking) ---------------------------------------------
        match self.tx.try_send(entry) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.metrics.entries_dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Channel gone — silently discard.
            }
        }
    }
}
