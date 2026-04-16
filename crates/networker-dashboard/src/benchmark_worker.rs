//! Benchmark worker stub (v0.28.0).
//!
//! The old `benchmark_worker` polled `benchmark_config` rows for status =
//! "queued" and spawned orchestrator processes locally. In v0.28.0 all test
//! execution (including benchmark-grade methodology tests) is dispatched to
//! remote agents via `ControlMessage::AssignRun` through the scheduler or the
//! launch endpoint. The local orchestrator flow is therefore dead code.
//!
//! This stub keeps `spawn()` callable from `main.rs` so the call-site doesn't
//! need a feature-flag — the task simply does nothing.

use crate::AppState;
use std::sync::Arc;

/// No-op — the benchmark worker is replaced by agent dispatch in v0.28.
pub fn spawn(_state: Arc<AppState>) {
    tracing::info!("Benchmark worker stub — no-op in v0.28 (execution via agent dispatch)");
}
