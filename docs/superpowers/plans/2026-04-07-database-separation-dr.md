# Database Separation & Disaster Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the single PostgreSQL database into core + logs databases on a dedicated DB server, add automated backups to Azure Blob, a disaster recovery CLI, and a sandbox provisioning system.

**Architecture:** Two Azure VMs (db-server for PostgreSQL, web-server for application). Two databases: `networker_core` (permanent, backed up) and `networker_logs` (7-day retention, ephemeral). Dashboard holds two connection pools. External scripts handle DB creation, backups, and DR — application code only manages schema within each database.

**Tech Stack:** PostgreSQL 16, deadpool-postgres, Azure CLI (az), Azure Blob Storage (azcopy), pg_dump/pg_restore, bash scripts, axum REST API, React/TypeScript frontend.

**Spec:** `docs/superpowers/specs/2026-04-07-database-separation-dr-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `scripts/init-logs-db.sql` | Docker entrypoint: creates `networker_logs` database |
| `scripts/migrate-to-split.sh` | One-time production migration: split `networker_dashboard` → core + logs |
| `scripts/backup-daily.sh` | Daily pg_dump of `networker_core` → Azure Blob |
| `scripts/retention-cleanup.sh` | Delete 7-day-old rows from logs DB + vacuum |
| `scripts/anonymize.sql` | Sandbox data anonymization queries |
| `scripts/infra.sh` | DR CLI: restore, sandbox, list |
| `crates/networker-dashboard/src/db/system_health.rs` | System health check queries and table management |
| `crates/networker-dashboard/src/api/system_health.rs` | `/api/system/health` endpoint |
| `dashboard/src/components/SystemHealthPanel.tsx` | System Health card for Settings page |

### Modified files

| File | Change |
|------|--------|
| `crates/networker-dashboard/src/db/mod.rs` | Add `create_logs_pool()`, export `system_health` module |
| `crates/networker-dashboard/src/config.rs` | Add `logs_database_url` field + `DASHBOARD_LOGS_DB_URL` parsing |
| `crates/networker-dashboard/src/main.rs` | Add `logs_db` field to `AppState`, create logs pool at startup |
| `crates/networker-dashboard/src/api/benchmark_callbacks.rs` | Switch progress insert to `state.logs_db` |
| `crates/networker-dashboard/src/api/benchmark_configs.rs` | Switch progress read to `state.logs_db` |
| `crates/networker-dashboard/src/api/perf_log.rs` | Switch all queries to `state.logs_db` |
| `crates/networker-dashboard/src/api/mod.rs` | Register `system_health` router |
| `crates/networker-dashboard/src/scheduler.rs` | Add hourly health check task |
| `crates/networker-dashboard/src/db/migrations.rs` | V026: `system_health` table in core DB |
| `docker-compose.dashboard.yml` | Rename DB to `networker_core`, mount init script |
| `dashboard/src/pages/SettingsPage.tsx` | Add System Health panel |

---

## Phase 1: Dual-Pool Application Code

### Task 1: Add `logs_database_url` to config

**Files:**
- Modify: `crates/networker-dashboard/src/config.rs:6-36` (struct) and `:38-147` (from_env)

- [ ] **Step 1: Add the field to DashboardConfig struct**

In `crates/networker-dashboard/src/config.rs`, add the field after `database_url`:

```rust
pub struct DashboardConfig {
    pub database_url: String,
    pub logs_database_url: String,
    // ... rest unchanged
}
```

- [ ] **Step 2: Parse DASHBOARD_LOGS_DB_URL in from_env()**

In the `from_env()` method, after the `database_url` line, add the logs URL derivation:

```rust
let database_url = std::env::var("DASHBOARD_DB_URL").unwrap_or_else(|_| {
    "postgres://networker:networker@localhost:5432/networker_core".into()
});

// Logs database URL: explicit env var, or derive from core URL by replacing DB name
let logs_database_url = std::env::var("DASHBOARD_LOGS_DB_URL").unwrap_or_else(|_| {
    // Replace the last path segment (database name) with networker_logs
    if let Some(pos) = database_url.rfind('/') {
        format!("{}/networker_logs", &database_url[..pos])
    } else {
        database_url.replace("networker_core", "networker_logs")
    }
});
```

Also update the default `database_url` from `networker_dashboard` to `networker_core`.

Add `logs_database_url` to the struct construction at the end of `from_env()`.

- [ ] **Step 3: Run `cargo build -p networker-dashboard` to verify compilation**

Run: `cargo build -p networker-dashboard`
Expected: Compilation error — `logs_database_url` not used in `main.rs` yet, but struct is valid. May get unused warning.

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/config.rs
git commit -m "feat(dashboard): add DASHBOARD_LOGS_DB_URL config"
```

---

### Task 2: Add `create_logs_pool()` to db module

**Files:**
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Add create_logs_pool function**

In `crates/networker-dashboard/src/db/mod.rs`, after the existing `create_pool` function:

```rust
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
```

- [ ] **Step 2: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles (function is unused but allowed).

- [ ] **Step 3: Commit**

```bash
git add crates/networker-dashboard/src/db/mod.rs
git commit -m "feat(dashboard): add create_logs_pool with max_size=8"
```

---

### Task 3: Add `logs_db` to AppState and wire up at startup

**Files:**
- Modify: `crates/networker-dashboard/src/main.rs:60-94` (AppState struct) and `:148-212` (main fn)

- [ ] **Step 1: Add logs_db field to AppState**

In `crates/networker-dashboard/src/main.rs`, add to the `AppState` struct:

```rust
pub struct AppState {
    pub db: deadpool_postgres::Pool,
    pub logs_db: deadpool_postgres::Pool,
    pub database_url: String,
    // ... rest unchanged
}
```

- [ ] **Step 2: Create logs pool in main() and pass to AppState**

After `let db_pool = db::create_pool(&cfg.database_url).await?;` (line ~149), add:

```rust
let logs_pool = db::create_logs_pool(&cfg.logs_database_url).await?;
```

In the `AppState` construction (line ~189), add:

```rust
let state = Arc::new(AppState {
    db: db_pool,
    logs_db: logs_pool,
    database_url: cfg.database_url.clone(),
    // ... rest unchanged
});
```

- [ ] **Step 3: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles successfully. All existing code still uses `state.db` — no behavior change.

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/main.rs
git commit -m "feat(dashboard): wire logs_db pool into AppState"
```

---

### Task 4: Switch benchmark_progress API calls to logs_db

**Files:**
- Modify: `crates/networker-dashboard/src/api/benchmark_callbacks.rs:479`
- Modify: `crates/networker-dashboard/src/api/benchmark_configs.rs:583`

- [ ] **Step 1: Switch progress insert in benchmark_callbacks.rs**

In `crates/networker-dashboard/src/api/benchmark_callbacks.rs`, find the `callback_request_progress` handler. Change:

```rust
let client = state.db.get().await.map_err(|e| {
    tracing::error!(error = %e, "DB pool error in callback_request_progress");
    StatusCode::INTERNAL_SERVER_ERROR
})?;
```

to:

```rust
let client = state.logs_db.get().await.map_err(|e| {
    tracing::error!(error = %e, "Logs DB pool error in callback_request_progress");
    StatusCode::INTERNAL_SERVER_ERROR
})?;
```

- [ ] **Step 2: Switch progress read in benchmark_configs.rs**

In `crates/networker-dashboard/src/api/benchmark_configs.rs`, find where `benchmark_progress::get_progress` is called. Change the `state.db.get()` on that handler to `state.logs_db.get()`.

**Important:** Only change the specific handler that calls `benchmark_progress::get_progress`. Other handlers in this file use core tables and must keep `state.db`.

- [ ] **Step 3: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/api/benchmark_callbacks.rs crates/networker-dashboard/src/api/benchmark_configs.rs
git commit -m "feat(dashboard): switch benchmark_progress to logs_db pool"
```

