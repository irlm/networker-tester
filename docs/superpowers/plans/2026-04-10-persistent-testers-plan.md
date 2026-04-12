# Persistent Testers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace per-benchmark ephemeral VM provisioning with long-lived, user-managed tester VMs. Application benchmarks acquire an exclusive lock on a chosen tester, run, and release — never destroy. Auto-shutdown nightly when fully drained. Region-grouped management UI. Push-based queue and phase updates.

**Architecture:** New `project_tester` domain object with two orthogonal state axes (`power_state` for VM lifecycle, `allocation` for benchmark reservation). Lock acquire/release primitives in `services/tester_state.rs`. Background tokio tasks for auto-shutdown, queue dispatch, crash recovery, version refresh. WebSocket subscription channel for live updates. Single shared `<PhaseBar />` React component. Single PR — backend + frontend + migration + smoke tests.

**Tech Stack:** Rust workspace (axum 0.8, tokio-postgres, chrono-tz), TypeScript + React + Vite (dashboard frontend), bash install.sh, shared `networker-common` crate for cross-binary types.

**Source spec:** `docs/superpowers/specs/2026-04-10-persistent-testers-design.md`

---

## Build Order

The plan is organized into 8 phases. Each phase produces compilable, testable code; phases must be completed in order because later phases depend on earlier ones.

1. **Phase A — Foundation** (Tasks 1–4): Baseline smoke test, Phase enum in shared crate, schema migration V027.
2. **Phase B — State primitives** (Tasks 5–8): DB row types, `tester_state` lock service with race-safe primitives, region timezone map.
3. **Phase C — Background services** (Tasks 9–13): Tester install (extracted from existing code), dispatcher, scheduler, recovery, version refresh.
4. **Phase D — REST API** (Tasks 14–18): All `/api/projects/{pid}/testers/...` endpoints.
5. **Phase E — WebSocket push** (Tasks 19–21): Subscription channel, hub, project enforcement.
6. **Phase F — Orchestrator rewrite** (Tasks 22–25): Replace ephemeral-VM path with tester lock acquisition.
7. **Phase G — Frontend** (Tasks 26–32): PhaseBar, hooks, modals, drawer, TestersPage, wizard step.
8. **Phase H — Bootstrap, smoke, ship** (Tasks 33–36): Reset binary, full smoke run, CHANGELOG, version bump.

---

## File Structure

### New files

| Path | Responsibility |
|---|---|
| `tests/cli_smoke.sh` | 8-scenario CLI smoke test script (hard pre-merge gate) |
| `crates/networker-common/src/phase.rs` | Shared `Phase` and `Outcome` enums |
| `crates/networker-common/src/tester_messages.rs` | WebSocket message types for queue + phase updates |
| `crates/networker-dashboard/src/db/project_testers.rs` | `ProjectTesterRow` types + DB CRUD |
| `crates/networker-dashboard/src/services/mod.rs` | Module entry point |
| `crates/networker-dashboard/src/services/tester_state.rs` | `try_acquire`, `release`, `try_power_transition` |
| `crates/networker-dashboard/src/services/azure_regions.rs` | Region → IANA timezone map |
| `crates/networker-dashboard/src/services/tester_install.rs` | One-time tester install (Chrome + Node + harness) |
| `crates/networker-dashboard/src/services/tester_dispatcher.rs` | `promote_next` + sweep loop |
| `crates/networker-dashboard/src/services/tester_scheduler.rs` | Auto-shutdown loop with deferral logic |
| `crates/networker-dashboard/src/services/tester_recovery.rs` | Startup crash recovery + opt-in auto-probe |
| `crates/networker-dashboard/src/services/version_refresh.rs` | Latest installer version cache + GitHub poll |
| `crates/networker-dashboard/src/services/tester_queue_hub.rs` | In-process pub/sub for queue updates |
| `crates/networker-dashboard/src/api/testers.rs` | All tester REST handlers |
| `crates/networker-dashboard/src/api/tester_ws.rs` | WebSocket subscription handler |
| `crates/networker-dashboard/src/bin/reset_pre_prod.rs` | Destructive bootstrap binary (env-gated) |
| `crates/networker-dashboard/bootstrap/reset-pre-prod.sql` | Destructive reset SQL script |
| `dashboard/src/components/PhaseBar.tsx` | Shared phase progress bar |
| `dashboard/src/components/CreateTesterModal.tsx` | Tester creation modal (shared wizard + management) |
| `dashboard/src/components/TesterDetailDrawer.tsx` | Tester detail drawer with all sections |
| `dashboard/src/components/TesterRegionGroup.tsx` | Region accordion group |
| `dashboard/src/components/wizard/TesterStep.tsx` | Wizard tester picker step |
| `dashboard/src/hooks/useTesterSubscription.ts` | WebSocket queue subscription hook |
| `dashboard/src/hooks/usePhaseSubscription.ts` | WebSocket phase update subscription hook |
| `dashboard/src/pages/TestersPage.tsx` | Tester management page (region-grouped accordion) |
| `dashboard/src/api/testers.ts` | Frontend API client for tester endpoints |

### Modified files

| Path | Change |
|---|---|
| `crates/networker-dashboard/src/db/migrations.rs` | Add V027 constant + runner block |
| `crates/networker-dashboard/src/db/mod.rs` | Re-export `project_testers` |
| `crates/networker-dashboard/src/lib.rs` | Add `services` module |
| `crates/networker-dashboard/src/api/mod.rs` | Wire `testers` and `tester_ws` routes |
| `crates/networker-dashboard/src/main.rs` | Spawn 4 background tasks |
| `crates/networker-dashboard/Cargo.toml` | Add `chrono-tz` dependency |
| `crates/networker-common/src/lib.rs` | Re-export `phase` and `tester_messages` modules |
| `benchmarks/orchestrator/src/executor.rs` | Replace `execute_testbed_application` body with tester-lock flow; delete ephemeral-VM provisioning code |
| `dashboard/src/pages/BenchmarkProgressPage.tsx` | Replace inline progress bar with `<PhaseBar />` |
| `dashboard/src/pages/AppBenchmarkWizardPage.tsx` | Add Tester step, remove "create new VM" path, require `tester_id` in payload |
| `dashboard/src/pages/BenchmarksPage.tsx` | Banner advertising persistent testers |
| `dashboard/src/components/ProjectNav.tsx` (or equivalent) | Add Testers nav entry |
| `Cargo.toml` (workspace) | Version bump 0.24.2 → 0.25.0 |
| `install.sh` | `INSTALLER_VERSION` bump |
| `install.ps1` | `$InstallerVersion` bump |
| `CHANGELOG.md` | New `## [0.25.0]` section |

---

## Phase A — Foundation

### Task 1: CLI smoke test script (baseline gate)

**Goal:** Create `tests/cli_smoke.sh` that exercises the existing CLI in 8 scenarios. Run it FIRST so we have a baseline before any code changes.

**Files:** Create `tests/cli_smoke.sh`

The script structure (full bash content provided in the spec annex; subagent produces it from this skeleton):

- Scenario 1: Local HTTP/2 probe against `networker-endpoint` started in background
- Scenario 2: HTTP/3 probe with QUIC
- Scenario 3: DNS probe against a public domain
- Scenario 4: All-modes probe against `https://www.cloudflare.com`
- Scenario 5: SQLite persistence with `--features sqlite --output-db /tmp/cli_smoke.db`
- Scenario 6: Workspace builds — `cargo build --workspace`, `--no-default-features`, `--all-features`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace --lib`
- Scenario 7: CLI reports phases to dashboard (gated by `SMOKE_TEST_DASHBOARD=1`; deferred to Task 35 for full implementation)
- Scenario 8: E2E persistent-tester flow (gated by `SMOKE_TEST_AZURE=1`; deferred to Task 35)

The script tracks pass/fail counts in a results array, prints a summary at the end, and exits with the count of failures. Cleanup trap kills any background `networker-endpoint` process and removes the temp SQLite file.

- [ ] **Step 1: Write the script** with the 8 scenarios above

- [ ] **Step 2: Make executable**

Run: `chmod +x tests/cli_smoke.sh`

- [ ] **Step 3: Run baseline**

Run: `bash tests/cli_smoke.sh`
Expected: scenario 6 (build) MUST pass. Scenarios 1–5 should pass if the existing CLI is healthy. Scenarios 7–8 print "deferred" and pass.

Document any pre-existing failures in the commit message — they are not regressions introduced by this PR.

- [ ] **Step 4: Commit**

```bash
git add tests/cli_smoke.sh
git commit -m "test: add CLI smoke gate for persistent-testers PR"
```

---

### Task 2: Phase enum in networker-common

**Goal:** Single source of truth for the unified `Phase` and `Outcome` enums shared across orchestrator, dashboard, agent, and CLI.

**Files:**
- Create `crates/networker-common/src/phase.rs`
- Modify `crates/networker-common/src/lib.rs`

- [ ] **Step 1: Write the module**

The file declares two `#[derive(Serialize, Deserialize)]` enums with `#[serde(rename_all = "lowercase")]`:

