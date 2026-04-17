# Unified TestConfig — authoritative spec

**Branch:** `backend-unification`
**Target version:** `v0.28.0`
**Status:** source of truth for parallel implementation agents

This document is frozen — any parallel agent must read this first and not invent shapes.

---

## 1. Schema (PostgreSQL)

All tables below replace the current `job`, `benchmark_config`, `benchmark_config_preset`, `schedule` (polymorphic), and related. Drop cleanly; dev-only, no backfill.

```sql
-- Canonical unit of work: definition of a test (simple OR benchmark-grade).
CREATE TABLE test_config (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id       TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    description      TEXT,

    -- Endpoint (one of three kinds — polymorphic via endpoint_kind + endpoint_ref)
    endpoint_kind    TEXT NOT NULL CHECK (endpoint_kind IN ('network','proxy','runtime')),
    endpoint_ref     JSONB NOT NULL,      -- shape depends on endpoint_kind (see §2.2)

    -- Workload (required, same for all kinds)
    workload         JSONB NOT NULL,      -- see §2.3

    -- Methodology (optional — NULL = simple test, NOT NULL = benchmark mode)
    methodology      JSONB,               -- see §2.4

    -- Lineage / audit
    created_by       UUID REFERENCES app_user(id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Baseline (optional — for comparison / regression detection)
    baseline_run_id  UUID REFERENCES test_run(id) ON DELETE SET NULL,

    -- Ops
    max_duration_secs INT NOT NULL DEFAULT 900,

    UNIQUE (project_id, name)
);
CREATE INDEX ix_test_config_project ON test_config(project_id);
CREATE INDEX ix_test_config_endpoint_kind ON test_config(endpoint_kind);
CREATE INDEX ix_test_config_is_benchmark ON test_config((methodology IS NOT NULL));

-- Every execution of a TestConfig produces one test_run.
CREATE TABLE test_run (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_config_id   UUID NOT NULL REFERENCES test_config(id) ON DELETE CASCADE,
    project_id       TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,

    -- Status flow: queued → running → completed | failed | cancelled
    status           TEXT NOT NULL CHECK (status IN ('queued','running','completed','failed','cancelled')),
    started_at       TIMESTAMPTZ,
    finished_at      TIMESTAMPTZ,

    -- Flat result summary — ALWAYS populated, regardless of methodology
    success_count    INT NOT NULL DEFAULT 0,
    failure_count    INT NOT NULL DEFAULT 0,
    error_message    TEXT,

    -- Rich artifact — populated only if test_config.methodology was NOT NULL
    artifact_id      UUID REFERENCES benchmark_artifact(id) ON DELETE SET NULL,

    -- Dispatch / execution metadata
    tester_id        UUID REFERENCES tester(id) ON DELETE SET NULL,
    worker_id        TEXT,
    last_heartbeat   TIMESTAMPTZ,

    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ix_test_run_config ON test_run(test_config_id);
CREATE INDEX ix_test_run_project_status ON test_run(project_id, status);
CREATE INDEX ix_test_run_created ON test_run(created_at DESC);

-- Methodology-mode artifact (kept from existing schema, renamed for clarity).
-- Only created when test_config.methodology IS NOT NULL.
CREATE TABLE benchmark_artifact (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_run_id      UUID NOT NULL REFERENCES test_run(id) ON DELETE CASCADE,

    -- Phase-level + statistical data
    environment      JSONB NOT NULL,       -- hardware/os/version capture
    methodology      JSONB NOT NULL,       -- copied from test_config.methodology for immutability
    launches         JSONB NOT NULL,       -- per-tester launch details
    cases            JSONB NOT NULL,       -- per-case results with p5/p25/p50/p75/p95/p99/p999
    samples          JSONB,                -- raw samples (optional — heavy)
    summaries        JSONB NOT NULL,       -- aggregated quality gates, CV, outliers
    data_quality     JSONB NOT NULL,       -- noise_level, sufficiency, publication_ready, blockers

    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ix_benchmark_artifact_run ON benchmark_artifact(test_run_id);

-- Unified schedule — references test_config_id only. No more polymorphism.
CREATE TABLE test_schedule (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_config_id   UUID NOT NULL REFERENCES test_config(id) ON DELETE CASCADE,
    project_id       TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,

    cron_expr        TEXT NOT NULL,
    timezone         TEXT NOT NULL DEFAULT 'UTC',
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,

    last_fired_at    TIMESTAMPTZ,
    last_run_id      UUID REFERENCES test_run(id) ON DELETE SET NULL,
    next_fire_at     TIMESTAMPTZ,

    created_by       UUID REFERENCES app_user(id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ix_test_schedule_enabled_next ON test_schedule(enabled, next_fire_at);
CREATE INDEX ix_test_schedule_config ON test_schedule(test_config_id);

-- Keep existing: request_attempt + per-protocol phase detail tables
-- (DnsResult, TcpResult, TlsResult, HttpResult, UdpResult) — they FK test_run_id
-- now instead of the old run_id. Simple column rename only.

-- Keep existing: tester, project, app_user, benchmark_vm_catalog, cloud_account,
-- deployment, share_link, share_token, regression_baseline, etc. — untouched.
```