---

### Task 5: Switch perf_log API calls to logs_db

**Files:**
- Modify: `crates/networker-dashboard/src/api/perf_log.rs:101,143,178`

- [ ] **Step 1: Replace all `state.db.get()` with `state.logs_db.get()` in perf_log.rs**

There are three occurrences (lines 101, 143, 178). Change each:

```rust
// Before:
let client = state.db.get().await.map_err(|e| {
    // ... perf_log context
})?;

// After:
let client = state.logs_db.get().await.map_err(|e| {
    // ... perf_log context
})?;
```

Also update line 101 which uses `let mut client`:

```rust
let mut client = state.logs_db.get().await.map_err(|e| {
```

- [ ] **Step 2: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/networker-dashboard/src/api/perf_log.rs
git commit -m "feat(dashboard): switch perf_log to logs_db pool"
```

---

### Task 6: Update docker-compose for dual databases

**Files:**
- Modify: `docker-compose.dashboard.yml`
- Create: `scripts/init-logs-db.sql`

- [ ] **Step 1: Create the init script**

Create `scripts/init-logs-db.sql`:

```sql
-- Creates the logs database on first PostgreSQL initialization.
-- Mounted as /docker-entrypoint-initdb.d/01-logs-db.sql in docker-compose.
CREATE DATABASE networker_logs OWNER networker;
```

- [ ] **Step 2: Update docker-compose.dashboard.yml**

Replace the full file:

```yaml
version: "3.8"

services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: networker_core
      POSTGRES_USER: networker
      POSTGRES_PASSWORD: networker
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./scripts/init-logs-db.sql:/docker-entrypoint-initdb.d/01-logs-db.sql
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U networker -d networker_core"]
      interval: 5s
      timeout: 5s
      retries: 5

volumes:
  pgdata:
```

- [ ] **Step 3: Test locally**

Stop existing containers and delete volume to start fresh:

```bash
docker compose -f docker-compose.dashboard.yml down -v
docker compose -f docker-compose.dashboard.yml up -d
```

Wait for health check, then verify both databases exist:

```bash
docker compose -f docker-compose.dashboard.yml exec postgres psql -U networker -d networker_core -c "SELECT 1;"
docker compose -f docker-compose.dashboard.yml exec postgres psql -U networker -d networker_logs -c "SELECT 1;"
```

Expected: Both return `1`.

- [ ] **Step 4: Commit**

```bash
git add docker-compose.dashboard.yml scripts/init-logs-db.sql
git commit -m "feat(docker): dual database setup — networker_core + networker_logs"
```

---

### Task 7: Add V026 migration — system_health table + logs DB schema

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`

- [ ] **Step 1: Add V026 SQL constant**

In `crates/networker-dashboard/src/db/migrations.rs`, after the last migration constant (V025), add:

```rust
/// V026: System health tracking table.
const V026_SYSTEM_HEALTH: &str = r#"
CREATE TABLE IF NOT EXISTS system_health (
    id            BIGSERIAL     NOT NULL PRIMARY KEY,
    checked_at    TIMESTAMPTZ   NOT NULL DEFAULT now(),
    check_name    VARCHAR(50)   NOT NULL,
    status        VARCHAR(10)   NOT NULL,
    value         TEXT,
    message       TEXT,
    details       JSONB
);
CREATE INDEX IF NOT EXISTS ix_system_health_checked_at ON system_health(checked_at DESC);
CREATE INDEX IF NOT EXISTS ix_system_health_name ON system_health(check_name, checked_at DESC);

-- Retention: auto-delete entries older than 7 days (cleanup runs in scheduler)
"#;
```

- [ ] **Step 2: Register V026 in the run() function**

In the `run()` function, after the V025 block, add:

```rust
// V026: System health table
let row = client
    .query_opt("SELECT version FROM _migrations WHERE version = 26", &[])
    .await?;

if row.is_none() {
    tracing::info!("Applying V026: system_health table...");
    client.batch_execute(V026_SYSTEM_HEALTH).await?;
    client
        .execute(
            "INSERT INTO _migrations (version) VALUES (26) ON CONFLICT DO NOTHING",
            &[],
        )
        .await?;
    tracing::info!("V026 migration complete");
}
```

- [ ] **Step 3: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles.

- [ ] **Step 4: Test migration runs**

Start PostgreSQL, then run the dashboard briefly to apply migration:

```bash
docker compose -f docker-compose.dashboard.yml up -d
DASHBOARD_DB_URL="postgres://networker:networker@localhost:5432/networker_core" \
DASHBOARD_JWT_SECRET="test-secret-that-is-at-least-32-bytes-long" \
DASHBOARD_ADMIN_EMAIL="admin@test.com" \
DASHBOARD_ADMIN_PASSWORD="testpassword123" \
cargo run -p networker-dashboard &
sleep 5
kill %1
```

Verify table exists:

```bash
docker compose -f docker-compose.dashboard.yml exec postgres psql -U networker -d networker_core \
  -c "SELECT * FROM _migrations WHERE version = 26;"
```

Expected: One row with version=26.

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat(dashboard): V026 migration — system_health table"
```

---

## Phase 2: Monitoring & Health Checks

### Task 8: Create system_health DB module

**Files:**
- Create: `crates/networker-dashboard/src/db/system_health.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs` (add module export)

- [ ] **Step 1: Create the module**

Create `crates/networker-dashboard/src/db/system_health.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;

#[derive(Debug, Serialize)]
pub struct HealthCheck {
    pub check_name: String,
    pub status: String,
    pub value: Option<String>,
    pub message: Option<String>,
    pub details: Option<serde_json::Value>,
    pub checked_at: DateTime<Utc>,
}

/// Insert a health check result.
pub async fn insert(
    client: &Client,
    check_name: &str,
    status: &str,
    value: Option<&str>,
    message: Option<&str>,
    details: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    client
        .execute(
            "INSERT INTO system_health (check_name, status, value, message, details) \
             VALUES ($1, $2, $3, $4, $5)",
            &[&check_name, &status, &value, &message, &details],
        )
        .await?;
    Ok(())
}

/// Get the latest health check for each check_name.
pub async fn latest_all(client: &Client) -> anyhow::Result<Vec<HealthCheck>> {
    let rows = client
        .query(
            "SELECT DISTINCT ON (check_name) \
                check_name, status, value, message, details, checked_at \
             FROM system_health \
             ORDER BY check_name, checked_at DESC",
            &[],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| HealthCheck {
            check_name: r.get("check_name"),
            status: r.get("status"),
            value: r.get("value"),
            message: r.get("message"),
            details: r.get("details"),
            checked_at: r.get("checked_at"),
        })
        .collect())
}