```rust
pub enum Phase { Queued, Starting, Deploy, Running, Collect, Done }
pub enum Outcome { Success, PartialSuccess, Failure, Cancelled }
```

Both have an `as_str(&self) -> &'static str` method that returns the lowercase form. Note that `PartialSuccess` serializes as `"partial_success"` due to a custom serde rename annotation; the test verifies this.

Inline `#[cfg(test)]` module with 4 tests:
1. `phase_serializes_as_lowercase` — `Phase::Deploy → "\"deploy\""`
2. `phase_round_trips` — `"\"running\"" → Phase::Running`
3. `outcome_serializes_as_lowercase` — `Outcome::PartialSuccess → "\"partial_success\""`
4. `phase_as_str_matches_serde` — for each variant, `as_str()` matches the JSON form (without quotes)

- [ ] **Step 2: Wire into lib.rs**

Add `pub mod phase;` to `crates/networker-common/src/lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p networker-common phase`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/networker-common/src/phase.rs crates/networker-common/src/lib.rs
git commit -m "feat(common): add unified Phase + Outcome enums"
```

---

### Task 3: Add chrono-tz dependency to dashboard

**Files:** Modify `crates/networker-dashboard/Cargo.toml`

- [ ] **Step 1:** Add `chrono-tz = "0.10"` to `[dependencies]`
- [ ] **Step 2:** Run `cargo build -p networker-dashboard` — clean build
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/Cargo.toml Cargo.lock
git commit -m "deps(dashboard): add chrono-tz for region-local timezones"
```

---

### Task 4: Schema migration V027

**Goal:** Add `project_tester` table, new columns on `benchmark_config`, phase columns on `probe_job`/`schedule_run`. Idempotent (`CREATE TABLE IF NOT EXISTS`, `ADD COLUMN IF NOT EXISTS`).

**Files:** Modify `crates/networker-dashboard/src/db/migrations.rs`

The full SQL is in the design spec (§ "The schema migration"). Key elements:

- `CREATE TABLE IF NOT EXISTS project_tester (...)` with all columns from the spec data model: identity, cloud handles, two state axes (`power_state`, `allocation`), `locked_by_config_id`, version tracking, schedule, deferral count, recovery flags, usage stats, audit columns.
- Two CHECK constraints inside the table: `lock_holder_implies_locked` and `lock_requires_running_vm`.
- 5 indexes: project, power, alloc, shutdown, last_used.
- `ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS` for: `tester_id`, all 5 `tester_*_snapshot` columns, `queued_at`, `current_phase`, `outcome`.
- The `benchmark_config.tester_id` FK to `project_tester(tester_id)` with `ON DELETE SET NULL` — added in a separate `DO $$ BEGIN ... EXCEPTION WHEN duplicate_object` block because both tables reference each other.
- The reverse FK `project_tester.locked_by_config_id` to `benchmark_config(config_id)` with `ON DELETE RESTRICT` — also in a `DO $$ BEGIN` block.
- `app_configs_need_tester` CHECK constraint — also in a `DO $$ BEGIN` block.
- `ALTER TABLE probe_job ADD COLUMN IF NOT EXISTS current_phase TEXT, outcome TEXT;` and same for `schedule_run`.

- [ ] **Step 1:** Append `const V027_PERSISTENT_TESTERS: &str = r#"..."#;` constant after the existing V026 constant in `crates/networker-dashboard/src/db/migrations.rs`.

- [ ] **Step 2:** Add a runner block at the end of `pub async fn run` (mirroring the existing V026 block):

```rust
    if client.query_opt("SELECT version FROM _migrations WHERE version = 27", &[]).await?.is_none() {
        client.batch_execute(V027_PERSISTENT_TESTERS).await?;
        client.execute("INSERT INTO _migrations (version) VALUES (27) ON CONFLICT DO NOTHING", &[]).await?;
        tracing::info!("Migration V027 applied: persistent testers");
    }
```

- [ ] **Step 3: Apply locally**

```bash
docker compose -f docker-compose.dashboard.yml up -d postgres
DASHBOARD_ADMIN_PASSWORD=admin cargo run -p networker-dashboard &
sleep 5
kill %1 2>/dev/null
```

- [ ] **Step 4: Verify schema landed**

Run `psql` against the local Postgres and confirm:
- `\d project_tester` shows the table with all columns
- `SELECT version FROM _migrations` includes 27
- The two CHECK constraints exist

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat(dashboard): V027 schema for persistent testers"
```

---

## Phase B — State primitives

### Task 5: ProjectTesterRow types and DB CRUD

**Files:** Create `crates/networker-dashboard/src/db/project_testers.rs`; modify `crates/networker-dashboard/src/db/mod.rs`.

The module defines:
- `pub struct ProjectTesterRow { ... }` with `#[derive(Debug, Clone, Serialize, Deserialize)]` and all columns from the V027 schema (using `Option` for nullable columns, `Uuid` for `tester_id`/`locked_by_config_id`/`created_by`, `DateTime<Utc>` for timestamps, `String` for INET via `ip.to_string()`).
- `from_row(&tokio_postgres::Row) -> Self` constructor.
- `SELECT_COLUMNS` constant listing all columns in a single string for reuse.
- `pub async fn list_for_project(client, project_id) -> Result<Vec<ProjectTesterRow>>`
- `pub async fn get(client, project_id, tester_id) -> Result<Option<ProjectTesterRow>>`
- `pub struct CreateTesterInput { name, cloud, region, vm_size?, auto_shutdown_local_hour?, auto_probe_enabled? }` with `#[derive(Deserialize)]`
- `pub async fn insert(client, project_id, input, created_by) -> Result<ProjectTesterRow>`
- `pub async fn delete(client, project_id, tester_id) -> Result<bool>`

- [ ] **Step 1:** Write the module
- [ ] **Step 2:** Add `pub mod project_testers;` to `crates/networker-dashboard/src/db/mod.rs`
- [ ] **Step 3:** Run `cargo build -p networker-dashboard` — clean
- [ ] **Step 4:** Commit

```bash
git add crates/networker-dashboard/src/db/project_testers.rs crates/networker-dashboard/src/db/mod.rs
git commit -m "feat(dashboard): ProjectTesterRow types + basic CRUD"
```

---

### Task 6: tester_state primitives (lock acquire/release)

**Files:**
- Create `crates/networker-dashboard/src/services/mod.rs`
- Create `crates/networker-dashboard/src/services/tester_state.rs`
- Modify `crates/networker-dashboard/src/lib.rs`

The `services/mod.rs` declares `pub mod tester_state;`. The `lib.rs` adds `pub mod services;`.

`tester_state.rs` defines:

```rust
pub enum AcquireOutcome {
    Acquired,
    NeedsStart,
    Transient(String),       // power_state was provisioning|starting|stopping
    Upgrading,
    AlreadyLockedBy(Uuid),
    Errored,
    NotIdle(String),
}

pub async fn try_acquire(client, tester_id, config_id) -> Result<AcquireOutcome>
pub async fn release(client, tester_id, config_id) -> Result<()>
pub async fn try_power_transition(client, tester_id, expected, next) -> Result<bool>
pub async fn set_status_message(client, tester_id, msg) -> Result<()>
pub async fn force_release(client, tester_id) -> Result<()>
```