### Tables DROPPED in this refactor

- `job`
- `job_config` (if separate)
- `benchmark_config`
- `benchmark_config_preset`
- `schedule` (old polymorphic one — replaced by `test_schedule`)
- Any view/materialized-view that joined across the above

---

## 2. Rust types (canonical — `crates/networker-common/src/test_config.rs`)

All types `#[derive(Debug, Clone, Serialize, Deserialize)]`. `serde(tag = "kind")` for tagged unions. Use `uuid::Uuid`, `chrono::DateTime<Utc>`.

### 2.1 Top-level

```rust
pub struct TestConfig {
    pub id: Uuid,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub endpoint: EndpointRef,
    pub workload: Workload,
    pub methodology: Option<Methodology>,
    pub baseline_run_id: Option<Uuid>,
    pub max_duration_secs: u32,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### 2.2 EndpointRef (tagged union — matches `endpoint_kind` + `endpoint_ref` JSONB)

```rust
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EndpointRef {
    Network { host: String, port: Option<u16> },       // direct protocol probe
    Proxy   { proxy_endpoint_id: Uuid },               // endpoint fronted by nginx/IIS
    Runtime { runtime_id: Uuid, language: String },    // language/framework stack
}
```

### 2.3 Workload (required)

```rust
pub struct Workload {
    pub modes: Vec<Mode>,                    // http1, http2, http3, tcp, tls, dns, udp, pageload, pageload2, pageload3, browser1, browser2, browser3, download, upload, curl, native, tlsresume
    pub runs: u32,                           // iterations per mode per case
    pub concurrency: u32,                    // parallel connections per run
    pub timeout_ms: u32,
    pub payload_sizes: Vec<u32>,             // bytes; for download/upload only
    pub capture_mode: CaptureMode,           // headers-only, full, metrics-only
}

#[serde(rename_all = "kebab-case")]
pub enum CaptureMode { HeadersOnly, Full, MetricsOnly }
```

`Mode` enum: union of current `networker_tester::Protocol` variants. Serialize as lowercase string.

### 2.4 Methodology (optional — benchmark mode)

```rust
pub struct Methodology {
    pub warmup_runs: u32,
    pub measured_runs: u32,
    pub cooldown_ms: u32,
    pub target_error_pct: f32,               // target CV% (e.g. 2.0 = 2%)
    pub outlier_policy: OutlierPolicy,
    pub quality_gates: QualityGates,
    pub publication_gates: PublicationGates, // block publish if any gate fails
}

#[serde(tag = "policy", rename_all = "kebab-case")]
pub enum OutlierPolicy {
    None,
    Iqr        { k: f32 },                   // default k=1.5
    StdDev     { sigma: f32 },               // default 3.0
    Percentile { lo: f32, hi: f32 },         // e.g. 0.5, 99.5
}

pub struct QualityGates {
    pub max_cv_pct: f32,                     // reject if CV > this
    pub min_samples: u32,                    // reject if n < this
    pub max_noise_level: f32,
}