/// Get check history for a specific check (last 7 days).
pub async fn history(
    client: &Client,
    check_name: &str,
    limit: i64,
) -> anyhow::Result<Vec<HealthCheck>> {
    let rows = client
        .query(
            "SELECT check_name, status, value, message, details, checked_at \
             FROM system_health \
             WHERE check_name = $1 AND checked_at > now() - interval '7 days' \
             ORDER BY checked_at DESC \
             LIMIT $2",
            &[&check_name, &limit],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| HealthCheck {
            check_name: r.get("check_name"),
            status: r.get("status"),
            value: r.get("value"),
            message: r.get("message"),
            details: r.get("details"),
            checked_at: r.get("checked_at"),
        })
        .collect())
}

/// Delete health records older than 7 days.
pub async fn cleanup(client: &Client) -> anyhow::Result<u64> {
    let result = client
        .execute(
            "DELETE FROM system_health WHERE checked_at < now() - interval '7 days'",
            &[],
        )
        .await?;
    Ok(result)
}
```

- [ ] **Step 2: Export the module in mod.rs**

In `crates/networker-dashboard/src/db/mod.rs`, add:

```rust
pub mod system_health;
```

- [ ] **Step 3: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/db/system_health.rs crates/networker-dashboard/src/db/mod.rs
git commit -m "feat(dashboard): system_health DB module — insert, query, cleanup"
```

---

### Task 9: Add health check runner to scheduler

**Files:**
- Modify: `crates/networker-dashboard/src/scheduler.rs`

- [ ] **Step 1: Add health check tracking variable**

In the `spawn()` function (line ~18), after the existing `last_*` variables, add:

```rust
let mut last_health_check = std::time::Instant::now();
```

- [ ] **Step 2: Add hourly health check block**

After the daily `check_workspace_inactivity` block (around line 75), add:

```rust
// Hourly system health checks
if last_health_check.elapsed() > std::time::Duration::from_secs(3600) {
    last_health_check = std::time::Instant::now();
    run_health_checks(&state).await;
}
```

- [ ] **Step 3: Implement run_health_checks function**

Add at the end of the file (before the `#[cfg(test)]` module):

```rust
async fn run_health_checks(state: &Arc<AppState>) {
    // Check core DB
    let core_status = match state.db.get().await {
        Ok(client) => match client.query_one("SELECT 1 as ok", &[]).await {
            Ok(_) => ("green", None),
            Err(e) => ("red", Some(format!("Query failed: {e}"))),
        },
        Err(e) => ("red", Some(format!("Pool error: {e}"))),
    };

    // Check logs DB
    let logs_status = match state.logs_db.get().await {
        Ok(client) => match client.query_one("SELECT 1 as ok", &[]).await {
            Ok(_) => ("green", None),
            Err(e) => ("red", Some(format!("Query failed: {e}"))),
        },
        Err(e) => ("red", Some(format!("Pool error: {e}"))),
    };

    // Check core DB size
    let core_size = match state.db.get().await {
        Ok(client) => {
            match client
                .query_one(
                    "SELECT pg_database_size(current_database()) as size",
                    &[],
                )
                .await
            {
                Ok(row) => {
                    let bytes: i64 = row.get("size");
                    let gb = bytes as f64 / 1_073_741_824.0;
                    let status = if gb > 5.0 { "red" } else if gb > 3.0 { "yellow" } else { "green" };
                    (status, Some(format!("{gb:.2} GB")), None)
                }
                Err(e) => ("red", None, Some(format!("Query failed: {e}"))),
            }
        }
        Err(e) => ("red", None, Some(format!("Pool error: {e}"))),
    };

    // Check logs DB size
    let logs_size = match state.logs_db.get().await {
        Ok(client) => {
            match client
                .query_one(
                    "SELECT pg_database_size(current_database()) as size",
                    &[],
                )
                .await
            {
                Ok(row) => {
                    let bytes: i64 = row.get("size");
                    let gb = bytes as f64 / 1_073_741_824.0;
                    let status = if gb > 2.0 { "red" } else if gb > 1.0 { "yellow" } else { "green" };
                    (status, Some(format!("{gb:.2} GB")), None)
                }
                Err(e) => ("red", None, Some(format!("Query failed: {e}"))),
            }
        }
        Err(e) => ("red", None, Some(format!("Pool error: {e}"))),
    };

    // Check logs retention (oldest row in perf_log)
    let retention_status = match state.logs_db.get().await {
        Ok(client) => {
            match client
                .query_opt(
                    "SELECT MIN(logged_at) as oldest FROM perf_log",
                    &[],
                )
                .await
            {
                Ok(Some(row)) => {
                    let oldest: Option<chrono::DateTime<chrono::Utc>> = row.get("oldest");
                    match oldest {
                        Some(ts) => {
                            let age_days = (chrono::Utc::now() - ts).num_days();
                            let status = if age_days > 8 { "red" } else if age_days > 7 { "yellow" } else { "green" };
                            (status, Some(format!("{age_days} days")), None)
                        }
                        None => ("green", Some("empty".into()), None),
                    }
                }
                Ok(None) => ("green", Some("empty".into()), None),
                Err(e) => ("red", None, Some(format!("Query failed: {e}"))),
            }
        }
        Err(e) => ("red", None, Some(format!("Pool error: {e}"))),
    };

    // Persist results to core DB
    if let Ok(client) = state.db.get().await {
        let checks: Vec<(&str, &str, Option<&str>, Option<&str>)> = vec![
            ("core_db", core_status.0, None, core_status.1.as_deref()),
            ("logs_db", logs_status.0, None, logs_status.1.as_deref()),
            ("core_db_size", core_size.0, core_size.1.as_deref(), core_size.2.as_deref()),
            ("logs_db_size", logs_size.0, logs_size.1.as_deref(), logs_size.2.as_deref()),
            ("logs_retention", retention_status.0, retention_status.1.as_deref(), retention_status.2.as_deref()),
        ];

        for (name, status, value, message) in &checks {
            if let Err(e) = crate::db::system_health::insert(&client, name, status, *value, *message, None).await {
                tracing::error!(error = %e, check = name, "Failed to persist health check");
            }
        }

        // Cleanup old health records
        if let Err(e) = crate::db::system_health::cleanup(&client).await {
            tracing::error!(error = %e, "Failed to cleanup old health records");
        }
    }

    // Log any red checks
    if core_status.0 == "red" {
        tracing::warn!("Health check FAILED: core_db — {:?}", core_status.1);
    }
    if logs_status.0 == "red" {
        tracing::warn!("Health check FAILED: logs_db — {:?}", logs_status.1);
    }
}
```

- [ ] **Step 4: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/scheduler.rs
git commit -m "feat(dashboard): hourly health checks — DB connectivity, size, retention"
```

---

### Task 10: Create /api/system/health endpoint

**Files:**
- Create: `crates/networker-dashboard/src/api/system_health.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`

- [ ] **Step 1: Create the API module**

Create `crates/networker-dashboard/src/api/system_health.rs`:

```rust
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use std::sync::Arc;

use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/system/health", get(get_health))
        .with_state(state)
}

/// GET /api/system/health — admin-only system health overview.
async fn get_health(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in system health");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let checks = crate::db::system_health::latest_all(&client)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to read health checks");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Also do a live connectivity check right now
    let core_live = state.db.get().await.is_ok();
    let logs_live = state.logs_db.get().await.is_ok();

    Ok(Json(serde_json::json!({
        "live": {
            "core_db": core_live,
            "logs_db": logs_live,
        },
        "checks": checks,
    })))
}
```

- [ ] **Step 2: Register the router in api/mod.rs**

In `crates/networker-dashboard/src/api/mod.rs`, add at the top:

```rust
mod system_health;
```

In the `router()` function, add to the `protected_flat` section (after the `admin::router` line):

```rust
.merge(system_health::router(state.clone()))
```

- [ ] **Step 3: Run `cargo build -p networker-dashboard`**

Run: `cargo build -p networker-dashboard`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/api/system_health.rs crates/networker-dashboard/src/api/mod.rs
git commit -m "feat(dashboard): GET /api/system/health endpoint"
```