The `try_acquire` function uses a single conditional UPDATE with `WHERE power_state = 'running' AND allocation = 'idle' AND locked_by_config_id IS NULL`, sets `allocation='locked'`, `locked_by_config_id=$2`, `last_used_at=NOW()`. If 0 rows updated, runs a follow-up SELECT to classify why and returns the appropriate `AcquireOutcome` variant.

The `release` function is the **single authoritative writer** of `(allocation='idle', locked_by_config_id=NULL)`. It is defensive: only updates if `locked_by_config_id` matches the expected config_id.

The `force_release` function is the **only other place** allowed to clear the lock pair, used exclusively by the recovery loop on dashboard restart.

- [ ] **Step 1:** Create both files
- [ ] **Step 2:** `cargo build -p networker-dashboard` — clean
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/ crates/networker-dashboard/src/lib.rs
git commit -m "feat(dashboard): tester_state lock primitives"
```

---

### Task 7: Lock invariant unit tests + grep guard

**Files:** Append to `crates/networker-dashboard/src/services/tester_state.rs`

Two tests:

1. **`release_is_only_writer_of_idle_unlock`** (always runs): walks `crates/networker-dashboard/src/services/` directory, opens every `.rs` file except `tester_state.rs`, fails if any file contains `allocation = 'idle'` or `allocation='idle'` literal. Enforces the rule that only `tester_state.rs` may write the unlock pair.

2. **`concurrent_acquires_only_one_wins`** (gated by `#[ignore]`, requires `DASHBOARD_DB_URL`): inserts a project + tester + 2 benchmark configs, spawns two tasks calling `try_acquire` concurrently against the same tester, asserts exactly one returns `AcquireOutcome::Acquired`. Cleans up.

- [ ] **Step 1:** Add the tests
- [ ] **Step 2:** Run `cargo test -p networker-dashboard --lib release_is_only_writer_of_idle_unlock` — passes
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/tester_state.rs
git commit -m "test(dashboard): lock invariant grep guard + race test scaffolding"
```

---

### Task 8: Azure region → timezone map

**Files:**
- Create `crates/networker-dashboard/src/services/azure_regions.rs`
- Modify `crates/networker-dashboard/src/services/mod.rs`

Module exposes:
- `pub fn region_timezone(region: &str) -> chrono_tz::Tz` — hardcoded match arm with US, Europe, Japan, Asia, Australia, Brazil, Canada, India, Africa, UAE regions. Default `chrono_tz::UTC` for unknowns.
- `pub fn next_shutdown_at(region: &str, local_hour: i16, now_utc: DateTime<Utc>) -> DateTime<Utc>` — converts now_utc to region's local time, builds a target time at `local_hour:00:00` today, returns it if in the future, otherwise returns tomorrow at the same hour. Handles DST ambiguity by falling through to the next day.

Inline tests:
- `known_regions_resolve` — `eastus → US/Eastern`, `westus2 → US/Pacific`, `japaneast → Asia/Tokyo`, `uksouth → Europe/London`
- `unknown_region_falls_back_to_utc`
- `next_shutdown_is_in_future` — for `now = Utc::now()`, the returned timestamp is strictly greater
- `next_shutdown_within_24h` — the returned timestamp is at most 25 hours after now

- [ ] **Step 1:** Write the module
- [ ] **Step 2:** Add `pub mod azure_regions;` to `services/mod.rs`
- [ ] **Step 3:** `cargo test -p networker-dashboard --lib azure_regions` — 4 tests pass
- [ ] **Step 4:** Commit

```bash
git add crates/networker-dashboard/src/services/azure_regions.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): azure region → timezone map for shutdown scheduling"
```

---

## Phase C — Background services

### Task 9: tester_install service (extracted from existing code)

**Goal:** Extract the existing `deploy_chrome_harness` logic from `benchmarks/orchestrator/src/executor.rs` (lines 162–249) into a reusable `services/tester_install.rs` so it runs once at tester creation, not per benchmark.

**Files:** Create `crates/networker-dashboard/src/services/tester_install.rs`; wire into `services/mod.rs`.

The module exposes `pub async fn install_tester(tester: &ProjectTesterRow, progress: impl Fn(&str)) -> Result<()>` which:

1. Waits up to 5 minutes for SSH to accept connections (30 attempts × 10s).
2. Stops `unattended-upgrades` service to avoid dpkg locks.
3. Installs `curl git ca-certificates` via apt.
4. Fresh-clones the networker-tester repo to `/tmp/nwk-repo` (rm + clone, no shallow pull).
5. Creates `/opt/bench/chrome-harness` directory.
6. Downloads `google-chrome-stable_current_amd64.deb` (skipped if already installed).
7. Installs Chrome `.deb` + Node.js + npm in one apt session.
8. Copies `package.json`, `runner.js`, `test-page.html` from the cloned repo into `/opt/bench/chrome-harness`, runs `npm install --production --silent`.
9. Verifies `/opt/bench/chrome-harness/runner.js` exists and `google-chrome` is on PATH.

The `progress` callback is invoked with a short string before each step so the calling code (Task 15's create endpoint background task) can update `status_message`.

The SSH executor is a wrapper around `tokio::process::Command::new("ssh")` with `StrictHostKeyChecking=no`, `UserKnownHostsFile=/dev/null`, `ConnectTimeout=10`. Returns the stdout on success, errors with stdout+stderr on failure.

- [ ] **Step 1:** Write the module
- [ ] **Step 2:** Add `pub mod tester_install;` to `services/mod.rs`
- [ ] **Step 3:** `cargo build -p networker-dashboard` — clean
- [ ] **Step 4:** Commit

```bash
git add crates/networker-dashboard/src/services/tester_install.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): tester_install service (one-time Chrome+Node setup)"
```

---

### Task 10: Tester dispatcher (`promote_next` + sweep loop)

**Spec ref:** § "Queue dispatcher" (Section 6b)

**Files:** Create `crates/networker-dashboard/src/services/tester_dispatcher.rs`; wire into `services/mod.rs`.

The module exposes:
- `pub async fn promote_next(client, tester_id) -> Result<Option<Uuid>>` — uses `FOR UPDATE SKIP LOCKED` to atomically promote the oldest queued benchmark for the tester to `pending` status. Returns its `config_id` or `None` if the queue is empty.
- `pub async fn sweep_loop(state)` — `tokio::time::interval(30s)` tick. Scans for `(power_state='running' AND allocation='idle')` testers with at least one queued benchmark, calls `promote_next` for each.

Both functions write to `service_log` with `subsystem='tester_dispatcher'`. Logs at `info` level when a benchmark is promoted, `debug` for ticks.

The SQL for `promote_next`:

```sql
UPDATE benchmark_config
   SET status    = 'pending',
       queued_at = NULL
 WHERE config_id = (
     SELECT config_id FROM benchmark_config
      WHERE tester_id = $1 AND status = 'queued'
      ORDER BY queued_at ASC
      LIMIT 1
      FOR UPDATE SKIP LOCKED
 )
 RETURNING config_id
```

Unit test: `promote_next_returns_none_on_empty_queue` (mocked DB).

- [ ] **Step 1:** Write the module + test
- [ ] **Step 2:** `cargo test -p networker-dashboard --lib tester_dispatcher` passes
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/tester_dispatcher.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): tester dispatcher promote_next + sweep loop"
```

---

### Task 11: Tester scheduler (auto-shutdown loop)

**Spec ref:** § "Auto-shutdown loop" (Section 6a)

**Files:** Create `crates/networker-dashboard/src/services/tester_scheduler.rs`; wire into `services/mod.rs`.

The module exposes `pub async fn auto_shutdown_loop(state)` running on a 60-second tick.

Each tick runs the eligibility query from the spec (filters on `auto_shutdown_enabled`, `next_shutdown_at < NOW()`, `power_state='running'`, `allocation='idle'`, AND `NOT EXISTS` benchmark in `('queued','pending','running')` for this tester).

