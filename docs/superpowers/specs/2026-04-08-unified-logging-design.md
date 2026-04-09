# Unified Logging with TimescaleDB

**Date:** 2026-04-08
**Status:** Approved
**Author:** Igor + Claude

---

## Problem

Logging across the workspace is fragmented:

- 5 crates each initialize their own `tracing_subscriber::fmt()` with different destinations (stdout vs stderr)
- The dashboard has a 1000-entry in-memory ring buffer (`LogBuffer`) that loses structured fields
- The orchestrator's tracing output went to stdout while the worker captured stderr — making errors invisible
- No persistence of service logs to database — when the buffer rotates, logs are gone
- No cross-service correlation (dashboard → orchestrator → agent)
- No way to query historical logs for a specific benchmark config
- The benchmark progress page shows "No log output was captured" because logs are only delivered via WebSocket (ephemeral)

## Solution

A shared `networker-log` crate that provides a unified tracing layer writing to both console and TimescaleDB. Every crate replaces its ad-hoc tracing init with a builder that routes logs to the right destinations.

---

## Architecture

### Components

1. **`networker-log` crate** — shared library providing:
   - Custom tracing `Layer` that writes to TimescaleDB
   - `LogBuilder` API for initializing logging in any crate
   - DB schema migration for the `service_log` hypertable
   - Query functions for the dashboard API

2. **TimescaleDB** — extension on the existing `networker_logs` PostgreSQL database. One hypertable with automatic 7-day retention.

3. **Dashboard UI** — System > Logs tab replaced with TimescaleDB-backed page. Benchmark progress page queries by `config_id`.

### Data Flow

```
Any crate
  └─ tracing::info!(config_id = %id, "VM provisioned")
       ├─ Console Layer → stderr (or stdout for CLI)
       └─ DB Layer → channel → batch flush (500ms / 100 entries)
                                    └─ INSERT INTO service_log
                                         └─ TimescaleDB hypertable
                                              └─ auto-drop chunks > 7 days

Dashboard UI
  └─ GET /api/logs?service=orchestrator&level=error&config_id=UUID
       └─ SELECT FROM service_log WHERE ...
```

---

## Data Model

### `service_log` hypertable

```sql
CREATE TABLE service_log (
    ts          TIMESTAMPTZ     NOT NULL DEFAULT now(),
    service     TEXT            NOT NULL,
    level       SMALLINT        NOT NULL,   -- 1=ERROR, 2=WARN, 3=INFO, 4=DEBUG, 5=TRACE
    message     TEXT            NOT NULL,
    config_id   UUID,
    project_id  CHAR(14),
    trace_id    UUID,
    fields      JSONB
);

SELECT create_hypertable('service_log', 'ts', chunk_time_interval => INTERVAL '1 day');

CREATE INDEX ix_service_log_service ON service_log (service, ts DESC);
CREATE INDEX ix_service_log_level   ON service_log (level, ts DESC);
CREATE INDEX ix_service_log_config  ON service_log (config_id, ts DESC) WHERE config_id IS NOT NULL;
CREATE INDEX ix_service_log_project ON service_log (project_id, ts DESC) WHERE project_id IS NOT NULL;

SELECT add_retention_policy('service_log', INTERVAL '7 days');
```

**Design decisions:**
- `level` as SMALLINT (not TEXT) — faster filtering, less storage
- `fields` JSONB without GIN index — queries on specific fields use `fields->>'key'` within a time-bounded window, fast enough without the index overhead at scale
- 1-day chunk interval — retention drops one chunk per day cleanly
- No `id` column — logs are append-only, never referenced by PK

---

## `networker-log` Crate API

### LogBuilder

```rust
use networker_log::{LogBuilder, Stream};

// Dashboard (always DB + console):
LogBuilder::new("dashboard")
    .with_console(Stream::Stderr)
    .with_db(&logs_db_url)
    .init()?;

// Orchestrator (DB + stderr, config_id context):
LogBuilder::new("orchestrator")
    .with_console(Stream::Stderr)
    .with_db(&logs_db_url)
    .with_context("config_id", &config_id)
    .with_context("project_id", &project_id)
    .init()?;

// CLI tester (console only, unless --log-db-url passed):
let mut builder = LogBuilder::new("tester")
    .with_console(Stream::Stderr);
if let Some(url) = log_db_url {
    builder = builder.with_db(&url);
}
builder.init()?;
```

### Internals

- `init()` composes a `tracing_subscriber::Registry` with:
  - `EnvFilter` layer (respects `RUST_LOG`, defaults to "info")
  - Console `fmt::Layer` writing to the chosen stream
  - Optional `DbLayer` writing to TimescaleDB