---

## Phase 3: Infrastructure Scripts

### Task 11: Create production migration script

**Files:**
- Create: `scripts/migrate-to-split.sh`

- [ ] **Step 1: Write the migration script**

Create `scripts/migrate-to-split.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# migrate-to-split.sh — One-time migration: networker_dashboard → networker_core + networker_logs
#
# Run this on the db-server during a maintenance window.
# Requires: pg_dump, pg_restore, psql, all on the db-server.
#
# Usage: ./migrate-to-split.sh [--db-host localhost] [--db-user networker] [--db-port 5432]

DB_HOST="${DB_HOST:-localhost}"
DB_USER="${DB_USER:-networker}"
DB_PORT="${DB_PORT:-5432}"
SOURCE_DB="networker_dashboard"
CORE_DB="networker_core"
LOGS_DB="networker_logs"
LOG_TABLES="benchmark_request_progress perf_log"

# Parse flags
while [[ $# -gt 0 ]]; do
    case "$1" in
        --db-host) DB_HOST="$2"; shift 2 ;;
        --db-user) DB_USER="$2"; shift 2 ;;
        --db-port) DB_PORT="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

PSQL="psql -h $DB_HOST -U $DB_USER -p $DB_PORT"
PG_DUMP="pg_dump -h $DB_HOST -U $DB_USER -p $DB_PORT"

echo "=== Database Split Migration ==="
echo "Source: $SOURCE_DB"
echo "Core:   $CORE_DB"
echo "Logs:   $LOGS_DB"
echo "Host:   $DB_HOST:$DB_PORT"
echo ""

# Pre-flight check: source DB must exist
$PSQL -d "$SOURCE_DB" -c "SELECT 1;" > /dev/null 2>&1 || {
    echo "ERROR: Source database '$SOURCE_DB' not reachable."
    exit 1
}

# Pre-flight check: core DB must NOT exist
if $PSQL -d "$CORE_DB" -c "SELECT 1;" > /dev/null 2>&1; then
    echo "ERROR: Target database '$CORE_DB' already exists. Aborting."
    exit 1
fi

echo "Step 1/8: Creating $CORE_DB..."
$PSQL -d postgres -c "CREATE DATABASE $CORE_DB OWNER $DB_USER;" < /dev/null

echo "Step 2/8: Copying schema + data from $SOURCE_DB to $CORE_DB..."
$PG_DUMP -d "$SOURCE_DB" -Fc | pg_restore -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" -d "$CORE_DB" --no-owner --no-acl < /dev/null

echo "Step 3/8: Creating $LOGS_DB..."
$PSQL -d postgres -c "CREATE DATABASE $LOGS_DB OWNER $DB_USER;" < /dev/null

echo "Step 4/8: Copying log table schemas to $LOGS_DB..."
for table in $LOG_TABLES; do
    echo "  - $table"
    $PG_DUMP -d "$SOURCE_DB" -Fc --table="$table" --schema-only | \
        pg_restore -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" -d "$LOGS_DB" --no-owner --no-acl < /dev/null
done

echo "Step 5/8: Copying log data to $LOGS_DB..."
for table in $LOG_TABLES; do
    count=$($PSQL -d "$SOURCE_DB" -t -c "SELECT COUNT(*) FROM $table;" < /dev/null | tr -d ' ')
    echo "  - $table: $count rows"
    if [ "$count" -gt 0 ]; then
        $PG_DUMP -d "$SOURCE_DB" -Fc --table="$table" --data-only | \
            pg_restore -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" -d "$LOGS_DB" --no-owner --no-acl < /dev/null
    fi
done

echo "Step 6/8: Dropping log tables from $CORE_DB..."
for table in $LOG_TABLES; do
    echo "  - DROP TABLE $table"
    $PSQL -d "$CORE_DB" -c "DROP TABLE IF EXISTS $table CASCADE;" < /dev/null
done

echo "Step 7/8: Verifying..."
core_tables=$($PSQL -d "$CORE_DB" -t -c "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public';" < /dev/null | tr -d ' ')
logs_tables=$($PSQL -d "$LOGS_DB" -t -c "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public';" < /dev/null | tr -d ' ')
echo "  Core DB: $core_tables tables"
echo "  Logs DB: $logs_tables tables"

echo "Step 8/8: Migration complete."
echo ""
echo "Next steps:"
echo "  1. Update DASHBOARD_DB_URL to point to $CORE_DB"
echo "  2. Set DASHBOARD_LOGS_DB_URL to point to $LOGS_DB"
echo "  3. Restart the dashboard application"
echo "  4. After verification, drop old database:"
echo "     $PSQL -d postgres -c 'DROP DATABASE $SOURCE_DB;'"
```

- [ ] **Step 2: Make executable and shellcheck**

```bash
chmod +x scripts/migrate-to-split.sh
shellcheck scripts/migrate-to-split.sh
```

Expected: No errors (or minor suggestions).

- [ ] **Step 3: Commit**

```bash
git add scripts/migrate-to-split.sh
git commit -m "feat(scripts): one-time database split migration script"
```

---

### Task 12: Create daily backup script

**Files:**
- Create: `scripts/backup-daily.sh`

- [ ] **Step 1: Write the backup script**

Create `scripts/backup-daily.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# backup-daily.sh — Daily pg_dump of networker_core to Azure Blob Storage.
#
# Intended to run as a cron job on db-server:
#   0 2 * * * /opt/networker/scripts/backup-daily.sh >> /var/log/networker-backup.log 2>&1
#
# Requires: pg_dump, azcopy (or az storage blob upload), gzip, jq
#
# Environment variables:
#   BACKUP_DB_HOST      (default: localhost)
#   BACKUP_DB_USER      (default: backup_user)
#   BACKUP_DB_PORT      (default: 5432)
#   BACKUP_DB_NAME      (default: networker_core)
#   BACKUP_BLOB_URL     (required: Azure Blob container SAS URL, e.g. https://account.blob.core.windows.net/backups?sv=...)
#   BACKUP_RETAIN_DAYS  (default: 30)

DB_HOST="${BACKUP_DB_HOST:-localhost}"
DB_USER="${BACKUP_DB_USER:-backup_user}"
DB_PORT="${BACKUP_DB_PORT:-5432}"
DB_NAME="${BACKUP_DB_NAME:-networker_core}"
BLOB_URL="${BACKUP_BLOB_URL:?BACKUP_BLOB_URL is required}"
RETAIN_DAYS="${BACKUP_RETAIN_DAYS:-30}"

DATE=$(date -u +%Y-%m-%d)
MONTH=$(date -u +%Y-%m)
DAY_OF_MONTH=$(date -u +%d)
TMPDIR="${TMPDIR:-/tmp}"
DUMP_FILE="$TMPDIR/networker_core_$DATE.sql.gz"

echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) Starting daily backup of $DB_NAME..."

# Step 1: pg_dump → gzip
pg_dump -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" -d "$DB_NAME" -Fc | gzip > "$DUMP_FILE"
DUMP_SIZE=$(stat -f%z "$DUMP_FILE" 2>/dev/null || stat -c%s "$DUMP_FILE" 2>/dev/null)
CHECKSUM=$(sha256sum "$DUMP_FILE" | cut -d' ' -f1)

echo "  Dump size: $DUMP_SIZE bytes"
echo "  Checksum: $CHECKSUM"

# Step 2: Upload to daily folder
azcopy copy "$DUMP_FILE" "$BLOB_URL/daily/$DATE.sql.gz" --log-level=ERROR < /dev/null

# Step 3: If 1st of month, also copy to monthly folder
if [ "$DAY_OF_MONTH" = "01" ]; then
    echo "  First of month — copying to monthly/$MONTH.sql.gz"
    azcopy copy "$DUMP_FILE" "$BLOB_URL/monthly/$MONTH.sql.gz" --log-level=ERROR < /dev/null
fi

# Step 4: Write marker file
MARKER="$TMPDIR/last_backup.json"
cat > "$MARKER" << JSONEOF
{
    "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
    "date": "$DATE",
    "database": "$DB_NAME",
    "size_bytes": $DUMP_SIZE,
    "checksum_sha256": "$CHECKSUM",
    "blob_path": "daily/$DATE.sql.gz"
}
JSONEOF

azcopy copy "$MARKER" "$BLOB_URL/last_backup.json" --log-level=ERROR < /dev/null

# Step 5: Cleanup local temp file
rm -f "$DUMP_FILE" "$MARKER"

echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) Backup complete: daily/$DATE.sql.gz ($DUMP_SIZE bytes)"
```