For each due tester, spawns a per-tester task that:
1. Re-validates the drain check (race window between SELECT and shutdown).
2. If still drained: `try_power_transition('running','stopping')`, calls a `vm_deallocate` helper (wraps `az vm deallocate`), on success sets `power_state='stopped'`, recomputes `next_shutdown_at` via `azure_regions::next_shutdown_at`, resets `shutdown_deferral_count` to 0.
3. If not drained: increments `shutdown_deferral_count`, pushes `next_shutdown_at` forward 5 minutes, retries next tick.
4. After 3 consecutive deferrals: writes a high-severity `service_log` entry tagged `tester_shutdown_stuck` with the names of the benchmarks holding the tester open.

The `vm_deallocate` helper specifically calls `az vm deallocate` (not `az vm stop`) because the cost-savings goal requires releasing compute resources.

Unit test: `deferral_count_caps_at_3_warning` — mocked, verifies the warning log fires after the 3rd consecutive deferral.

- [ ] **Step 1:** Write the module
- [ ] **Step 2:** Run unit tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/tester_scheduler.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): tester scheduler auto-shutdown with deferral cap"
```

---

### Task 12: Tester recovery (crash recovery + opt-in auto-probe)

**Spec ref:** § "Crash recovery on startup" (Section 6c)

**Files:** Create `crates/networker-dashboard/src/services/tester_recovery.rs`; wire into `services/mod.rs`.

Module exposes:
- `pub async fn recover_on_startup(state)` — sleeps 5 minutes after dashboard startup (grace period for in-flight background tasks to resume), then runs the recovery scan.

The scan does two things:

1. **Force-release stuck locks.** Find testers where `allocation='locked'` AND the referenced `benchmark_config` is in a terminal status (`completed`, `completed_with_errors`, `failed`, `cancelled`). For each, call `tester_state::force_release`, then `tester_dispatcher::promote_next`. Audit-log as `crash_recovery_lock_released`.

2. **Handle stuck transient power states.** Find testers where `power_state IN ('starting','stopping','upgrading','provisioning')` AND `updated_at < NOW() - INTERVAL '30 minutes'`. For each:
   - If `auto_probe_enabled = TRUE`: query Azure VM state via `az vm show`, resync `power_state` to match (`Running → 'running'`, `Deallocated → 'stopped'`, etc.). If the probe itself fails, set `power_state='error'` with reason. Audit-log as `crash_recovery_auto_probed`.
   - If `auto_probe_enabled = FALSE`: set `power_state='error'` with `status_message="Stuck in {state} after dashboard restart — needs manual recovery (auto-probe disabled)"`. Audit-log as `crash_recovery_marked_error`.

Unit test: `classify_stuck_testers_by_age` — given a mocked DB, verify only testers with `updated_at` >30 min old are picked up.

- [ ] **Step 1:** Write the module
- [ ] **Step 2:** Run tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/tester_recovery.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): tester crash recovery with opt-in auto-probe"
```

---

### Task 13: Version refresh service

**Spec ref:** § "Latest-version refresh" (Section 6d)

**Files:** Create `crates/networker-dashboard/src/services/version_refresh.rs`; wire into `services/mod.rs`.

Module exposes:
- `pub async fn refresh_latest_version_loop(state)` — `tokio::time::interval(6 * 3600s)`. On each tick: try GitHub releases API (`https://api.github.com/repos/irlm/networker-tester/releases/latest`) with `reqwest`, fall back to `env!("CARGO_PKG_VERSION")` on failure. Store in `state.latest_known_version: Arc<RwLock<String>>`.
- `pub async fn refresh_now(state) -> Result<String>` — manual trigger from the API endpoint, runs the same resolution and returns the new value.
- Helper `pick_higher_semver(a: &str, b: Option<&str>) -> String`.

Resolution order: GitHub preferred (canonical source); dashboard `CARGO_PKG_VERSION` is the floor when GitHub is unreachable.

Unit test: `pick_higher_semver_returns_newer` — `pick_higher_semver("0.24.0", Some("0.25.1")) == "0.25.1"`.

- [ ] **Step 1:** Write the module
- [ ] **Step 2:** Run tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/version_refresh.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): version refresh loop + manual refresh helper"
```

---

## Phase D — REST API

### Task 14: Tester REST handlers — listing & inspection

**Spec ref:** § "API surface" → "Listing & inspection"

**Files:** Create `crates/networker-dashboard/src/api/testers.rs`.

Endpoints (all require `ProjectRole::Member`):
- `GET /api/projects/{pid}/testers` — calls `db::project_testers::list_for_project`. Returns array of `ProjectTesterRow`.
- `GET /api/projects/{pid}/testers/{tid}` — calls `db::project_testers::get`. 404 if not in project (even for platform admins).
- `GET /api/projects/{pid}/testers/regions` — returns `cloud_account.regions` for the project's azure account.
- `GET /api/projects/{pid}/testers/{tid}/queue` — returns `{ tester_id, running, queued }`. The `running` field is non-null if `allocation='locked'`; the `queued` field is an array of benchmarks ordered by `queued_at` with their position and ETA computed from `avg_benchmark_duration_seconds`.
- `GET /api/projects/{pid}/testers/{tid}/cost_estimate` — uses a hardcoded `vm_size → hourly_usd` lookup (Standard_D2s_v3 = $0.096/hr) and `(24 - shutdown_hours_per_day) * 30 * hourly_usd` for the monthly figure.

Each handler uses the existing `extract_project_id` and `require_project_role` middleware patterns from neighboring API modules.

Integration test in `tests/api_testers.rs`: insert a tester directly into the DB, hit each GET endpoint, verify response shape and that cross-project access returns 404.

- [ ] **Step 1:** Write the handlers + tests
- [ ] **Step 2:** Run integration test
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/api/testers.rs crates/networker-dashboard/tests/api_testers.rs
git commit -m "feat(dashboard): tester listing + inspection REST endpoints"
```

---

### Task 15: Tester REST handlers — lifecycle

**Spec ref:** § "API surface" → "Lifecycle"

**Files:** Modify `crates/networker-dashboard/src/api/testers.rs`.

Endpoints (all require `ProjectRole::Admin`):

- `POST /api/projects/{pid}/testers` — body `{ name, cloud, region, vm_size?, auto_shutdown_local_hour?, auto_probe_enabled? }`. Calls `db::project_testers::insert`. Spawns background task that:
  1. Creates the Azure VM via `az vm create` (uses cloud_account credentials)
  2. Updates the row with `vm_resource_id`, `public_ip`, `vm_name`
  3. Calls `services::tester_install::install_tester` with a progress callback that updates `status_message`
  4. On success: transitions `power_state` from `provisioning → running`, sets `installer_version` to current, sets `next_shutdown_at`
  5. On failure: sets `power_state='error'` with the failure reason in `status_message`

  Rate limit: refuses with 429 if the project has created 5+ testers in the past hour OR has 10+ testers total.

- `POST /api/projects/{pid}/testers/{tid}/start` — refuses 409 if `power_state != 'stopped'`. Background task: `try_power_transition('stopped','starting')`, `az vm start`, wait for SSH, transition `starting → running`.

- `POST /api/projects/{pid}/testers/{tid}/stop` — refuses 409 if `allocation != 'idle'` OR if any benchmark `status IN ('queued','pending','running')` for this tester. Background task: `running → stopping → stopped` with `az vm deallocate`.

- `POST /api/projects/{pid}/testers/{tid}/upgrade` — refuses 409 if `allocation != 'idle'` OR queue non-empty. Body `{ confirm: true }`. Background task: transitions `allocation idle → upgrading`, calls `tester_install::install_tester` again, on success transitions `upgrading → idle` and updates `installer_version` + `last_installed_at`.

- `DELETE /api/projects/{pid}/testers/{tid}` — refuses 409 if `power_state IN ('provisioning','starting','stopping','upgrading')` OR `allocation != 'idle'` OR queue non-empty. Destroys Azure resources via `az vm delete --yes`, then `db::project_testers::delete`. Historical `benchmark_config` rows have `tester_id` cleared by `ON DELETE SET NULL`; their `tester_*_snapshot` columns preserve identity.