- `with_context()` sets fields attached to every log entry from this process
- `DbLayer` collects entries in an `mpsc` channel; a background tokio task flushes every 500ms or 100 entries (whichever first)
- If the DB is unreachable, entries are dropped (logging must not block the application)
- `init()` returns a `LogGuard` that flushes on drop (graceful shutdown)

### Crate Structure

```
crates/networker-log/
  Cargo.toml
  src/
    lib.rs          — public API (LogBuilder, Stream, LogGuard)
    builder.rs      — LogBuilder implementation
    db_layer.rs     — tracing Layer that writes to TimescaleDB
    console_layer.rs — thin wrapper around fmt::Layer
    schema.rs       — migration + query functions
    batch.rs        — background batch insert logic
```

---

## Dashboard API

### Endpoints

**`GET /api/logs`** — query logs with filters

Parameters (all optional, combinable):
- `service` — filter by service name
- `level` — minimum level (e.g., "error" returns ERROR only, "warn" returns ERROR + WARN)
- `config_id` — benchmark correlation
- `project_id` — workspace scoping
- `search` — substring match on message
- `from` / `to` — ISO 8601 time range (default: last 1 hour)
- `limit` — max results (default 200, max 1000)
- `offset` — pagination

Response: `{ entries: [{ ts, service, level, message, config_id, project_id, trace_id, fields }], total: N }`

Auth: regular users see only their project_id. Platform admins see all.

**`GET /api/logs/stats`** — summary counts

Parameters: `from`, `to` (default: last 1 hour)

Response: `{ by_service: { "orchestrator": { error: 3, warn: 12, info: 450 } }, total: N }`

### Removed

- `LogBuffer` struct and `LogBufferLayer` — replaced by DB queries
- `GET /api/admin/logs` — replaced by `GET /api/logs`
- WebSocket log broadcasts for benchmark progress — replaced by DB polling

### Kept

- `perf_log` system — separate concern (frontend performance metrics, not service tracing)

---

## Dashboard UI

### System > Logs Tab

Replaces the current in-memory log viewer:

- **Filter bar:** dropdowns for service and level, text input for search, date range picker, optional config_id field
- **Summary bar:** per-service error/warn counts for the time window (from `/api/logs/stats`)
- **Log table:** timestamp, service, level, message. Click to expand and see `fields` JSON.
- **Live tail toggle:** polls every 2 seconds for new entries
- **URL state:** all filters reflected in query params (shareable/bookmarkable)

### Benchmark Progress Page

The "live log" section changes from WebSocket-only to:
- Query `GET /api/logs?config_id=X&service=orchestrator` on load (shows historical logs)
- Poll every 2 seconds while benchmark is running
- Logs persist after benchmark finishes — no more "No log output was captured"

---

## Integration Per Crate

| Crate | Console | DB | Context Fields |
|-------|---------|-----|----------------|
| `networker-dashboard` | stderr | always | project_id (per-request span) |
| `benchmarks/orchestrator` | stderr | always (URL from config) | config_id, project_id |
| `networker-agent` | stderr | always (URL from dashboard config) | agent_id |
| `networker-endpoint` | stderr | optional (`--log-db-url`) | — |
| `networker-tester` | stderr | optional (`--log-db-url`) | — |

The orchestrator gets the `logs_db_url` from the benchmark config JSON (the worker already writes `callback_url` there; it will also write `logs_db_url`).

---

## Migration & Rollout

### Phase 1 — Infrastructure
- Docker: switch to `timescale/timescaledb:latest-pg16` image
- Production: `CREATE EXTENSION IF NOT EXISTS timescaledb` on `networker_logs` DB
- Create `service_log` hypertable

### Phase 2 — `networker-log` crate
- New crate, add to workspace
- Implements LogBuilder, console layer, DB layer, batch writer, schema, queries

### Phase 3 — Crate integration (one at a time)
1. `networker-dashboard` — replace LogBuffer + LogBufferLayer
2. `benchmarks/orchestrator` — fix diagnostic visibility
3. `networker-agent` — agent log visibility
4. `networker-endpoint` — lower priority
5. `networker-tester` — CLI, optional DB

### Phase 4 — Dashboard UI
- Replace System > Logs with TimescaleDB query
- Update benchmark progress page
- Remove LogBuffer, LogBufferLayer, old API

### Phase 5 — Production deploy
- Install TimescaleDB extension
- Deploy new binaries
- Verify retention policy active

Each phase is independently deployable. If `with_db()` is not called, logging works exactly like today (console only).

---

## Pipeline Validation