- [ ] **Step 2: Make executable and shellcheck**

```bash
chmod +x scripts/backup-daily.sh
shellcheck scripts/backup-daily.sh
```

- [ ] **Step 3: Commit**

```bash
git add scripts/backup-daily.sh
git commit -m "feat(scripts): daily pg_dump backup to Azure Blob with GFS retention"
```

---

### Task 13: Create retention cleanup script

**Files:**
- Create: `scripts/retention-cleanup.sh`

- [ ] **Step 1: Write the retention script**

Create `scripts/retention-cleanup.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# retention-cleanup.sh — Delete logs older than 7 days from networker_logs.
#
# Intended to run as a cron job on db-server:
#   0 3 * * * /opt/networker/scripts/retention-cleanup.sh >> /var/log/networker-retention.log 2>&1
#
# Environment variables:
#   LOGS_DB_HOST      (default: localhost)
#   LOGS_DB_USER      (default: networker)
#   LOGS_DB_PORT      (default: 5432)
#   LOGS_DB_NAME      (default: networker_logs)
#   RETAIN_DAYS       (default: 7)
#   BATCH_SIZE        (default: 10000)

DB_HOST="${LOGS_DB_HOST:-localhost}"
DB_USER="${LOGS_DB_USER:-networker}"
DB_PORT="${LOGS_DB_PORT:-5432}"
DB_NAME="${LOGS_DB_NAME:-networker_logs}"
RETAIN_DAYS="${RETAIN_DAYS:-7}"
BATCH_SIZE="${BATCH_SIZE:-10000}"

PSQL="psql -h $DB_HOST -U $DB_USER -p $DB_PORT -d $DB_NAME"

echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) Starting retention cleanup (retain ${RETAIN_DAYS}d)..."

# Batched delete for benchmark_request_progress
total_deleted=0
while true; do
    deleted=$($PSQL -t -c "
        WITH to_delete AS (
            SELECT ctid FROM benchmark_request_progress
            WHERE created_at < now() - interval '${RETAIN_DAYS} days'
            LIMIT $BATCH_SIZE
        )
        DELETE FROM benchmark_request_progress
        WHERE ctid IN (SELECT ctid FROM to_delete);
        SELECT ROW_COUNT();
    " < /dev/null 2>/dev/null | tr -d ' ')
    # Fallback: use a simpler approach if ROW_COUNT doesn't work
    deleted=$($PSQL -t -c "
        DELETE FROM benchmark_request_progress
        WHERE ctid IN (
            SELECT ctid FROM benchmark_request_progress
            WHERE created_at < now() - interval '${RETAIN_DAYS} days'
            LIMIT $BATCH_SIZE
        );
    " < /dev/null 2>&1 | grep -oP 'DELETE \K[0-9]+' || echo "0")
    total_deleted=$((total_deleted + deleted))
    [ "$deleted" -lt "$BATCH_SIZE" ] && break
    sleep 1
done
echo "  benchmark_request_progress: $total_deleted rows deleted"

# Batched delete for perf_log
total_deleted=0
while true; do
    deleted=$($PSQL -t -c "
        DELETE FROM perf_log
        WHERE ctid IN (
            SELECT ctid FROM perf_log
            WHERE logged_at < now() - interval '${RETAIN_DAYS} days'
            LIMIT $BATCH_SIZE
        );
    " < /dev/null 2>&1 | grep -oP 'DELETE \K[0-9]+' || echo "0")
    total_deleted=$((total_deleted + deleted))
    [ "$deleted" -lt "$BATCH_SIZE" ] && break
    sleep 1
done
echo "  perf_log: $total_deleted rows deleted"

# Vacuum to reclaim space
echo "  Running VACUUM ANALYZE..."
$PSQL -c "VACUUM ANALYZE benchmark_request_progress;" < /dev/null
$PSQL -c "VACUUM ANALYZE perf_log;" < /dev/null

echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) Retention cleanup complete."
```

- [ ] **Step 2: Make executable and shellcheck**

```bash
chmod +x scripts/retention-cleanup.sh
shellcheck scripts/retention-cleanup.sh
```

- [ ] **Step 3: Commit**

```bash
git add scripts/retention-cleanup.sh
git commit -m "feat(scripts): 7-day log retention with batched deletes"
```

---

### Task 14: Create anonymization SQL

**Files:**
- Create: `scripts/anonymize.sql`

- [ ] **Step 1: Write the anonymization script**

Create `scripts/anonymize.sql`:

```sql
-- anonymize.sql — Scrub sensitive data for sandbox environments.
-- Run against a COPY of networker_core, never against production.

BEGIN;

-- Anonymize user identities
UPDATE dash_user SET
    email = 'user_' || user_id::text || '@sandbox.local',
    display_name = 'User ' || LEFT(user_id::text, 8),
    password_hash = '$argon2id$v=19$m=19456,t=2,p=1$sandbox$sandbox',
    avatar_url = NULL,
    password_reset_token = NULL,
    password_reset_expires = NULL;

-- Delete cloud credentials (never copy real creds to sandbox)
DELETE FROM cloud_account;
DELETE FROM cloud_connection;

-- Delete access tokens and invites
DELETE FROM workspace_invite;
DELETE FROM share_link;
DELETE FROM command_approval;

COMMIT;
```

- [ ] **Step 2: Commit**

```bash
git add scripts/anonymize.sql
git commit -m "feat(scripts): sandbox data anonymization SQL"
```

---

### Task 15: Create infra.sh DR CLI

**Files:**
- Create: `scripts/infra.sh`

- [ ] **Step 1: Write the DR CLI**