Each mutating endpoint calls an `audit_tester_action` helper that writes a `tester_action` entry to `service_log` with `actor_user_id`, `tester_id`, `project_id`, action, outcome.

Integration tests: each endpoint exercised, refusal cases verified.

- [ ] **Step 1:** Write the handlers
- [ ] **Step 2:** Add integration tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/api/testers.rs crates/networker-dashboard/tests/api_testers.rs
git commit -m "feat(dashboard): tester lifecycle REST endpoints (create/start/stop/upgrade/delete)"
```

---

### Task 16: Tester REST handlers — schedule + recovery

**Spec ref:** § "API surface" → "Schedule control" + "Recovery actions"

**Files:** Modify `crates/networker-dashboard/src/api/testers.rs`.

Endpoints (all `ProjectRole::Admin`):

- `PATCH /api/projects/{pid}/testers/{tid}/schedule` — body `{ auto_shutdown_enabled?, auto_shutdown_local_hour? }`. Updates the row, recomputes `next_shutdown_at` via `azure_regions::next_shutdown_at`.
- `POST /api/projects/{pid}/testers/{tid}/postpone` — body accepts three forms: `{ until: ISO8601 }`, `{ add_hours: int }`, `{ skip_tonight: true }`. Updates `next_shutdown_at` only; recurring schedule unchanged.
- `POST /api/projects/{pid}/testers/{tid}/probe` — calls a helper that runs `az vm show` and parses the power state. Resyncs the row's `power_state` to match. Refuses 409 if `allocation IN ('locked','upgrading')`. State transitions: Running → `power_state='running'`, Deallocated → `power_state='stopped'`, Starting/Stopping → recheck after 15s, Unknown → `power_state='error'`.
- `POST /api/projects/{pid}/testers/{tid}/force-stop` — body `{ confirm: true, reason: String }`. Refuses 409 if `(power_state='running' AND allocation='locked')`. Sets `power_state='stopped', allocation='idle'` directly without any Azure operation. Audit-logs the actor user and reason.

Each endpoint calls `audit_tester_action`.

Integration tests for each.

- [ ] **Step 1:** Write the handlers
- [ ] **Step 2:** Tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/api/testers.rs crates/networker-dashboard/tests/api_testers.rs
git commit -m "feat(dashboard): tester schedule + recovery REST endpoints"
```

---

### Task 17: Refresh-latest-version endpoint + audit hook completion

**Files:** Modify `crates/networker-dashboard/src/api/testers.rs`; finalize the `audit_tester_action` helper in `services/tester_state.rs` (or create `services/tester_audit.rs`).

Endpoint:
- `POST /api/projects/{pid}/testers/refresh-latest-version` — Admin only. Calls `version_refresh::refresh_now`, returns `{ "latest_version": "v0.25.0" }`.

Audit hook signature: `audit_tester_action(state, project_id, tester_id, actor_id, action: &str, outcome: &str, message: Option<&str>)`. Writes a row to `service_log` with `service='dashboard'`, `subsystem='tester_action'`, `event=action`, and the relevant context.

Tracked actions: `tester_created`, `tester_deleted`, `tester_start_requested`, `tester_stop_requested`, `tester_upgrade_requested`, `tester_schedule_changed`, `tester_postponed`, `tester_lock_acquired`, `tester_lock_released`, `tester_lock_force_released`, `tester_auto_shutdown_completed`, `tester_auto_shutdown_deferred`, `tester_shutdown_stuck`, `tester_probed`, `tester_force_stopped`.

Integration test: verify a `tester_action` log entry appears in `service_log` after a `POST /testers/{tid}/start` call.

- [ ] **Step 1:** Write the endpoint + helper
- [ ] **Step 2:** Wire `audit_tester_action` into all mutating endpoints from Tasks 15+16
- [ ] **Step 3:** Tests
- [ ] **Step 4:** Commit

```bash
git add crates/networker-dashboard/src/api/testers.rs crates/networker-dashboard/src/services/
git commit -m "feat(dashboard): version refresh endpoint + tester action audit logging"
```

---

### Task 18: Wire tester routes into app router

**Files:** Modify `crates/networker-dashboard/src/api/mod.rs`.

- Add `pub mod testers;`
- Add a `testers::router()` function in `api/testers.rs` that registers all 15 routes from Tasks 14–17 under a sub-router scoped to `/projects/{pid}/testers`
- Merge into the existing project router

Verification: start the dashboard locally, `curl -s -H "Authorization: Bearer ..." http://localhost:3000/api/projects/{pid}/testers` returns `[]`.

- [ ] **Step 1:** Wire the routes
- [ ] **Step 2:** Manual smoke test with curl
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/api/mod.rs crates/networker-dashboard/src/api/testers.rs
git commit -m "feat(dashboard): wire tester routes into app router"
```

---

## Phase E — WebSocket push channel

### Task 19: WebSocket message types in networker-common

**Spec ref:** § "Push-based updates" → "Subscribe / update messages"

**Files:** Create `crates/networker-common/src/tester_messages.rs`; modify `lib.rs`.

Define serde-tagged enum variants:
- `SubscribeTesterQueue { project_id, tester_ids }`
- `UnsubscribeTesterQueue { tester_ids }`
- `TesterQueueSnapshot { project_id, tester_id, seq, running, queued }`
- `TesterQueueUpdate { project_id, tester_id, seq, trigger, running, queued }`
- `PhaseUpdate { project_id, entity_type, entity_id, seq, phase, outcome, message, applied_stages }`

All messages include `project_id` (required for project enforcement). All update messages include `seq: u64`.

Round-trip serde tests for each variant.

- [ ] **Step 1:** Define the enums
- [ ] **Step 2:** Tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-common/src/tester_messages.rs crates/networker-common/src/lib.rs
git commit -m "feat(common): WebSocket message types for tester queue + phase updates"
```

---

### Task 20: Tester queue hub (in-process pub/sub)

**Spec ref:** § "Push-based updates" → "Project enforcement at every layer"

**Files:** Create `crates/networker-dashboard/src/services/tester_queue_hub.rs`; wire into `services/mod.rs`.

```rust
pub struct TesterQueueHub {
    subscribers: Arc<RwLock<HashMap<(ProjectId, TesterId), Vec<WsSender>>>>,
    seq_counters: Arc<RwLock<HashMap<TesterId, u64>>>,
}

impl TesterQueueHub {
    pub fn new() -> Self { ... }
    pub async fn subscribe(&self, project_id, tester_id, sender) -> Result<u64>
    pub async fn unsubscribe(&self, project_id, tester_id, sender_id)
    pub async fn notify(&self, project_id, tester_id, trigger)
}
```

`subscribe` returns the current `seq` for the tester (used to populate the snapshot). `notify` increments the seq, builds a `TesterQueueUpdate`, broadcasts to all senders for that key.

Subscription limits: refuses if subscriber count for `(project_id)` exceeds `DASHBOARD_MAX_SUBS_PER_PROJECT` (default 50, env var override).

Concurrent test under `--include-ignored`: spawn 10 subscribers, fire `notify`, verify all receive the update.