pub struct PublicationGates {
    pub max_failure_pct: f32,                // overall failure rate ceiling
    pub require_all_phases: bool,            // every phase must have >=min_samples
}
```

### 2.5 Run

```rust
pub struct TestRun {
    pub id: Uuid,
    pub test_config_id: Uuid,
    pub project_id: String,
    pub status: RunStatus,                   // Queued, Running, Completed, Failed, Cancelled
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub success_count: u32,
    pub failure_count: u32,
    pub error_message: Option<String>,
    pub artifact_id: Option<Uuid>,           // Some iff methodology was set
    pub tester_id: Option<Uuid>,
    pub worker_id: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[serde(rename_all = "lowercase")]
pub enum RunStatus { Queued, Running, Completed, Failed, Cancelled }
```

### 2.6 Schedule

```rust
pub struct TestSchedule {
    pub id: Uuid,
    pub test_config_id: Uuid,
    pub project_id: String,
    pub cron_expr: String,
    pub timezone: String,
    pub enabled: bool,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub last_run_id: Option<Uuid>,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}
```

---

## 3. REST API

Base: `/api/v2`. All old `/api/...` endpoints are DELETED in the same PR (no users to protect).

### 3.1 TestConfig CRUD

| Verb | Path | Body | Response |
|---|---|---|---|
| POST | `/api/v2/projects/:pid/test-configs` | `TestConfigCreate` (no id/timestamps) | `TestConfig` |
| GET  | `/api/v2/projects/:pid/test-configs` | — | `[TestConfigListItem]` (summary, no heavy fields) |
| GET  | `/api/v2/test-configs/:id` | — | `TestConfig` |
| PATCH| `/api/v2/test-configs/:id` | partial | `TestConfig` |
| DELETE| `/api/v2/test-configs/:id` | — | 204 |
| POST | `/api/v2/test-configs/:id/launch` | `{ tester_id?: Uuid }` | `TestRun` (status = queued) |

### 3.2 TestRun

| Verb | Path | Notes |
|---|---|---|
| GET  | `/api/v2/projects/:pid/test-runs?status=&endpoint_kind=&has_artifact=&limit=&before=` | paginated list with filters |
| GET  | `/api/v2/test-runs/:id` | full detail + artifact if present |
| GET  | `/api/v2/test-runs/:id/artifact` | benchmark artifact JSON |
| GET  | `/api/v2/test-runs/:id/attempts` | per-mode request attempts |
| POST | `/api/v2/test-runs/:id/cancel` | if queued/running |
| POST | `/api/v2/test-runs/compare` | body: `{ run_ids: [Uuid,...] }` → `ComparisonReport` |

### 3.3 Schedule

| Verb | Path |
|---|---|
| POST | `/api/v2/projects/:pid/schedules` (body references `test_config_id`) |
| GET  | `/api/v2/projects/:pid/schedules` |
| PATCH| `/api/v2/schedules/:id` |
| DELETE| `/api/v2/schedules/:id` |
| POST | `/api/v2/schedules/:id/trigger` → launches a run now |

---

## 4. WebSocket protocol

Bump `protocol_version` to `2`. Old agents rejected with an upgrade-required error (dev mode, we control all agents).

```rust
// Dashboard → Agent
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    AssignRun { run: TestRun, config: TestConfig },
    CancelRun { run_id: Uuid },
    Heartbeat { now: DateTime<Utc> },
    Shutdown,
}

// Agent → Dashboard
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    RunStarted   { run_id: Uuid, started_at: DateTime<Utc> },
    RunProgress  { run_id: Uuid, success: u32, failure: u32 },
    RunFinished  { run_id: Uuid, status: RunStatus, artifact: Option<BenchmarkArtifact> },
    AttemptEvent { run_id: Uuid, attempt: RequestAttempt },
    Heartbeat    { now: DateTime<Utc> },
    Error        { run_id: Option<Uuid>, message: String },
}
```

---

## 5. CLI (`networker-tester` binary)

The CLI remains first-class. Accepts `TestConfig` YAML or JSON via `--config path.yaml` OR builds one inline from flags.

### 5.1 Config file shape (new — YAML preferred)

```yaml
name: "cloudflare connectivity check"
endpoint:
  kind: network
  host: www.cloudflare.com
workload:
  modes: [dns, tcp, tls, http2]
  runs: 10
  concurrency: 1
  timeout_ms: 5000
  payload_sizes: []
  capture_mode: headers-only