Create `scripts/infra.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# infra.sh — Disaster Recovery CLI for networker dashboard.
#
# Commands:
#   restore   — Restore production from backup
#   sandbox   — Create/destroy sandboxes with anonymized prod data
#   list      — List running sandboxes
#
# Usage:
#   ./infra.sh restore --env prod --confirm-production
#   ./infra.sh restore --env prod --db-only --confirm-production
#   ./infra.sh sandbox --name dev-igor
#   ./infra.sh sandbox --name dev-igor --destroy
#   ./infra.sh list

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RESOURCE_GROUP="${AZURE_RESOURCE_GROUP:-networker-rg}"
LOCATION="${AZURE_LOCATION:-eastus}"
VNET_NAME="${AZURE_VNET:-networker-vnet}"
SUBNET_NAME="${AZURE_SUBNET:-default}"
DB_IMAGE="${AZURE_DB_IMAGE:-networker-db-image}"
WEB_IMAGE="${AZURE_WEB_IMAGE:-networker-web-image}"
BLOB_URL="${BACKUP_BLOB_URL:?BACKUP_BLOB_URL is required}"
VM_SIZE="${AZURE_VM_SIZE:-Standard_B1ls}"

usage() {
    cat <<EOF
Usage: $0 <command> [options]

Commands:
    restore     Restore production from backup
    sandbox     Create or destroy a sandbox environment
    list        List running sandboxes

Restore options:
    --env prod              Target environment (only 'prod' supported)
    --db-only               Restore only the database server
    --confirm-production    Required safety flag for production restores

Sandbox options:
    --name <name>           Sandbox name (required)
    --destroy               Tear down the named sandbox
    --ttl <hours>           Auto-shutdown after N hours (default: 48)

EOF
    exit 1
}

# ── Restore ──────────────────────────────────────────────────────────────

cmd_restore() {
    local env="" db_only=false confirmed=false

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --env) env="$2"; shift 2 ;;
            --db-only) db_only=true; shift ;;
            --confirm-production) confirmed=true; shift ;;
            *) echo "Unknown option: $1"; usage ;;
        esac
    done

    if [ "$env" != "prod" ]; then
        echo "ERROR: --env prod is required."
        exit 1
    fi

    if [ "$confirmed" != "true" ]; then
        echo "ERROR: Production restore requires --confirm-production flag."
        exit 1
    fi

    echo "=== Production Restore ==="
    echo "Starting at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    local start_time=$SECONDS

    # Step 1: Provision db-server
    echo ""
    echo "Step 1: Provisioning db-server..."
    local db_vm="networker-db-$(date +%s)"
    az vm create \
        --resource-group "$RESOURCE_GROUP" \
        --name "$db_vm" \
        --image "$DB_IMAGE" \
        --size "$VM_SIZE" \
        --vnet-name "$VNET_NAME" \
        --subnet "$SUBNET_NAME" \
        --public-ip-address "" \
        --nsg "" \
        --admin-username azureuser \
        --generate-ssh-keys \
        --output none < /dev/null

    local db_private_ip
    db_private_ip=$(az vm show -g "$RESOURCE_GROUP" -n "$db_vm" -d --query privateIps -o tsv)
    echo "  db-server: $db_vm ($db_private_ip)"

    # Step 2: Download latest backup
    echo ""
    echo "Step 2: Downloading latest backup..."
    local marker_file="/tmp/last_backup.json"
    azcopy copy "$BLOB_URL/last_backup.json" "$marker_file" --log-level=ERROR < /dev/null
    local backup_date backup_path
    backup_date=$(jq -r '.date' "$marker_file")
    backup_path=$(jq -r '.blob_path' "$marker_file")
    echo "  Latest backup: $backup_date ($backup_path)"

    local dump_file="/tmp/restore_$backup_date.sql.gz"
    azcopy copy "$BLOB_URL/$backup_path" "$dump_file" --log-level=ERROR < /dev/null

    # Step 3: Restore core DB
    echo ""
    echo "Step 3: Restoring networker_core..."
    # SSH to db-server to run restore (via web-server as jump host)
    scp -o StrictHostKeyChecking=no "$dump_file" "azureuser@$db_private_ip:/tmp/" < /dev/null
    ssh -o StrictHostKeyChecking=no "azureuser@$db_private_ip" bash -s < /dev/null <<'REMOTE'
sudo -u postgres createdb networker_core
sudo -u postgres createdb networker_logs
gunzip -c /tmp/restore_*.sql.gz | sudo -u postgres pg_restore -d networker_core --no-owner --no-acl
rm -f /tmp/restore_*.sql.gz
REMOTE

    # Step 4: Apply security config
    echo ""
    echo "Step 4: Configuring PostgreSQL security..."
    ssh -o StrictHostKeyChecking=no "azureuser@$db_private_ip" bash -s < /dev/null <<'REMOTE'
# Create DB users
sudo -u postgres psql -c "CREATE USER app_core WITH PASSWORD '$(openssl rand -base64 24)';"
sudo -u postgres psql -c "CREATE USER app_logs WITH PASSWORD '$(openssl rand -base64 24)';"
sudo -u postgres psql -c "CREATE USER backup_user WITH PASSWORD '$(openssl rand -base64 24)';"
sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE networker_core TO app_core;"
sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE networker_logs TO app_logs;"
sudo -u postgres psql -c "GRANT CONNECT ON DATABASE networker_core TO backup_user;"

# Enable SSL
sudo -u postgres psql -c "ALTER SYSTEM SET ssl = on;"
sudo systemctl restart postgresql
REMOTE

    if [ "$db_only" = "false" ]; then
        # Step 5: Provision web-server
        echo ""
        echo "Step 5: Provisioning web-server..."
        local web_vm="networker-web-$(date +%s)"
        az vm create \
            --resource-group "$RESOURCE_GROUP" \
            --name "$web_vm" \
            --image "$WEB_IMAGE" \
            --size "$VM_SIZE" \
            --vnet-name "$VNET_NAME" \
            --subnet "$SUBNET_NAME" \
            --admin-username azureuser \
            --generate-ssh-keys \
            --output none < /dev/null

        local web_public_ip
        web_public_ip=$(az vm show -g "$RESOURCE_GROUP" -n "$web_vm" -d --query publicIps -o tsv)
        echo "  web-server: $web_vm ($web_public_ip)"

        # Step 6: Deploy and configure
        echo ""
        echo "Step 6: Deploying application..."
        local latest_tag
        latest_tag=$(gh release view --json tagName -q '.tagName' 2>/dev/null || echo "latest")
        ssh -o StrictHostKeyChecking=no "azureuser@$web_public_ip" bash -s "$db_private_ip" "$latest_tag" < /dev/null <<'REMOTE'
DB_IP="$1"
TAG="$2"
# Download latest release binaries
gh release download "$TAG" -p "networker-dashboard-*-linux-*" -D /opt/networker/bin/ 2>/dev/null || true
gh release download "$TAG" -p "networker-endpoint-*-linux-*" -D /opt/networker/bin/ 2>/dev/null || true

# Configure connection strings
cat > /opt/networker/.env <<ENVEOF
DASHBOARD_DB_URL=postgres://app_core:password@$DB_IP:5432/networker_core
DASHBOARD_LOGS_DB_URL=postgres://app_logs:password@$DB_IP:5432/networker_logs
ENVEOF

# Restart services
sudo systemctl restart networker-dashboard networker-endpoint
REMOTE
        echo "  Web server deployed: http://$web_public_ip:3000"
    fi

    # Step 7: Health check
    echo ""
    echo "Step 7: Running health checks..."
    sleep 10
    if [ "$db_only" = "false" ]; then
        local health_url="http://${web_public_ip:-localhost}:3000/api/system/health"
        if curl -sf "$health_url" > /dev/null 2>&1; then
            echo "  Health check: PASSED"
        else
            echo "  Health check: FAILED (service may still be starting)"
        fi
    fi

    local elapsed=$(( SECONDS - start_time ))
    echo ""
    echo "=== Restore complete in ${elapsed}s ==="
    echo "  db-server: $db_vm ($db_private_ip)"
    [ "$db_only" = "false" ] && echo "  web-server: $web_vm ($web_public_ip)"

    rm -f "$dump_file" "$marker_file"
}

# ── Sandbox ──────────────────────────────────────────────────────────────

cmd_sandbox() {
    local name="" destroy=false ttl=48

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name) name="$2"; shift 2 ;;
            --destroy) destroy=true; shift ;;
            --ttl) ttl="$2"; shift 2 ;;
            *) echo "Unknown option: $1"; usage ;;
        esac
    done

    if [ -z "$name" ]; then
        echo "ERROR: --name is required."
        exit 1
    fi

    local vm_name="sandbox-$name"

    if [ "$destroy" = "true" ]; then
        echo "Destroying sandbox: $vm_name"
        az vm delete -g "$RESOURCE_GROUP" -n "$vm_name" --yes --force-deletion true < /dev/null
        az network nic delete -g "$RESOURCE_GROUP" -n "${vm_name}VMNic" < /dev/null 2>/dev/null || true
        az disk delete -g "$RESOURCE_GROUP" -n "${vm_name}OsDisk" --yes < /dev/null 2>/dev/null || true
        echo "Sandbox $name destroyed."
        return
    fi

    echo "=== Creating sandbox: $name ==="

    # Step 1: Provision single VM
    echo "Step 1: Provisioning sandbox VM..."
    az vm create \
        --resource-group "$RESOURCE_GROUP" \
        --name "$vm_name" \
        --image "$DB_IMAGE" \
        --size "$VM_SIZE" \
        --admin-username azureuser \
        --generate-ssh-keys \
        --tags env=sandbox ttl="$ttl" created="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        --output none < /dev/null

    local sandbox_ip
    sandbox_ip=$(az vm show -g "$RESOURCE_GROUP" -n "$vm_name" -d --query publicIps -o tsv)

    # Step 2: Auto-shutdown
    echo "Step 2: Setting auto-shutdown (${ttl}h)..."
    local shutdown_time
    shutdown_time=$(date -u -d "+${ttl} hours" +%H%M 2>/dev/null || date -u -v+${ttl}H +%H%M)
    az vm auto-shutdown \
        -g "$RESOURCE_GROUP" \
        -n "$vm_name" \
        --time "$shutdown_time" \
        --output none < /dev/null 2>/dev/null || true

    # Step 3: Restore + anonymize
    echo "Step 3: Restoring and anonymizing data..."
    local marker_file="/tmp/last_backup.json"
    azcopy copy "$BLOB_URL/last_backup.json" "$marker_file" --log-level=ERROR < /dev/null
    local backup_path
    backup_path=$(jq -r '.blob_path' "$marker_file")

    local dump_file="/tmp/sandbox_restore.sql.gz"
    azcopy copy "$BLOB_URL/$backup_path" "$dump_file" --log-level=ERROR < /dev/null

    scp -o StrictHostKeyChecking=no "$dump_file" "azureuser@$sandbox_ip:/tmp/" < /dev/null
    scp -o StrictHostKeyChecking=no "$SCRIPT_DIR/anonymize.sql" "azureuser@$sandbox_ip:/tmp/" < /dev/null

    ssh -o StrictHostKeyChecking=no "azureuser@$sandbox_ip" bash -s < /dev/null <<'REMOTE'
sudo -u postgres createdb networker_core
sudo -u postgres createdb networker_logs
gunzip -c /tmp/sandbox_restore.sql.gz | sudo -u postgres pg_restore -d networker_core --no-owner --no-acl
sudo -u postgres psql -d networker_core -f /tmp/anonymize.sql
rm -f /tmp/sandbox_restore.sql.gz /tmp/anonymize.sql
REMOTE

    # Step 4: Deploy app
    echo "Step 4: Deploying application..."
    ssh -o StrictHostKeyChecking=no "azureuser@$sandbox_ip" bash -s < /dev/null <<'REMOTE'
cat > /opt/networker/.env <<ENVEOF
DASHBOARD_DB_URL=postgres://networker:networker@localhost:5432/networker_core
DASHBOARD_LOGS_DB_URL=postgres://networker:networker@localhost:5432/networker_logs
DASHBOARD_JWT_SECRET=$(openssl rand -hex 32)
DASHBOARD_ADMIN_PASSWORD=sandbox-password-123
ENVEOF
sudo systemctl restart networker-dashboard networker-endpoint 2>/dev/null || true
REMOTE

    echo ""
    echo "=== Sandbox ready ==="
    echo "  Name: $name"
    echo "  URL: http://$sandbox_ip:3000"
    echo "  Login: any user@sandbox.local / (use sandbox password)"
    echo "  Auto-shutdown: ${ttl}h"
    echo "  Destroy: $0 sandbox --name $name --destroy"

    rm -f "$dump_file" "$marker_file"
}

# ── List ─────────────────────────────────────────────────────────────────

cmd_list() {
    echo "=== Running Sandboxes ==="
    az vm list -g "$RESOURCE_GROUP" \
        --query "[?tags.env=='sandbox'].{Name:name, IP:publicIps, Created:tags.created, TTL:tags.ttl, Status:powerState}" \
        -d -o table < /dev/null
}

# ── Main ─────────────────────────────────────────────────────────────────

if [ $# -lt 1 ]; then
    usage
fi

COMMAND="$1"
shift

case "$COMMAND" in
    restore) cmd_restore "$@" ;;
    sandbox) cmd_sandbox "$@" ;;
    list)    cmd_list "$@" ;;
    *)       echo "Unknown command: $COMMAND"; usage ;;
esac
```