- [ ] **Step 1:** Write the hub
- [ ] **Step 2:** Tests
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/services/tester_queue_hub.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): in-process tester queue hub for WebSocket push"
```

---

### Task 21: WebSocket subscription handler

**Spec ref:** § "Push-based updates" → "Subscribe / update messages"

**Files:** Create `crates/networker-dashboard/src/api/tester_ws.rs`; wire into the app router.

Handler at `/ws/testers`. Steps:
1. Validates JWT in handshake (existing auth pattern from agent WS).
2. On `subscribe_tester_queue` message: validates `project_id` against `project_member` for the user; validates each `tester_id` belongs to that project (queries `project_tester WHERE project_id=$1 AND tester_id=ANY($2)`); registers each (project_id, tester_id) pair in the hub.
3. Sends `tester_queue_snapshot` immediately for each subscribed tester.
4. On `unsubscribe_tester_queue`: removes subscriptions.
5. Rate limit: max 10 subscribe/unsubscribe messages per minute per connection (env var `DASHBOARD_MAX_SUB_MSGS_PER_MIN`).
6. On any update event from the hub, forwards to the WS sink.

Integration test: open a WS connection, subscribe, verify snapshot arrives, manually call `hub.notify()`, verify update arrives.

- [ ] **Step 1:** Write the handler
- [ ] **Step 2:** Wire route
- [ ] **Step 3:** Test
- [ ] **Step 4:** Commit

```bash
git add crates/networker-dashboard/src/api/tester_ws.rs crates/networker-dashboard/src/api/mod.rs
git commit -m "feat(dashboard): WebSocket subscription handler with project enforcement"
```

---

## Phase F — Orchestrator rewrite

### Task 22: Tester lookup helper in orchestrator

**Files:** Modify `benchmarks/orchestrator/src/executor.rs`.

Add `async fn lookup_tester(state, config_id) -> Result<ProjectTesterRow>` that joins `benchmark_config` and `project_tester` to fetch the tester for the given benchmark. Returns an error if `tester_id IS NULL` (the SQL CHECK constraint should make this impossible for application-mode benchmarks, but defend against it).

- [ ] **Step 1:** Add the helper
- [ ] **Step 2:** `cargo build` clean
- [ ] **Step 3:** Commit

```bash
git add benchmarks/orchestrator/src/executor.rs
git commit -m "feat(orchestrator): tester lookup helper"
```

---

### Task 23: Replace execute_testbed_application body with lock acquisition flow

**Spec ref:** § "Orchestrator integration" → "New flow"

**Files:** Modify `benchmarks/orchestrator/src/executor.rs` (function at line 800).

Replace the entire body of `execute_testbed_application` with the new flow from the spec. Pseudocode in spec § "New flow"; implementer translates to Rust:

1. `lookup_tester(config.tester_id)`
2. Loop: call `try_acquire(tester.id, config.id)`
   - On `Acquired`: break out of loop, proceed
   - On `NeedsStart`: call `vm_start_helper(tester)`, wait for SSH, retry acquire
   - On `Transient | Upgrading | AlreadyLockedBy`: set `benchmark_config.status='queued'`, `queued_at=NOW()`, return Queued status to caller (do NOT proceed)
   - On `Errored`: fail benchmark with explanatory message
3. After acquire: `refresh_bench_token(tester)`, scp the token to the VM
4. For each (proxy, lang) in `config.testbed.proxies × config.testbed.languages`: deploy the proxy, deploy the language server, run the chrome benchmark
5. Wrap the per-(proxy,lang) loop in a `try { ... } finally { release_guard }` pattern using a `Drop` impl on a guard struct so the lock is always released even on panic
6. After release: call `notify_queue_dispatcher(state, tester.id)` (Task 10's promote_next)
7. Update `benchmark_config.status = 'completed' | 'completed_with_errors' | 'failed'` based on per-(proxy,lang) outcomes

Each phase transition (`queued → starting → deploy → running → collect → done`) calls a helper that emits a `phase_update` message via the WebSocket hub AND updates `benchmark_config.current_phase`.

Existing integration tests must still pass (with mocked SSH).

- [ ] **Step 1:** Rewrite the function
- [ ] **Step 2:** `cargo test --workspace --lib`
- [ ] **Step 3:** `bash tests/cli_smoke.sh`
- [ ] **Step 4:** Commit

```bash
git add benchmarks/orchestrator/src/executor.rs
git commit -m "feat(orchestrator): replace execute_testbed_application with tester lock flow"
```

---

### Task 24: Remove deploy_chrome_harness call from per-benchmark path

**Files:** Modify `benchmarks/orchestrator/src/executor.rs`.

Find the `deploy_chrome_harness(vm).await` call in the new `execute_testbed_application` flow and **delete it**. The harness is now installed once at tester creation time (Task 9, called from Task 15's create endpoint background task).

Keep the `deploy_chrome_harness` function definition for now — Task 25 will remove it entirely.

After this task: application benchmarks against an existing tester start in seconds instead of 15+ minutes.

- [ ] **Step 1:** Delete the call
- [ ] **Step 2:** Build and run smoke test
- [ ] **Step 3:** Commit

```bash
git add benchmarks/orchestrator/src/executor.rs
git commit -m "refactor(orchestrator): remove per-benchmark Chrome harness install"
```

---

### Task 25: Delete dead ephemeral VM provisioning code

**Spec ref:** § "Cleanup & deployment plan" → "What we delete"

**Files:** Modify `benchmarks/orchestrator/src/executor.rs` and possibly `benchmarks/orchestrator/src/provisioner.rs`.

1. Delete the original `deploy_chrome_harness` function (now in `tester_install.rs`).
2. Delete any `provisioner::create_vm` and `provisioner::destroy_vm` call sites remaining in the application benchmark path.
3. Delete the `auto_teardown` config option from `BenchmarkConfig` if it's only used by the orchestrator.
4. Run `cargo clippy --all-targets -- -D warnings` and remove any dead-code warnings (unused imports, unused helper functions).

Tests must still pass after the deletions.

- [ ] **Step 1:** Delete the dead code
- [ ] **Step 2:** Clippy clean
- [ ] **Step 3:** Smoke test
- [ ] **Step 4:** Commit

```bash
git add benchmarks/orchestrator/src/
git commit -m "chore(orchestrator): delete dead ephemeral VM provisioning code"
```

---

## Phase G — Frontend

### Task 26: PhaseBar React component

**Files:** Create `dashboard/src/components/PhaseBar.tsx` and `PhaseBar.test.tsx`.

```tsx
type Phase = 'queued' | 'starting' | 'deploy' | 'running' | 'collect' | 'done';
type Outcome = 'success' | 'partial_success' | 'failure' | 'cancelled';

interface PhaseBarProps {
  phase: Phase;
  outcome: Outcome | null;
  appliedStages: Phase[];
}

