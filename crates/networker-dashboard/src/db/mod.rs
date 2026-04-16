pub mod agent_commands;
pub mod agents;
pub mod benchmark_artifacts;
pub mod benchmark_testbeds;
pub mod benchmark_vm_catalog;
pub mod cloud_accounts;
pub mod cloud_connections;
pub mod command_approvals;
pub mod deployments;
pub mod invites;
pub mod migrations;
pub mod perf_log;
pub mod project_testers;
pub mod projects;
pub mod share_links;
pub mod sso_providers;
pub mod system_health;
pub mod test_configs;
pub mod test_runs;
pub mod test_schedules;
pub mod tls_profiles;
pub mod url_tests;
pub mod users;
pub mod visibility;
pub mod vm_lifecycle;
pub mod workspace_warnings;
pub mod zones;

// NOTE (v0.28.0 refactor): the following modules were removed in favour
// of the unified TestConfig primitive. See `.critique/refactor/03-spec.md`:
//   - `jobs.rs`              → replaced by `test_configs` + `test_runs`
//   - `schedules.rs`         → replaced by `test_schedules`
//   - `benchmark_configs.rs` → replaced by `test_configs`
//   - `benchmarks.rs`        → split: CRUD goes to `benchmark_artifacts`;
//                              comparison / leaderboard / run reading
//                              helpers will be rebuilt on top of the new
//                              schema by Agent B as needed.
//   - `runs.rs`              → the legacy TestRun listing / attempt-reading
//                              helpers will be rebuilt by Agent B against
//                              the new `test_run` table + renamed
//                              per-protocol phase columns.
//   - `benchmark_progress.rs` → `benchmark_request_progress` table is dropped.
//   - `benchmark_presets.rs`  → `benchmark_compare_preset` table is dropped.

use deadpool_postgres::{Config, Pool, Runtime};
use tokio_postgres::NoTls;

pub async fn create_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.into());
    cfg.pool = Some(deadpool_postgres::PoolConfig {
        max_size: 16,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(std::time::Duration::from_secs(5)),
            create: Some(std::time::Duration::from_secs(5)),
            recycle: Some(std::time::Duration::from_secs(5)),
        },
        ..Default::default()
    });
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    // Test connectivity
    let _client = pool.get().await?;
    tracing::info!("Connected to PostgreSQL");
    Ok(pool)
}

/// Create a connection pool for the logs database (smaller pool, same timeouts).
pub async fn create_logs_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.into());
    cfg.pool = Some(deadpool_postgres::PoolConfig {
        max_size: 8,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(std::time::Duration::from_secs(5)),
            create: Some(std::time::Duration::from_secs(5)),
            recycle: Some(std::time::Duration::from_secs(5)),
        },
        ..Default::default()
    });
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    let _client = pool.get().await?;
    tracing::info!("Connected to logs PostgreSQL");
    Ok(pool)
}