- [ ] **Step 2: Make executable and shellcheck**

```bash
chmod +x scripts/infra.sh
shellcheck scripts/infra.sh
```

Fix any shellcheck findings (likely SC2086 for word splitting — quote variables).

- [ ] **Step 3: Commit**

```bash
git add scripts/infra.sh
git commit -m "feat(scripts): infra.sh DR CLI — restore, sandbox, list"
```

---

## Phase 4: Frontend System Health Panel

### Task 16: Create SystemHealthPanel component

**Files:**
- Create: `dashboard/src/components/SystemHealthPanel.tsx`

- [ ] **Step 1: Create the component**

Create `dashboard/src/components/SystemHealthPanel.tsx`:

```tsx
import { useState, useEffect, useCallback } from "react";

interface HealthCheck {
  check_name: string;
  status: string;
  value: string | null;
  message: string | null;
  checked_at: string;
}

interface HealthData {
  live: { core_db: boolean; logs_db: boolean };
  checks: HealthCheck[];
}

const STATUS_COLORS: Record<string, string> = {
  green: "text-emerald-400",
  yellow: "text-yellow-400",
  red: "text-red-400",
};

const STATUS_DOT: Record<string, string> = {
  green: "bg-emerald-400",
  yellow: "bg-yellow-400",
  red: "bg-red-400",
};

const CHECK_LABELS: Record<string, string> = {
  core_db: "Core Database",
  logs_db: "Logs Database",
  core_db_size: "Core DB Size",
  logs_db_size: "Logs DB Size",
  logs_retention: "Logs Retention",
  last_backup: "Last Backup",
};

export default function SystemHealthPanel() {
  const [health, setHealth] = useState<HealthData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchHealth = useCallback(async () => {
    try {
      const token = localStorage.getItem("token");
      const res = await fetch("/api/system/health", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data: HealthData = await res.json();
      setHealth(data);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load health data");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchHealth();
    const interval = setInterval(fetchHealth, 60_000);
    return () => clearInterval(interval);
  }, [fetchHealth]);

  if (loading) {
    return (
      <div className="border border-zinc-700/50 rounded-lg p-4">
        <h3 className="text-sm font-medium text-zinc-400 mb-3">System Health</h3>
        <p className="text-xs text-zinc-500">Loading...</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="border border-red-800/50 rounded-lg p-4">
        <h3 className="text-sm font-medium text-zinc-400 mb-3">System Health</h3>
        <p className="text-xs text-red-400">{error}</p>
      </div>
    );
  }

  const overallStatus = health?.checks.some((c) => c.status === "red")
    ? "red"
    : health?.checks.some((c) => c.status === "yellow")
      ? "yellow"
      : "green";

  return (
    <div className="border border-zinc-700/50 rounded-lg p-4">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-zinc-400">System Health</h3>
        <div className="flex items-center gap-1.5">
          <span
            className={`inline-block w-2 h-2 rounded-full ${STATUS_DOT[overallStatus]}`}
          />
          <span className={`text-xs ${STATUS_COLORS[overallStatus]}`}>
            {overallStatus === "green"
              ? "All systems operational"
              : overallStatus === "yellow"
                ? "Degraded"
                : "Issues detected"}
          </span>
        </div>
      </div>

      {/* Live connectivity */}
      <div className="flex gap-4 mb-3 text-xs">
        <span className={health?.live.core_db ? "text-emerald-400" : "text-red-400"}>
          Core DB: {health?.live.core_db ? "connected" : "down"}
        </span>
        <span className={health?.live.logs_db ? "text-emerald-400" : "text-red-400"}>
          Logs DB: {health?.live.logs_db ? "connected" : "down"}
        </span>
      </div>

      {/* Check details */}
      <div className="space-y-1.5">
        {health?.checks.map((check) => (
          <div
            key={check.check_name}
            className="flex items-center justify-between text-xs"
          >
            <div className="flex items-center gap-1.5">
              <span
                className={`inline-block w-1.5 h-1.5 rounded-full ${STATUS_DOT[check.status] ?? "bg-zinc-500"}`}
              />
              <span className="text-zinc-300">
                {CHECK_LABELS[check.check_name] ?? check.check_name}
              </span>
            </div>
            <div className="flex items-center gap-2">
              {check.value && (
                <span className="text-zinc-400 font-mono">{check.value}</span>
              )}
              {check.message && (
                <span className="text-zinc-500 truncate max-w-48" title={check.message}>
                  {check.message}
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      {/* Last checked timestamp */}
      {health?.checks[0] && (
        <p className="text-[10px] text-zinc-600 mt-2">
          Last checked:{" "}
          {new Date(health.checks[0].checked_at).toLocaleString()}
        </p>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Run `npm run build` in dashboard**

Run: `cd dashboard && npm run build`
Expected: Compiles (component not imported yet, tree-shaken out).

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/components/SystemHealthPanel.tsx
git commit -m "feat(frontend): SystemHealthPanel component for Settings page"
```