export function PhaseBar({ phase, outcome, appliedStages }: PhaseBarProps) {
  // Renders a row of segments, one per stage in appliedStages.
  // Earlier stages: cyan-600 fill, dim text.
  // Active stage (current phase, not done): purple-500 fill with animate-pulse, bright text.
  // Later stages: gray-800 fill, muted text.
  // On done: all segments cyan-600, last segment colored by outcome (green/yellow/red/gray).
}
```

vitest test:
- `renders correct number of segments`
- `marks active stage with pulse animation`
- `colors final stage by outcome on done`

- [ ] **Step 1:** Write the component + tests
- [ ] **Step 2:** `cd dashboard && npm test`
- [ ] **Step 3:** Commit

```bash
git add dashboard/src/components/PhaseBar.tsx dashboard/src/components/PhaseBar.test.tsx
git commit -m "feat(dashboard): PhaseBar shared progress component"
```

---

### Task 27: WebSocket subscription hooks

**Files:** Create `dashboard/src/hooks/useTesterSubscription.ts` and `dashboard/src/hooks/usePhaseSubscription.ts`.

`useTesterSubscription(projectId, testerIds)`:
- Opens a WebSocket to `/ws/testers` (use the existing connection pattern from `useWebSocket.ts`)
- Sends `subscribe_tester_queue` with `project_id` and `tester_ids`
- Stores received `tester_queue_snapshot` and `tester_queue_update` messages in state, keyed by `tester_id`
- Drops messages with `seq <= last_seen_seq` for the tester (within a single connection)
- On disconnect: exponential backoff reconnect, re-subscribes, treats new snapshot as authoritative
- Returns `Map<TesterId, QueueState>`

`usePhaseSubscription(projectId, entityType, entityId)`:
- Same pattern, subscribes to phase updates for a single entity
- Returns `{ phase, outcome, message, appliedStages }` or null

vitest test with mocked WebSocket: hook returns the latest snapshot after a subscribe message.

- [ ] **Step 1:** Write both hooks
- [ ] **Step 2:** Tests
- [ ] **Step 3:** Commit

```bash
git add dashboard/src/hooks/useTesterSubscription.ts dashboard/src/hooks/usePhaseSubscription.ts
git commit -m "feat(dashboard): tester + phase WebSocket subscription hooks"
```

---

### Task 28: CreateTesterModal

**Spec ref:** § "Wizard UX" → "Create-tester modal"

**Files:** Create `dashboard/src/components/CreateTesterModal.tsx`.

Modal with the form fields from spec § "Create-tester modal":
- Cloud (dropdown, only Azure for now)
- Region (dropdown, populated from `GET /testers/regions`)
- Name (text input, validated unique on blur via `GET /testers?name=...`)
- VM size (dropdown with 2-3 presets)
- Auto-shutdown enabled (checkbox)
- Auto-shutdown local hour (dropdown 0-23)
- Auto-probe enabled (checkbox, off by default, with explanatory tooltip)

On submit: `POST /api/projects/{pid}/testers`. Transitions to "creating" state with progress checklist subscribed via `useTesterSubscription` to the new tester's id. When `power_state='running'` and `allocation='idle'`, fires the `onCreated` callback with the new tester id.

Pre-fill props: `defaultCloud`, `defaultRegion` so the wizard can pre-fill from the user's region selection.

vitest test: render, fill form, mock API, verify POST body shape, verify creating state is shown after submit.

- [ ] **Step 1:** Write the modal + tests
- [ ] **Step 2:** Commit

```bash
git add dashboard/src/components/CreateTesterModal.tsx dashboard/src/components/CreateTesterModal.test.tsx
git commit -m "feat(dashboard): CreateTesterModal component"
```

---

### Task 29: TesterDetailDrawer

**Spec ref:** § "Tester Management page" → "Detail drawer"

**Files:** Create `dashboard/src/components/TesterDetailDrawer.tsx`.

Slide-in drawer (right side) with the sections from the spec:

1. **Status** — pill with `power_state`/`allocation` combination, locked-by info if `allocation='locked'`
2. **Identity** — cloud, region, VM size, vm_name, public_ip, created_by/at
3. **Version** — installed version, latest known version, "Update available" badge with version diff and a `[ View changelog → ]` link to the GitHub releases page
4. **Cost estimate** — calls `/cost_estimate`, shows `monthly_with_schedule` vs `monthly_always_on`
5. **Usage** — `benchmark_run_count`, `avg_benchmark_duration_seconds`, `last_used_at`
6. **Auto-shutdown** — schedule, next shutdown time, deferral count if > 0, three buttons: `[ Edit schedule ]`, `[ Postpone… ]`, `[ Disable ]`
7. **Recovery** — auto-probe toggle + `[ Run probe now ]` button
8. **Queue** — running benchmark, queued benchmarks (uses `useTesterSubscription`)
9. **Recent activity** — last 10 audit log entries from `service_log` filtered by `tester_id`, `[ View full history → ]` link
10. **Danger zone** — `[ Stop tester ]`, `[ Delete tester ]` (refused while running/queued)

When `power_state='error'`: shows a prominent **"Fix tester first"** panel above the normal sections with four buttons:
- `[ Run probe ]` — `POST /testers/{tid}/probe`
- `[ Reinstall tester ]` — `POST /testers/{tid}/upgrade`
- `[ Force to stopped ]` — `POST /testers/{tid}/force-stop` with confirmation modal
- `[ Delete tester ]`

There is NO "Mark as healthy" button (intentional, per spec).

vitest test: render in idle state, locked state, error state — verify correct sections appear.

- [ ] **Step 1:** Write the component + tests
- [ ] **Step 2:** Commit

```bash
git add dashboard/src/components/TesterDetailDrawer.tsx dashboard/src/components/TesterDetailDrawer.test.tsx
git commit -m "feat(dashboard): TesterDetailDrawer component"
```

---

### Task 30: TesterRegionGroup + TestersPage

**Spec ref:** § "Tester Management page" → "Page layout" (Layout C — region accordion)

**Files:** Create `dashboard/src/components/TesterRegionGroup.tsx` and `dashboard/src/pages/TestersPage.tsx`.

`TesterRegionGroup`:
- Collapsible card with region header (`▾ azure / eastus    3 testers · 1 running · 2 in queue   [+ add to eastus]`)
- Body: list of tester rows, each with status pill, name, version, queue depth, ETA, action buttons
- Click row → opens `TesterDetailDrawer`

`TestersPage`:
- Calls `GET /api/projects/{pid}/testers`
- Groups results by `(cloud, region)`
- Renders one `TesterRegionGroup` per group, sorted by region name
- Empty state: friendly explanation + `[ + Create your first tester in eastus (recommended) ]` button (pre-fills `cloud=azure, region=eastus, name=eastus-1, vm_size=Standard_D2s_v3, auto_shutdown_enabled=true, auto_shutdown_local_hour=23` in the modal)
- Page header: `[ Refresh latest version ]` button (admin only) calling `POST /testers/refresh-latest-version`
- Subscribes to `useTesterSubscription` for live updates

vitest test: render with empty state, with multiple regions, with mixed statuses. Verify clicking row opens drawer.

- [ ] **Step 1:** Write components
- [ ] **Step 2:** Tests
- [ ] **Step 3:** Commit

```bash
git add dashboard/src/components/TesterRegionGroup.tsx dashboard/src/pages/TestersPage.tsx
git commit -m "feat(dashboard): TestersPage with region-grouped accordion (layout C)"
```

---

### Task 31: Wizard TesterStep + remove ephemeral VM path

**Spec ref:** § "Wizard UX" → "Tester picker (step 5)"

**Files:**
- Create `dashboard/src/components/wizard/TesterStep.tsx`
- Modify `dashboard/src/pages/AppBenchmarkWizardPage.tsx`

`TesterStep`:
- Receives `cloud`, `region` from prior wizard steps
- Calls `GET /testers?cloud=...&region=...` and renders one of three states:
  - **State A (testers exist):** radio list with status pill, version, queue depth. Selecting a busy tester shows a warning panel with queue position + ETA computed from `avg_benchmark_duration_seconds`. Selecting a stopped tester shows "will auto-start". A `+ Create another tester in {region}` button opens the create modal pre-filled.
  - **State B (no testers in region):** explanation + single `[ Create {region} tester ]` button.
  - **State C (creating):** progress checklist subscribed via `useTesterSubscription` to the new tester's id. When status flips to idle, auto-selects.
- Wizard cannot advance until exactly one tester is selected.

Wizard rewrite in `AppBenchmarkWizardPage.tsx`:
- Insert `TesterStep` after the region selection step, before the languages step
- Wizard payload now includes `tester_id: string` (required)
- Remove the "Provision new VM" UI path entirely
- Remove the `auto_teardown` checkbox
- On submit: payload includes `tester_id` so the backend can populate the snapshot columns

vitest test: walk through the wizard end-to-end with a mocked API.

- [ ] **Step 1:** Write `TesterStep`
- [ ] **Step 2:** Modify wizard page
- [ ] **Step 3:** Tests
- [ ] **Step 4:** Commit

```bash
git add dashboard/src/components/wizard/TesterStep.tsx dashboard/src/pages/AppBenchmarkWizardPage.tsx
git commit -m "feat(dashboard): wizard requires tester selection, remove ephemeral VM path"
```

---

### Task 32: Navigation, banner, and PhaseBar adoption

**Files:**
- Modify `dashboard/src/components/ProjectNav.tsx` (or wherever the nav lives)
- Modify `dashboard/src/pages/BenchmarksPage.tsx`
- Modify `dashboard/src/pages/BenchmarkProgressPage.tsx`

Steps:
1. Add `Testers` link to project nav, between Schedules and Settings.
2. Add a dismissable banner at the top of `BenchmarksPage`: *"Persistent testers are now available — create one to make benchmarks 4× faster on subsequent runs."* with a link to the Testers page.
3. In `BenchmarkProgressPage.tsx`, replace the existing inline 6-stage bar with `<PhaseBar phase={phase} outcome={outcome} appliedStages={appliedStages} />`. Subscribe to `usePhaseSubscription` for the current benchmark.

- [ ] **Step 1:** Nav entry
- [ ] **Step 2:** Banner
- [ ] **Step 3:** PhaseBar adoption in BenchmarkProgressPage
- [ ] **Step 4:** `cd dashboard && npm run build && npm run lint`
- [ ] **Step 5:** Commit

```bash
git add dashboard/src/
git commit -m "feat(dashboard): navigation entry + benchmarks banner + PhaseBar adoption"
```

---

## Phase H — Bootstrap, smoke, ship

### Task 33: Reset bootstrap binary

**Spec ref:** § "Cleanup & deployment plan" → "The destructive bootstrap reset"

**Files:**
- Create `crates/networker-dashboard/bootstrap/reset-pre-prod.sql`
- Create `crates/networker-dashboard/src/bin/reset_pre_prod.rs`

The SQL script content is in the spec — `TRUNCATE TABLE ... RESTART IDENTITY CASCADE` over all tables, followed by `VACUUM FULL ... ANALYZE` outside the transaction.

The Rust binary:
1. Refuses to run unless `DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true` env var is set. Prints a clear error referencing the command needed to enable it.
2. Connects to the DB.
3. Refuses to run if `SELECT count(*) FROM cloud_account WHERE name LIKE 'prod%' OR labels @> '{"env":"production"}'` returns > 0. Prints which projects have production cloud_accounts so the user can investigate.
4. Reads the SQL script file via `include_str!` and runs it via `client.batch_execute`.
5. Logs success and exits 0.

- [ ] **Step 1:** Write the SQL script
- [ ] **Step 2:** Write the binary
- [ ] **Step 3:** Run without env var, verify it refuses
- [ ] **Step 4:** Commit

```bash
git add crates/networker-dashboard/bootstrap/ crates/networker-dashboard/src/bin/
git commit -m "feat(dashboard): reset-pre-prod binary for one-time bootstrap"
```

---

### Task 34: Spawn background tasks in main.rs

**Files:** Modify `crates/networker-dashboard/src/main.rs`.

After the DB pool is initialized and the `AppState` is constructed, before the HTTP listener binds, spawn the four background tasks:

```rust
tokio::spawn(services::tester_scheduler::auto_shutdown_loop(state.clone()));
tokio::spawn(services::tester_dispatcher::sweep_loop(state.clone()));
tokio::spawn(services::tester_recovery::recover_on_startup(state.clone()));
tokio::spawn(services::version_refresh::refresh_latest_version_loop(state.clone()));
```

Verification: `cargo run -p networker-dashboard` — startup logs include all 4 task startup messages.

- [ ] **Step 1:** Add the spawn calls
- [ ] **Step 2:** Run dashboard locally, verify logs
- [ ] **Step 3:** Commit

```bash
git add crates/networker-dashboard/src/main.rs
git commit -m "feat(dashboard): spawn tester background tasks in main"
```

---

### Task 35: Complete CLI smoke scenarios 7 + 8

**Files:** Modify `tests/cli_smoke.sh`.

**Scenario 7 (CLI reports phases to dashboard):**
- Start a local Postgres + dashboard
- Create a temporary admin user via the bootstrap path
- Generate a JWT for that user
- Run `networker-tester` against `localhost:8080` with `--dashboard-url http://localhost:3000 --dashboard-token <jwt>`
- Query the dashboard's `benchmark_config` table for the resulting row, verify `current_phase='done'` and `outcome='success'`