# Omit `methodology` for a simple test; add it for benchmark mode:
# methodology:
#   warmup_runs: 5
#   measured_runs: 30
#   ...
```

### 5.2 Flag mapping (back-compat at the flag level, not config-file level)

| Old flag | New behavior |
|---|---|
| `--target` | builds `endpoint.kind = network`, `endpoint.host` |
| `--mode a,b,c` | builds `workload.modes` |
| `--runs N` | builds `workload.runs` |
| `--concurrency N` | builds `workload.concurrency` |
| `--timeout-ms N` | builds `workload.timeout_ms` |
| `--config PATH` | parse full `TestConfig` from file |
| `--benchmark` | new — toggles default `methodology` block |
| `--dashboard-url URL` + `--api-key K` | unchanged |

### 5.3 Rejected input

Old v1 config files (with top-level `target_host`, `modes` without `endpoint`/`workload` wrapping) produce a clear error with a migration hint URL. No silent compat.

---

## 6. Dashboard — page-to-page mapping

| Old page | New page | Notes |
|---|---|---|
| `JobsPage` (Tests) | merged into `RunsPage` | Tests nav item deleted |
| `RunsPage` | **canonical list** — add endpoint-kind filter, has-artifact filter | gains "New Run" CTA |
| `SchedulesPage` | unchanged structure, refactored to use `test_config_id` | |
| `BenchmarksPage` (Full Stack) | merged into `RunsPage` | gone as separate nav |
| `AppBenchmarkWizardPage` | merged into `NewRunPage` (endpoint-kind = runtime) | |
| `BenchmarkWizardPage` | merged into `NewRunPage` (endpoint-kind = proxy) | |
| `BenchmarkProgressPage` | merged into `RunDetailPage` (methodology section auto-rendered if artifact exists) | |
| `BenchmarkDetailPage` | merged into `RunDetailPage` | |
| `BenchmarkComparePage` | moved to `/runs/compare?ids=...` | any 2+ runs with artifacts |
| `BenchmarkConfigResultsPage` | moved to `/test-configs/:id/runs` | reads unified test_run |
| `BenchmarkRegressionsPage` | kept as `/runs/regressions` | reads artifact delta |
| `BenchmarkCatalogPage` | renamed `/runtimes` (the Runtime registry — same concept) | |
| other (Sidebar, Users, Settings, etc.) | untouched | |

**New page:** `NewRunPage` (`/projects/:pid/runs/new`):
- Step 1: pick endpoint kind (Network / Proxy / Runtime) via segmented control
- Step 2: fill workload (modes, runs, concurrency)
- Step 3: optional toggle "benchmark mode" → methodology fields
- Step 4: save as TestConfig + optional schedule → launch now or later

Replaces: `CreateJobDialog`, `BenchmarkWizardPage`, `AppBenchmarkWizardPage`.

---

## 7. Parallel implementation plan

Four agents can run in parallel once this spec is locked:

| Agent | Scope | Files |
|---|---|---|
| **A · Backend** | Schema + Rust types + DB layer | `crates/networker-dashboard/src/db/**`, `crates/networker-common/src/**`, migrations.rs |
| **B · API + WS** | REST v2 + WebSocket protocol v2 + services | `crates/networker-dashboard/src/api/**`, `src/ws/**`, `src/services/**` |
| **C · CLI + Agent** | networker-tester binary + networker-agent | `crates/networker-tester/**`, `crates/networker-agent/**` |
| **D · Dashboard** | React rewrite of affected pages | `dashboard/src/pages/**`, `dashboard/src/api/client.ts`, nav updates |

Dependencies: B depends on A (types). C depends on A (types) + B (WS). D depends on B (REST). Start A first, then A+B+C, then D.

---

## 8. Acceptance checklist (before merge)

- [ ] `cargo build --workspace --all-features` green
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace --lib` all pass
- [ ] Integration tests green with real Postgres (docker-compose.db.yml)
- [ ] `cargo build -p networker-tester --no-default-features` green (no http3 stub regression)
- [ ] `cd dashboard && npm run lint && npm run build` green
- [ ] Manual smoke: create simple test via dashboard, run it, see result
- [ ] Manual smoke: create benchmark test via dashboard, run it, see artifact + phase detail
- [ ] Manual smoke: schedule a test, verify it fires
- [ ] Manual smoke: CLI `networker-tester --target example.com --mode http2` works
- [ ] Manual smoke: CLI `--config simple.yaml` and `--config benchmark.yaml` both work
- [ ] Version bumped to `v0.28.0` in Cargo.toml + CHANGELOG.md + INSTALLER_VERSION in both installers
- [ ] Cargo.lock committed
- [ ] Gist updated post-merge

---

## 9. Ground rules for parallel agents

1. **Do not invent types.** If a shape isn't in §2, stop and ask.
2. **No backward compat.** Old table drops happen in the same migration.
3. **One migration file.** All DDL in a single idempotent migration at the top of `migrations.rs`.
4. **Preserve existing foreign tables** (project, app_user, tester, deployment, etc.). Only drop the tables listed in §1.
5. **Strong typing over `serde_json::Value`** wherever possible in the Rust types, but store polymorphic fields (endpoint_ref, workload, methodology) as JSONB for schema flexibility.
6. **Commit granularity:** one commit per logical unit (schema, types module, REST module, page rewrite). Prefix: `refactor(v0.28):`.
7. **Tests first where possible** — when adding a new endpoint or DB function, write the test first if one doesn't exist.