---

### Task 17: Add SystemHealthPanel to Settings page

**Files:**
- Modify: `dashboard/src/pages/SettingsPage.tsx`

- [ ] **Step 1: Import the component**

At the top of `dashboard/src/pages/SettingsPage.tsx`, add:

```tsx
import SystemHealthPanel from "../components/SystemHealthPanel";
```

- [ ] **Step 2: Add the panel to the page**

Find the beginning of the JSX return in SettingsPage. Add the `SystemHealthPanel` at the top of the page content (before the existing version info section):

```tsx
<SystemHealthPanel />
```

The exact insertion point depends on the current page structure — place it as the first card/section so it's immediately visible.

- [ ] **Step 3: Run lint and build**

```bash
cd dashboard && npm run lint && npm run build
```

Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/pages/SettingsPage.tsx
git commit -m "feat(frontend): add System Health panel to Settings page"
```

---

## Phase 5: Lint, Build, Test

### Task 18: Full workspace validation

- [ ] **Step 1: Format and lint Rust**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Fix any warnings.

- [ ] **Step 2: Build workspace**

```bash
cargo build --workspace
```

- [ ] **Step 3: Run unit tests**

```bash
cargo test --workspace --lib
```

- [ ] **Step 4: Build frontend**

```bash
cd dashboard && npm run lint && npm run build
```

- [ ] **Step 5: Shellcheck all scripts**

```bash
shellcheck scripts/migrate-to-split.sh scripts/backup-daily.sh scripts/retention-cleanup.sh scripts/infra.sh
```

- [ ] **Step 6: Fix any issues found, then commit**

```bash
git add -A
git commit -m "chore: fix lint warnings from database separation work"
```

---

### Task 19: Version bump + CHANGELOG

**Files:**
- Modify: `Cargo.toml` (workspace version)
- Modify: `CHANGELOG.md`
- Modify: `install.sh` (INSTALLER_VERSION)
- Modify: `install.ps1` (INSTALLER_VERSION)

- [ ] **Step 1: Bump version to 0.22.0**

This is a significant feature (new architecture), so minor version bump.

In `Cargo.toml` workspace section, change version to `"0.22.0"`.

In `install.sh`, update `INSTALLER_VERSION="0.22.0"`.

In `install.ps1`, update `$INSTALLER_VERSION = "0.22.0"`.

- [ ] **Step 2: Add CHANGELOG entry**

Add at the top of CHANGELOG.md (after the header):

```markdown
## [0.22.0] - 2026-04-07

### Added
- **Database separation**: split into `networker_core` (permanent, backed up) and `networker_logs` (7-day retention)
- **Dual connection pools**: `DASHBOARD_DB_URL` for core, `DASHBOARD_LOGS_DB_URL` for logs
- **System health monitoring**: `/api/system/health` endpoint, hourly automated checks, Settings page health panel
- **V026 migration**: `system_health` table for tracking DB connectivity, size, retention, backup status
- **Production migration script**: `scripts/migrate-to-split.sh` — one-time split from `networker_dashboard`
- **Daily backup script**: `scripts/backup-daily.sh` — pg_dump to Azure Blob with GFS retention (30 daily, 12 monthly)
- **Log retention script**: `scripts/retention-cleanup.sh` — batched 7-day cleanup for logs DB
- **DR CLI**: `scripts/infra.sh` — restore production, create/destroy sandboxes with anonymized data
- **Data anonymization**: `scripts/anonymize.sql` for sandbox environments
- **Docker dual-database**: `docker-compose.dashboard.yml` now creates both `networker_core` and `networker_logs`
```

- [ ] **Step 3: Run `cargo generate-lockfile` and commit**

```bash
cargo generate-lockfile
git add Cargo.toml Cargo.lock CHANGELOG.md install.sh install.ps1
git commit -m "chore: bump version to 0.22.0 — database separation + DR"
```
