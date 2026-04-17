//! Regression detection stub (v0.28.0).
//!
//! The old regression module compared `benchmark_config` runs to baseline. In
//! v0.28.0, regression detection will compare `benchmark_artifact` rows
//! attached to `test_run` entries. The detection logic is the same but the
//! data access layer changed. This stub compiles and exposes empty types so
//! downstream API handlers (which will be rebuilt on v2) compile without error.

use serde::Serialize;
use uuid::Uuid;

/// A detected regression.
#[derive(Debug, Clone, Serialize)]
pub struct Regression {
    pub phase: String,
    pub metric: String,
    pub baseline_value: f64,
    pub current_value: f64,
    pub change_pct: f64,
}

/// A regression row (DB-persisted).
#[derive(Debug, Clone, Serialize)]
pub struct RegressionRow {
    pub id: Uuid,
    pub test_run_id: Uuid,
    pub regressions: Vec<Regression>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A regression row joined with its config name for listing.
#[derive(Debug, Clone, Serialize)]
pub struct RegressionWithConfig {
    pub id: Uuid,
    pub test_config_id: Uuid,
    pub config_name: String,
    pub test_run_id: Uuid,
    pub regressions: Vec<Regression>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