Every deploy must prove the system works before it's considered complete. No more "site is down after deploy."

### Health Check Endpoint

`GET /api/health` — returns 200 with:

```json
{
  "status": "ok",
  "version": "0.23.0",
  "db": "ok",           // main DB reachable
  "logs_db": "ok",      // TimescaleDB reachable (or "fallback" if using main DB)
  "uptime_secs": 12
}
```

Returns 503 if any critical dependency is down. This is the gate the pipeline waits on.

### Smoke Test Endpoint

`POST /api/admin/smoke-test` (platform admin only) — writes a test log entry and reads it back:

1. Inserts `{ service: "smoke-test", message: "__deploy_smoke_NNN__" }` into `service_log`
2. Queries it back within 3 seconds
3. Deletes the test entry
4. Returns `{ "ok": true, "roundtrip_ms": 42 }` or `{ "ok": false, "error": "..." }`

### Release Workflow Changes

The deploy step in `release.yml` becomes:

```yaml
# 1. Backup current binary
cp /opt/alethedash/networker-dashboard /opt/alethedash/networker-dashboard.bak

# 2. Deploy new binary + frontend
cp /tmp/networker-dashboard /opt/alethedash/networker-dashboard
# ... frontend, endpoint, agent ...

# 3. Restart services
systemctl restart networker-dashboard

# 4. Wait for health (max 30s)
for i in $(seq 1 30); do
  if curl -sf http://localhost:3000/api/health | grep -q '"status":"ok"'; then
    echo "Health check passed"
    break
  fi
  sleep 1
done

# 5. Run smoke test
SMOKE=$(curl -sf -X POST http://localhost:3000/api/admin/smoke-test \
  -H "Authorization: Bearer $DEPLOY_TOKEN")
if echo "$SMOKE" | grep -q '"ok":true'; then
  echo "Smoke test passed"
else
  echo "::error::Smoke test failed — rolling back"
  cp /opt/alethedash/networker-dashboard.bak /opt/alethedash/networker-dashboard
  systemctl restart networker-dashboard
  exit 1
fi

# 6. Cleanup
rm /opt/alethedash/networker-dashboard.bak
```

### What This Catches

- Binary panics at startup (axum route syntax, missing DB, bad config) → health check fails → rollback
- DB migration breaks (missing table, bad SQL) → health check fails → rollback  
- Logging pipeline broken (TimescaleDB not installed, extension missing) → smoke test fails → rollback
- Service starts but can't serve requests → health check 503 → rollback

---

## Failure Modes & Observability

### Logging Pipeline Metrics

The `DbLayer` exposes internal counters via a `LogPipelineMetrics` struct shared with the dashboard:

```rust
pub struct LogPipelineMetrics {
    pub entries_written: AtomicU64,     // successfully inserted
    pub entries_dropped: AtomicU64,     // dropped due to full channel or DB failure
    pub flush_count: AtomicU64,         // number of batch flushes
    pub flush_errors: AtomicU64,        // failed flushes
    pub last_flush_ms: AtomicU64,       // latency of most recent flush
    pub queue_depth: AtomicU32,         // current channel backlog
}
```

These are exposed via `GET /api/logs/pipeline-status`:

```json
{
  "entries_written": 145230,
  "entries_dropped": 0,
  "flush_count": 2904,
  "flush_errors": 0,
  "last_flush_ms": 12,
  "queue_depth": 3,
  "status": "healthy"
}
```

`status` is `"healthy"`, `"degraded"` (drops > 0 in last 5min), or `"failing"` (last flush errored).

When entries are dropped, a WARN is emitted to the console layer (which is always active): `"Log pipeline dropped N entries (DB unreachable)"`. This ensures drops are visible even when the DB layer is down.

### Failure Scenarios

| Scenario | Behavior | Detection |
|----------|----------|-----------|
| DB unavailable at startup | `with_db()` returns warning, console-only mode | `pipeline-status` shows no DB layer |
| DB goes down mid-run | Channel fills, entries dropped, console continues | `entries_dropped` counter increases |
| DB latency spike | Batch flush slows, queue grows | `queue_depth` > 1000 or `last_flush_ms` > 5000 |
| Channel overflow (1000 capacity) | Newest entries dropped (back-pressure) | `entries_dropped` counter |
| Partial batch failure | Failed batch retried once, then dropped | `flush_errors` counter |
| Schema missing (no migration) | First INSERT fails, layer disables itself | `status: "failing"`, console WARN |

### Health vs Degraded Mode