**Scenario 8 (E2E persistent tester, gated by `SMOKE_TEST_AZURE=1`):**
- Start a local dashboard
- `POST /api/projects/{pid}/testers` to create a tester (real Azure VM)
- Poll `GET /testers/{tid}` until `power_state='running' AND allocation='idle'`
- `POST /benchmark-configs` with `tester_id=<new>` and `benchmark_type=application`
- `POST /benchmark-configs/{cid}/launch`
- Poll until terminal status
- Assert tester returned to `allocation='idle'`
- Assert tester was NOT destroyed (still exists)
- Cleanup: `DELETE /testers/{tid}`

- [ ] **Step 1:** Implement scenario 7
- [ ] **Step 2:** Implement scenario 8
- [ ] **Step 3:** Run with `SMOKE_TEST_DASHBOARD=1` set, scenario 7 passes
- [ ] **Step 4:** Commit

```bash
git add tests/cli_smoke.sh
git commit -m "test: complete CLI smoke scenarios 7 + 8"
```

---

### Task 36: CHANGELOG, version bump, final validation, ship

**Files:**
- Modify `Cargo.toml` (workspace) — version `0.24.2 → 0.25.0`
- Modify `install.sh` — `INSTALLER_VERSION="v0.25.0"`
- Modify `install.ps1` — `$InstallerVersion = "v0.25.0"`
- Modify `CHANGELOG.md` — new `## [0.25.0] - 2026-04-10` section listing all the new features

CHANGELOG content should mention:
- Persistent tester architecture (no more per-benchmark VM provisioning)
- Tester management page with region-grouped accordion
- Auto-shutdown with deferral cap and per-region timezones
- Queue dispatcher with FIFO ordering
- Push-based phase + queue updates over WebSocket
- Unified PhaseBar progress display across the dashboard
- Wizard tester picker (required step)
- CLI smoke test gate
- BREAKING: ephemeral VM provisioning removed; existing benchmark configs without a tester reference no longer work

Steps:
1. Update all four version locations
2. Run `cargo generate-lockfile && (cd benchmarks/orchestrator && cargo generate-lockfile)`
3. Run `bash tests/cli_smoke.sh` — all 8 scenarios pass (7 + 8 with env gates set)
4. Run `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace --lib`
5. Run `cd dashboard && npm run build && npm run lint`
6. Commit and push the branch
7. Open PR with the title "feat: persistent testers (v0.25.0)"
8. Wait for CI green
9. Merge

```bash
git add Cargo.toml Cargo.lock benchmarks/orchestrator/Cargo.lock install.sh install.ps1 CHANGELOG.md
git commit -m "chore: bump v0.25.0 — persistent testers"
git push -u origin <branch>
gh pr create --title "feat: persistent testers (v0.25.0)" --body "..."
```

---

## Final verification before merge

Run this exact checklist before clicking merge:

- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace --lib` passes
- [ ] `cargo build --workspace` clean
- [ ] `cargo build -p networker-tester --no-default-features` clean
- [ ] `cargo build -p networker-tester --all-features` clean
- [ ] `bash tests/cli_smoke.sh` — all scenarios pass (7 + 8 with env gates if Azure available)
- [ ] `shellcheck install.sh` clean
- [ ] `bats tests/installer.bats` passes
- [ ] `cd dashboard && npm run build && npm run lint` clean
- [ ] CHANGELOG, Cargo.toml, install.sh, install.ps1 all show v0.25.0
- [ ] Both Cargo.lock files updated and committed
- [ ] PR description references the design spec

After merge:
- Tag is auto-created via the Auto-tag workflow
- Release workflow builds binaries and deploys to alethedash.com
- After deploy: run `DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true cargo run --bin reset_pre_prod` against the production DB to wipe and bootstrap
- Log in with the temp `DASHBOARD_ADMIN_PASSWORD`, change the password
- Use Chrome MCP from a fresh Claude session to walk through the validation surfaces (project create → cloud_account → tester create → benchmark launch → queue → auto-shutdown verification)

---

## Self-review notes

This plan was self-reviewed against the spec on 2026-04-10:

- **Spec coverage:** Every section of the spec maps to one or more tasks. Data model → Tasks 4–5. Lock primitives → Tasks 6–7. Region timezones → Task 8. Tester install → Task 9. Background services → Tasks 10–13. REST API → Tasks 14–18. WebSocket → Tasks 19–21. Orchestrator → Tasks 22–25. Frontend → Tasks 26–32. Bootstrap + smoke → Tasks 33–36.
- **Placeholder scan:** No "TBD", "TODO", "implement later", or "fill in" placeholders remain.
- **Type consistency:** Tasks consistently use `power_state`, `allocation`, `locked_by_config_id` (the new two-axis names from the revised spec). No remaining references to a single `status` column on `project_tester`.
- **Build order:** Each phase's tasks depend only on prior phases' tasks. Task 23 (orchestrator rewrite) depends on Task 6 (lock primitives). Task 30 (TestersPage) depends on Task 27 (subscription hooks). Etc.