The logs DB is a **soft dependency**:
- `GET /api/health` returns `"logs_db": "ok"`, `"degraded"`, or `"unavailable"`
- Health status is `200 OK` even when logs DB is down — the application continues to function
- Health status is `503` only when the **main DB** or the **application itself** is broken
- The smoke test (`/api/admin/smoke-test`) returns `{ "ok": false }` if logs DB is down — the pipeline blocks deployment of logging features, but does NOT roll back a working application

---

## Trace Propagation

### trace_id Generation

- Generated as `Uuid::new_v4()` at the **entry point** of a logical operation:
  - Dashboard: per-HTTP-request (middleware generates it, sets it as a tracing span field)
  - Orchestrator: per-benchmark-config (set once at startup via `with_context()`)
  - Agent: per-job-execution

### Cross-Service Propagation

- Dashboard → Orchestrator: `trace_id` is written to the benchmark config JSON (alongside `callback_url`, `logs_db_url`). The orchestrator reads it and sets it via `with_context("trace_id", &id)`.
- Dashboard → Agent: `trace_id` is sent in WebSocket job messages. The agent sets it as a span field for the job's duration.
- No HTTP header propagation (services don't call each other via HTTP except for callbacks, which already carry `config_id` for correlation).

### Required vs Optional

| Field | Required | Set By |
|-------|----------|--------|
| `ts` | yes | DbLayer (auto) |
| `service` | yes | LogBuilder::new("name") |
| `level` | yes | tracing macro |
| `message` | yes | tracing macro |
| `config_id` | no | with_context() — orchestrator, agent |
| `project_id` | no | with_context() or per-request span |
| `trace_id` | no | middleware (dashboard) or with_context() |
| `fields` | no | all structured tracing fields |

---

## Multi-Tenancy Enforcement

Project-scoped log access is enforced **server-side** in the API handler, not client-driven:

```rust
async fn query_logs(
    State(state): State<Arc<AppState>>,
    req: Request,
    Query(params): Query<LogQueryParams>,
) -> Result<Json<LogResponse>, StatusCode> {
    let claims = extract_auth(&req)?;

    // Platform admins: unrestricted
    // Regular users: forced to their active project_id
    let project_filter = if claims.is_platform_admin {
        params.project_id  // optional, admin can query any or all
    } else {
        let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
        Some(ctx.project_id.clone())  // always forced, ignores query param
    };

    // ... query with project_filter applied to WHERE clause
}
```

Users cannot override or omit the `project_id` filter. The middleware extracts it from the JWT + active workspace context, same as all other project-scoped endpoints.

---

## Search Strategy

### Current Scope

`search` uses `message ILIKE '%term%'` but is **always bounded** by:
1. Time range (default 1 hour, max 24 hours for non-admins)
2. At least one of: `service`, `level`, or `config_id` (the API rejects unbounded searches)

Within a time-bounded, service-filtered window, `ILIKE` on TEXT is fast enough — TimescaleDB only scans the relevant chunks.

### Future (if needed at scale)

- Add `pg_trgm` extension + trigram GIN index on `message` for fast substring search
- Or switch to `tsvector` full-text search for keyword queries
- Decision deferred until query latency exceeds 500ms on production workloads

---

## Pagination & Ordering

- **Ordering:** `ts DESC` (newest first) by default. Stable because `ts` has microsecond precision from `clock_timestamp()` (not `now()` which is transaction-level).
- **Live tail:** Client sends `GET /api/logs?from={last_seen_ts}&limit=100` every 2 seconds. Server returns entries with `ts > from`. No cursor needed — timestamp is monotonic within a service.
- **Deep pagination:** `offset` for page N. For very large result sets (>10K), recommend narrowing filters instead of deep pagination (offset-based is O(N) in Postgres). Documented in API response: `{ "entries": [...], "total": N, "truncated": true }` when total > 10K.

---

## Docker & Version Pinning

```yaml
# docker-compose.dashboard.yml
services:
  postgres:
    image: timescale/timescaledb-ha:pg16.6-ts2.17.2  # pinned, not :latest
    environment:
      POSTGRES_DB: networker_core
      POSTGRES_USER: networker
      POSTGRES_PASSWORD: networker
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./scripts/init-logs-db.sql:/docker-entrypoint-initdb.d/01-logs-db.sql
```

Production: pin to same TimescaleDB version. Upgrades are explicit PRs with changelog entry.

---

## Non-Goals

- **Log aggregation from multiple servers** — single-server deployment for now
- **Alerting on log patterns** — out of scope, can be added later
- **Replacing perf_log** — separate system for frontend performance metrics
- **Real-time streaming via WebSocket** — polling every 2s is simpler and sufficient
- **Log export (S3, Elasticsearch)** — can be added as a TimescaleDB continuous aggregate or extension later
