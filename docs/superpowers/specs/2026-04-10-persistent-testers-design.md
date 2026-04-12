# Persistent Testers — Design Spec

**Date:** 2026-04-10
**Author:** Brainstorming session with project owner
**Status:** Approved, ready for implementation planning

---

## Problem

The orchestrator currently provisions a fresh Azure VM for every application benchmark, installs Chrome + Node.js + every language toolchain on it from scratch (~15–20 min), runs the benchmark, then destroys the VM. This is wasteful, slow, and the install routinely hits the 20-minute SSH timeout we set as a workaround. Multi-language benchmarks have been failing for days because of this single architectural choice.

The owner's stated architecture: the **tester** (the machine running the benchmark harness — Chrome, Node.js, runner.js) must be **persistent and reused across runs**, with Chrome always pre-installed. Endpoints are per-test (transient, fast to create). Testers are managed explicitly by the user with their own dedicated UI. Multiple testers per region are allowed so concurrent benchmarks don't fight for the same VM.

This spec describes a complete rebuild of the tester half of the orchestrator. Endpoints (language servers) are unaffected — they continue to install fresh on the tester VM at the start of each benchmark.

## Goals

- A tester is provisioned **once per region** and reused indefinitely across benchmarks
- Chrome and the Node.js harness are installed during the one-time tester creation, never on a per-benchmark basis
- Testers auto-shutdown nightly to control cost (region-local 11 PM by default), only if completely idle
- Users explicitly manage testers through a dedicated page — create, list, start/stop, upgrade, delete
- Benchmarks must reference a tester before they can launch — no fallback to ephemeral VMs
- Multiple benchmarks against the same tester queue with FIFO ordering, exclusive lock, full visibility
- Project-scoped enforcement at every layer — testers belong to one project, no sharing, no exceptions
- Unified phase progress display (`queued → starting → deploy → running → collect → done`) used across every benchmark and test in the dashboard
- Original `networker-tester` CLI keeps working — verified by a hard smoke test gate before merge

## Non-goals

- Multi-cloud (AWS / GCP) — Azure only for this spec, hooks left for future expansion
- Cross-project tester sharing — explicit non-goal, project isolation is non-negotiable
- Auto-upgrading testers when a new installer version drops — version is pinned, user-driven upgrade only
- Scaling testers horizontally (one tester runs one benchmark at a time, period — measurement integrity over throughput)
- Migrating production data — there is no production data; the deploy is treated as a fresh install with full database reset

## Architecture

A new domain object — `project_tester` — owns the lifecycle of a long-lived testbed VM. Benchmarks reference a tester by ID; the orchestrator acquires an exclusive lock on the tester before each run, queues if it's busy, and releases when done. Two background tasks handle scheduled shutdowns and queue dispatch. A new top-level page in the dashboard lets users see and manage their testers; the wizard requires picking one before launching any application benchmark.

### Key components

- `project_tester` table — the new domain object, with **two orthogonal state axes** (`power_state` for VM lifecycle, `allocation` for benchmark reservation), version tracking, schedule, lock holder
- `services/tester_state.rs` — atomic lock acquire/release/transition helpers
- `services/tester_dispatcher.rs` — promote next queued benchmark when a tester frees
- `services/tester_scheduler.rs` — auto-shutdown loop (defers if queue not empty)
- `services/tester_recovery.rs` — startup crash recovery + opt-in Azure auto-probe
- `services/tester_install.rs` — one-time install of Chrome + Node.js + harness on a fresh VM
- `services/azure_regions.rs` — region → IANA timezone map
- `services/version_refresh.rs` — periodic refresh of "latest installer version" for the update badge
- `api/testers.rs` — REST CRUD + lifecycle endpoints under `/api/projects/{pid}/testers`
- WebSocket subscription channel — push-based queue and phase updates, project-scoped
- `crates/networker-common::Phase` — shared enum used by every progress-tracking entity in the system
- `dashboard/src/pages/TestersPage.tsx` — region-grouped accordion management UI
- `dashboard/src/components/PhaseBar.tsx` — single shared progress bar component
- `dashboard/src/components/wizard/TesterStep.tsx` — required wizard step picking the tester for a new benchmark
- `tests/cli_smoke.sh` — hard pre-merge gate that verifies the `networker-tester` CLI still works

## Data model

### `project_tester` — new table

```sql
CREATE TABLE project_tester (
    tester_id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id          TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,

    -- identity
    name                TEXT NOT NULL,                 -- user-friendly, e.g. "eastus-1"; unique per project
    cloud               TEXT NOT NULL,                 -- 'azure'
    region              TEXT NOT NULL,                 -- 'eastus', 'westus', 'japaneast', ...
    vm_size             TEXT NOT NULL DEFAULT 'Standard_D2s_v3',

    -- cloud resource handles — NULL until provisioning succeeds
    vm_name             TEXT,           -- e.g. 'tester-eastus-2-a3f9c'
    vm_resource_id      TEXT,           -- full Azure resource ID
    public_ip           INET,           -- assigned by Azure on first start
    ssh_user            TEXT NOT NULL DEFAULT 'azureuser',

    -- ── Lifecycle state — split into two orthogonal axes ─────────────────
    -- Power state describes what the underlying VM is doing.
    -- Allocation describes whether the tester is reserved by a benchmark.
    -- These are NEVER folded into a single column because they answer
    -- different questions and can change independently.
    power_state         TEXT NOT NULL DEFAULT 'provisioning',
                        -- provisioning | starting | running | stopping | stopped | upgrading | error
                        -- Reflects the Azure VM's state, not benchmark activity.
    allocation          TEXT NOT NULL DEFAULT 'idle',
                        -- idle | locked | upgrading
                        -- 'idle' = available for a benchmark to acquire
                        -- 'locked' = exclusively held by exactly one benchmark
                        -- 'upgrading' = install.sh re-running; new acquisitions blocked
    status_message      TEXT,
    locked_by_config_id UUID REFERENCES benchmark_config(config_id) ON DELETE RESTRICT,
                        -- ON DELETE RESTRICT (NOT SET NULL): the FK must never silently
                        -- clear the lock, because that would leave allocation='locked'
                        -- with locked_by_config_id=NULL, violating the invariant below.
                        -- A benchmark_config row holding a lock cannot be deleted until
                        -- the lock is released through the orchestrator's release() path.

    -- version tracking
    installer_version   TEXT,                          -- e.g. 'v0.24.2'; NULL until first install
    last_installed_at   TIMESTAMPTZ,

    -- auto-shutdown schedule
    auto_shutdown_enabled    BOOLEAN  NOT NULL DEFAULT TRUE,
    auto_shutdown_local_hour SMALLINT NOT NULL DEFAULT 23,    -- 23 = 11 PM region-local
    next_shutdown_at         TIMESTAMPTZ,
    shutdown_deferral_count  SMALLINT NOT NULL DEFAULT 0,     -- consecutive defers tonight; reset on successful shutdown
    -- After 3 consecutive deferrals, scheduler emits a high-severity service_log
    -- entry tagged 'tester_shutdown_stuck' so the user is notified that the
    -- tester has been kept up beyond its scheduled stop time.

    -- recovery behaviour
    auto_probe_enabled  BOOLEAN NOT NULL DEFAULT FALSE,       -- opt-in: auto-resync from Azure on stuck states

    -- usage tracking (drives ETA calculation in queue position UI)
    last_used_at                  TIMESTAMPTZ,                -- updated each time a lock is acquired
    avg_benchmark_duration_seconds INTEGER,                   -- moving average across last 20 runs; NULL until 3 runs
    benchmark_run_count           INTEGER NOT NULL DEFAULT 0, -- lifetime count, used for "first benchmark on this tester" copy

    -- audit
    created_by          UUID NOT NULL REFERENCES app_user(user_id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, name)
);

-- Integrity invariants enforced at the database level
ALTER TABLE project_tester
    ADD CONSTRAINT lock_holder_implies_locked
    CHECK ((allocation = 'locked' AND locked_by_config_id IS NOT NULL)
        OR (allocation != 'locked' AND locked_by_config_id IS NULL));

ALTER TABLE project_tester
    ADD CONSTRAINT lock_requires_running_vm
    CHECK (allocation != 'locked' OR power_state = 'running');

CREATE INDEX idx_project_tester_project    ON project_tester(project_id);
CREATE INDEX idx_project_tester_power      ON project_tester(power_state)  WHERE power_state IN ('running','stopped');
CREATE INDEX idx_project_tester_alloc      ON project_tester(allocation)   WHERE allocation IN ('idle','locked');
CREATE INDEX idx_project_tester_shutdown   ON project_tester(next_shutdown_at) WHERE auto_shutdown_enabled = TRUE;
CREATE INDEX idx_project_tester_last_used  ON project_tester(project_id, last_used_at DESC NULLS LAST);
```

### Two orthogonal state axes

`power_state` and `allocation` describe different things and must never be conflated:

- **`power_state`** answers "what is the VM doing right now?" Driven entirely by Azure operations (`vm_create`, `vm_start`, `vm_deallocate`). Examples of legal transitions: `provisioning → running`, `running → stopping → stopped`, `stopped → starting → running`, `running → upgrading → running`. Any state can transition to `error` if the underlying Azure operation fails.
- **`allocation`** answers "is this tester reserved by a benchmark right now?" Driven entirely by orchestrator operations (`try_acquire`, `release`). Three values:
  - `idle` — available; the next benchmark can acquire it
  - `locked` — exclusively held by exactly one benchmark (`locked_by_config_id` is non-NULL)
  - `upgrading` — install.sh is re-running on the VM; new acquisitions are blocked

A tester whose VM is `running` may be either `idle` (available) or `locked` (busy). A tester whose VM is `stopped` is always `idle` (you cannot lock a powered-off tester). A tester whose VM is `upgrading` has `allocation='upgrading'` to match.

```
   power_state        allocation       human-readable label in UI
   ─────────────       ──────────       ────────────────────────────
   provisioning       idle             "Provisioning…"
   running            idle             "Idle"
   running            locked           "Running benchmark X"
   running            upgrading        "Updating…"
   stopping           idle             "Stopping…"
   stopped            idle             "Stopped"
   starting           idle             "Starting…"
   upgrading          upgrading        "Updating…"
   error              idle             "Error"
```

(Combinations not in this table — e.g. `power_state=stopped` with `allocation=locked` — are forbidden by the `lock_requires_running_vm` CHECK constraint.)

### Lock invariant — enforced in three places

The single authoritative answer to "is the tester busy?" is:

```
allocation = 'locked'   AND   locked_by_config_id IS NOT NULL
```

This invariant is protected by three independent layers:

1. **CHECK constraint** `lock_holder_implies_locked` makes the database reject any row where `allocation='locked'` but the holder is NULL, or where the holder is non-NULL but the allocation is something else. The two columns can never disagree at rest.
2. **`ON DELETE RESTRICT`** on `locked_by_config_id` prevents Postgres from clearing the holder when a benchmark_config row is deleted. The previous design used `ON DELETE SET NULL`, which had a hole: deleting a config could leave the tester in the forbidden state. Now you cannot delete a benchmark_config that holds a lock — you must release it first via the orchestrator's `release()` path, which is the only code path allowed to mutate `allocation` from `locked` back to `idle`.
3. **`release()` is the sole writer** of the `(allocation, locked_by_config_id)` pair on the way out of `locked`. No other function in the codebase touches these columns when transitioning out of `locked`. A unit test enforces this with a grep-style assertion against the source tree.

The orchestrator enters the locked state via `try_acquire` (atomic conditional UPDATE) and exits via `release`. There is no other path.

### Modifications to `benchmark_config`

```sql
ALTER TABLE benchmark_config
    -- Live FK to the tester. Nullable so a tester deletion doesn't block
    -- removal of historical benchmark rows. ON DELETE SET NULL clears the
    -- live link; the snapshot columns below preserve historical identity.
    ADD COLUMN tester_id          UUID REFERENCES project_tester(tester_id) ON DELETE SET NULL,

    -- Denormalized snapshot of the tester at launch time. Populated when the
    -- benchmark is created and never updated afterwards. This is what the UI
    -- displays for historical runs even after the tester row is gone.
    ADD COLUMN tester_name_snapshot      TEXT,
    ADD COLUMN tester_region_snapshot    TEXT,
    ADD COLUMN tester_cloud_snapshot     TEXT,
    ADD COLUMN tester_vm_size_snapshot   TEXT,
    ADD COLUMN tester_version_snapshot   TEXT,         -- installer_version at the moment of launch

    ADD COLUMN queued_at     TIMESTAMPTZ,
    ADD COLUMN current_phase TEXT,                     -- queued | starting | deploy | running | collect | done
    ADD COLUMN outcome       TEXT;                     -- success | partial | failure | cancelled (set when phase='done')

-- Application benchmarks REQUIRE either a live tester reference OR a snapshot.
-- The snapshot is mandatory at launch; tester_id may be cleared later via SET NULL,
-- but the snapshot proves the benchmark was launched against a real tester.
ALTER TABLE benchmark_config
    ADD CONSTRAINT app_configs_need_tester
    CHECK (
        benchmark_type != 'application'
        OR (tester_id IS NOT NULL OR tester_name_snapshot IS NOT NULL)
    );
```

### Authoritative orchestration state — `benchmark_config.status` only

`benchmark_config.status` is the **single source of truth for orchestration decisions**. Every dispatcher promotion, every drain check, every queue ordering, every cancel — all read and write `status`.

`current_phase` is **purely presentational**. It is read by the UI to drive the `<PhaseBar />` component and by audit logs to record what stage of work was happening. **No orchestration logic ever reads `current_phase`** — not the scheduler, not the dispatcher, not the recovery loop. If you find yourself writing a query that joins `status` and `current_phase` for a control flow decision, you are doing it wrong.

To enforce this rule, the SQL queries used by the schedulers and dispatcher are reviewed in the implementation plan and a code-level check (clippy lint or unit test) ensures `current_phase` never appears in a `WHERE` clause inside the `services/tester_*` modules.

The auto-shutdown drain check therefore reads `status` only:

```sql
AND NOT EXISTS (
    SELECT 1 FROM benchmark_config c
     WHERE c.tester_id = t.tester_id
       AND c.status IN ('queued','pending','running')
)
```

### Status state machine for `benchmark_config`

```
       (created via wizard)
              │
              ↓
          pending ──┐         (launched immediately, tester was idle)
              │     │
              │     └─→ running ──┬─→ completed
              │                   ├─→ completed_with_errors
              │                   ├─→ failed
              │                   └─→ cancelled
              │
              └─→ queued (tester was busy at launch time)
                    │
                    └─→ pending (promoted by dispatcher when tester freed)
                          ↓
                       running → ...
```

Terminal statuses: `completed`, `completed_with_errors`, `failed`, `cancelled`.

**Queue semantics:** when a benchmark is launched against a busy tester, the row gets `status='queued'`, `tester_id=<chosen>`, `queued_at=NOW()`. The dispatcher promotes the oldest queued benchmark to `pending` when the tester frees; the orchestrator then picks up `pending` and runs it. `current_phase` follows along for display purposes only.

### Tester deletion semantics

A tester can be deleted only when:
- `power_state IN ('stopped', 'idle', 'error')` AND `allocation = 'idle'`
- No benchmark_config row references this tester with `status IN ('queued','pending','running')`

The API endpoint enforces both checks before issuing the Azure resource delete. On successful delete:
- Azure resources (VM, NIC, public IP, NSG, OS disk) are destroyed
- The `project_tester` row is deleted
- All historical `benchmark_config` rows that referenced this tester have their `tester_id` cleared by `ON DELETE SET NULL`, but the `tester_*_snapshot` columns retain the historical identity
- The UI for historical benchmarks shows the snapshot (e.g. *"eastus-1 · azure/eastus · v0.24.2 · (deleted 2026-04-15)"*) so users can still understand what the run was against

This means tester deletion is non-blocking on history while preserving full provenance.

### Phase columns on other entities

```sql
ALTER TABLE probe_job    ADD COLUMN current_phase TEXT, ADD COLUMN outcome TEXT;
ALTER TABLE schedule_run ADD COLUMN current_phase TEXT, ADD COLUMN outcome TEXT;
```

The unified phase enum lives in `crates/networker-common/src/phase.rs` and is imported by every producer (orchestrator, agent, dashboard) and consumer (frontend `<PhaseBar />` component).

### Region → timezone map

`crates/networker-dashboard/src/services/azure_regions.rs`:

```rust
pub fn region_timezone(region: &str) -> chrono_tz::Tz {
    match region {
        "eastus" | "eastus2"               => chrono_tz::US::Eastern,
        "westus" | "westus2" | "westus3"   => chrono_tz::US::Pacific,
        "japaneast" | "japanwest"          => chrono_tz::Asia::Tokyo,
        "westeurope" | "northeurope"       => chrono_tz::Europe::Berlin,
        "uksouth"  | "ukwest"              => chrono_tz::Europe::London,
        "australiaeast" | "australiasoutheast" => chrono_tz::Australia::Sydney,
        "centralus" | "northcentralus" | "southcentralus" => chrono_tz::US::Central,
        _                                  => chrono_tz::UTC,
    }
}
```

Hardcoded; new regions added by extending the match. Defaults to UTC for unknown regions, which the user can override per-tester.

## API surface

All endpoints under `/api/projects/{pid}/testers/...`. Project membership is required; admin role required for create/delete/upgrade/start/stop/schedule changes; member role sufficient for view + pick-in-wizard.

### Listing & inspection

```
GET    /api/projects/{pid}/testers
GET    /api/projects/{pid}/testers/{tester_id}
GET    /api/projects/{pid}/testers/regions       — available regions for this project's cloud_account
GET    /api/projects/{pid}/testers/{tid}/queue   — running + queued benchmarks for this tester, ordered
```

### Lifecycle

```
POST   /api/projects/{pid}/testers
       Body: { name, cloud, region, vm_size?, auto_shutdown_local_hour?, auto_probe_enabled? }
       Returns immediately with tester in power_state='provisioning'; background task does the actual install.

POST   /api/projects/{pid}/testers/{tid}/start    — power_state stopped → starting → running
                                                  — refuses 409 if power_state is already running, starting, or in any other transient state
POST   /api/projects/{pid}/testers/{tid}/stop     — power_state running → stopping → stopped
                                                  — refuses 409 if allocation='locked' (cannot stop a tester running a benchmark)
                                                  — refuses 409 if any benchmark is in queued/pending status for this tester
POST   /api/projects/{pid}/testers/{tid}/upgrade  — re-run install.sh on the existing VM
                                                  — transitions: power_state stays 'running', allocation idle → upgrading → idle
                                                  — refuses 409 if allocation != 'idle' (can't upgrade a busy tester)
                                                  — refuses 409 if any benchmark is queued, pending, or running for this tester
                                                  — body: { confirm: true } required to proceed
DELETE /api/projects/{pid}/testers/{tid}          — destroys Azure resources and clears the row
                                                  — refuses 409 if allocation != 'idle'
                                                  — refuses 409 if power_state IN ('provisioning','starting','stopping','upgrading')
                                                  — refuses 409 if any benchmark queued/pending/running for this tester
                                                  — refusing on transient states prevents orphaned Azure resources
                                                  — historical benchmark rows referencing this tester have tester_id cleared
                                                    by ON DELETE SET NULL; the tester_*_snapshot columns preserve identity
```

### Schedule control

```
PATCH  /api/projects/{pid}/testers/{tid}/schedule
       Body: { auto_shutdown_enabled?, auto_shutdown_local_hour? }

POST   /api/projects/{pid}/testers/{tid}/postpone
       Body: { until?: ISO8601 } | { add_hours?: int } | { skip_tonight?: true }
```

### Recovery actions

```
POST   /api/projects/{pid}/testers/{tid}/probe
       Queries Azure for the current VM state and resyncs the tester row.
       Allowed in any power_state. Returns the updated tester row.
       Transitions on completion:
         Azure VM Running       → power_state='running', allocation='idle'  (if harness sanity check passes)
                                → power_state='error',   status_message='harness missing or broken'
         Azure VM Deallocated   → power_state='stopped', allocation='idle'
         Azure VM Starting      → power_state='starting', recheck after 15s
         Azure VM Stopping      → power_state='stopping', recheck after 15s
         Azure VM Unknown       → power_state='error',   status_message='unknown Azure state'
       Refuses 409 if allocation='locked' or allocation='upgrading' (don't probe a busy tester).

POST   /api/projects/{pid}/testers/{tid}/force-stop
       Forces the tester row to power_state='stopped', allocation='idle' WITHOUT
       running any Azure operation. Used by the "Force to stopped" recovery action
       on the error-state drawer when the user has externally verified the VM is
       deallocated.
       Body: { confirm: true, reason: "..." }
       Allowed in any power_state EXCEPT 'running' AND 'allocation=locked'
       (cannot force-stop a tester that is actively running a benchmark).
       The next benchmark launch will exercise vm_start + SSH wait + harness check,
       providing real validation that the tester is healthy.
       Audit-logged with the actor user and the reason.
```

### Auxiliary endpoints

```
GET    /api/projects/{pid}/testers/{tid}/cost_estimate
       Returns rough monthly cost based on VM size + auto-shutdown schedule:
       {
         "vm_size": "Standard_D2s_v3",
         "compute_hourly_usd": 0.096,
         "disk_monthly_usd": 4.80,
         "estimated_monthly_usd_with_schedule": 28.50,    // assuming nightly shutdown
         "estimated_monthly_usd_always_on": 73.92,        // for comparison
         "shutdown_hours_per_day": 9                       // 11 PM → 8 AM in tester's timezone
       }
       Surfaced in the tester detail drawer to give users an at-a-glance cost figure.

POST   /api/projects/{pid}/testers/refresh-latest-version
       Manually triggers the latest-version refresh loop (Section 6d) without
       waiting for the next 6h tick. Returns the new latest known installer
       version. Admin role required.
```

### Rate limiting

To prevent accidental Azure quota exhaustion or runaway costs, the create endpoint is rate-limited:

- **Tester creation**: max **5 testers per project per hour**, max **10 per project total** (global cap; user can delete and recreate)
- Limits are enforced at the API layer with a clear 429 error message: *"You've created N testers in the past hour. The limit is 5 per hour to prevent quota exhaustion. Try again in M minutes."*
- The 10-tester global cap is configurable per project by platform admins via the project settings page (not exposed in MVP — defaults to 10).

### Audit logging on dangerous actions

Every action that mutates a tester's state writes a `service_log` entry tagged `tester_action` with `actor_user_id`, `tester_id`, `project_id`, action name, and outcome:

- `tester_created`, `tester_deleted`
- `tester_start_requested`, `tester_stop_requested`, `tester_upgrade_requested`
- `tester_schedule_changed`, `tester_postponed`
- `tester_lock_acquired`, `tester_lock_released`, `tester_lock_force_released`
- `tester_auto_shutdown_completed`, `tester_auto_shutdown_deferred`, `tester_shutdown_stuck`

These appear on the existing Logs page and in the per-tester drawer's activity feed.

### Authorization

Uses existing `require_project_role` middleware. Cloud credentials always come from the project's `cloud_account` row, never from environment variables — even for platform admins. Switching projects swaps the entire visible tester list, including for admins.

### Asynchronous operations

`POST /testers` and `POST /testers/{id}/upgrade` return immediately with the tester in a transient state (`provisioning` or `upgrading`). The background task progresses the row through states; clients poll the GET endpoint or subscribe over WebSocket for live updates.

## Orchestrator integration

The orchestrator's `execute_testbed_application` function is rewritten end-to-end. The legacy ephemeral-VM path is **deleted entirely** — no flag, no fallback, no escape hatch. Application benchmarks always run against a referenced tester.

### New flow

```
execute_testbed_application(config, ...)
  ↓
  // 1. Resolve which tester to run on (FK on benchmark_config)
  tester = lookup_tester(config.tester_id)
  if tester is None: fail benchmark with "no tester selected"

  // 2. Acquire exclusive lock (or queue)
  match tester_state::try_acquire(tester.id, config.id):
    Acquired                    → proceed
    NeedsStart                  → start_tester(tester); retry acquire
    Transient(power_state)      → enqueue, return Queued status
                                  // power_state was provisioning|starting|stopping
    Upgrading                   → enqueue, return Queued status
    AlreadyLockedBy(other_cfg)  → enqueue, return Queued status
    Errored                     → fail benchmark with "tester is in error state, fix it first"

  // 3. Ensure tester is awake and reachable
  // (only runs if try_acquire returned NeedsStart and the retry succeeded —
  //  by this point allocation='locked' and power_state='running')
  refresh_bench_token(tester)               ← per-run token, scp'd to VM

  // 4. For each (proxy, language): deploy + run + collect
  for each (proxy, lang) {
    deploy_benchmark_proxy(tester.ip, proxy)    ← install.sh --benchmark-proxy-swap
    deploy_benchmark_server(tester.ip, lang)    ← install.sh --benchmark-server
    run_chrome_benchmark(tester.ip, ...)
  }

  // 5. Release lock — tester transitions allocation locked → idle
  //    power_state stays 'running'; the auto-shutdown scheduler will deallocate
  //    later if appropriate. The orchestrator NEVER destroys the tester.
  tester_state::release(tester.id)

  // 6. Notify dispatcher so any queued benchmark starts immediately
  notify_queue_dispatcher(tester.id)
```

Wall-clock comparison for a 6-language application benchmark:
- **Old (ephemeral VM)**: ~45 minutes (~30 min wasted on Chrome install)
- **New (warm idle tester)**: ~10 minutes (just the benchmark runs)
- **New (stopped tester)**: ~11 minutes (+1 min Azure VM start)

### Lock acquisition (race-free)

```rust
pub async fn try_acquire(client: &PgClient, tester_id: Uuid, config_id: Uuid) -> Result<AcquireOutcome> {
    // The conditional UPDATE is the only operation that can flip allocation
    // from 'idle' to 'locked'. Postgres serialises it, so two concurrent
    // acquire calls can never both succeed. The follow-up SELECT below is
    // intentionally a separate statement (not RETURNING more columns) for
    // clarity: if the UPDATE failed, we want to know *why* and the diagnostic
    // logic is easier to read in two steps. The cost is one extra round-trip
    // on the contention path, which is rare and not performance-critical.
    let row = client.query_opt(
        r#"
        UPDATE project_tester
           SET allocation          = 'locked',
               locked_by_config_id = $2,
               last_used_at        = NOW(),
               updated_at          = NOW()
         WHERE tester_id    = $1
           AND power_state  = 'running'
           AND allocation   = 'idle'
           AND locked_by_config_id IS NULL
         RETURNING tester_id
        "#,
        &[&tester_id, &config_id],
    ).await?;

    if row.is_some() { return Ok(AcquireOutcome::Acquired); }

    // Didn't get it — figure out why
    let cur = client.query_one(
        "SELECT power_state, allocation, locked_by_config_id FROM project_tester WHERE tester_id = $1",
        &[&tester_id],
    ).await?;
    let power: String = cur.get(0);
    let alloc: String = cur.get(1);
    let locker: Option<Uuid> = cur.get(2);

    match (power.as_str(), alloc.as_str()) {
        ("stopped", _)              => Ok(AcquireOutcome::NeedsStart),
        ("starting", _)             => Ok(AcquireOutcome::Transient(power)),
        ("stopping" | "provisioning", _) => Ok(AcquireOutcome::Transient(power)),
        ("running",  "locked")      => Ok(AcquireOutcome::AlreadyLockedBy(locker.unwrap())),
        ("running",  "upgrading")   => Ok(AcquireOutcome::Upgrading),
        ("error", _)                => Ok(AcquireOutcome::Errored),
        _                           => Ok(AcquireOutcome::NotIdle(format!("{}/{}", power, alloc))),
    }
}
```

The `release()` function is the **only** writer that clears `(allocation, locked_by_config_id)` together:

```rust
pub async fn release(client: &PgClient, tester_id: Uuid, config_id: Uuid) -> Result<()> {
    // Defensive: only release if WE hold it. Prevents accidental release
    // by buggy callers that don't actually own the lock.
    client.execute(
        r#"
        UPDATE project_tester
           SET allocation          = 'idle',
               locked_by_config_id = NULL,
               updated_at          = NOW()
         WHERE tester_id           = $1
           AND locked_by_config_id = $2
        "#,
        &[&tester_id, &config_id],
    ).await?;
    Ok(())
}
```

This is the only function in the entire codebase that flips `allocation` from `locked` back to `idle`. Enforced by:
- A unit test that greps the source tree for any other `SET allocation = 'idle'` outside this file
- The `lock_holder_implies_locked` CHECK constraint catches any code path that tries to clear the holder without also clearing the allocation

### SSH wait timeout — configurable

`ensure_tester_running` waits for the tester's SSH port to accept connections after `az vm start` returns. Default wait is 60 seconds (Azure typically takes 30–60s for boot + cloud-init). For slower regions or older VM SKUs, this can be tuned:

- **Per-tester override**: a future column on `project_tester` (`ssh_wait_timeout_secs`) lets users bump it for specific testers known to be slow. NOT in MVP — defaults to 60 for everyone in v1.
- **Global default**: env var `DASHBOARD_TESTER_SSH_WAIT_SECS` overrides the hardcoded 60s default for all testers.
- **Hard ceiling**: 300 seconds (5 minutes). If SSH still isn't ready after that, the tester is marked `error` with a clear message — something is wrong beyond a slow boot.

### Bench token lifetime

Each benchmark run generates a fresh `BENCH_API_TOKEN` that's scp'd to the tester before the benchmark starts and used by the language servers to authenticate harness traffic. Token semantics:

- **Generated per benchmark run**, not per tester. A new token for each lock acquisition.
- **Stored in Azure Key Vault** (when configured) for audit trail and revocation, scoped by `(project_id, tester_id, config_id)`.
- **Lifetime**: valid only for the duration of one benchmark run. Cleaned up from the tester's filesystem on lock release.
- **Rotation policy**: automatic — there is no "long-lived token" mode. Even back-to-back benchmarks on the same tester get fresh tokens.

This prevents any single token from accumulating audit history across many runs and ensures that a benchmark abandoned mid-run leaves no usable credentials behind.

### Lock release safety

Three layers prevent stuck locks:
1. Happy path: explicit `release()` at end of `execute_testbed_application`.
2. Error path: release wrapped in a finally-style block that runs even on `Err` or panic.
3. Crash recovery: a startup task scans for testers in `running` status whose `locked_by_config_id` references a benchmark already in a terminal state, and force-releases them.

## Wizard UX

### Step order

```
1. Name & description           (unchanged)
2. Benchmark type               (application | network | mixed)
3. Topology                     (Loopback | Split | …)
4. Cloud + region               (azure / eastus / westus / …)
5. Tester                       ← NEW REQUIRED STEP
6. Languages                    (only if benchmark_type=application)
7. Methodology                  (warmup, measured runs, modes)
8. Review + Launch
```

### Tester picker (step 5)

After cloud + region are selected in step 4, step 5 calls `GET /testers?cloud=...&region=...` and renders one of three states:

- **State A — testers exist**: list of radio options, each showing status, version, queue depth (if any), next shutdown. Selecting a busy tester shows an inline "will queue at position #N" warning with an ETA computed from `avg_benchmark_duration_seconds * (queue_depth + 1) - elapsed_in_running_benchmark`. Selecting a stopped tester shows "will auto-start when benchmark begins". An "Update available" badge appears for testers running an old installer version, but selecting one does NOT force the upgrade — that's an explicit action on the management page. A "+ Create another tester in {region}" button opens the create modal **pre-filled with the cloud and region from step 4** so the user only fills in name + size.
- **State B — no testers in this region**: explanatory text + a single "Create {region} tester" button. Wizard cannot advance until a tester is created. Same modal pre-fill as above.
- **State C — tester being created**: progress checklist with phase-by-phase updates pushed via WebSocket. When status flips to `idle`, the wizard auto-selects the new tester and enables Next.

### Create-tester modal (shared)

```
┌──────────────────────────────────────────┐
│ Create tester                            │
│                                          │
│ Cloud      [ Azure         ▾ ]           │
│ Region     [ eastus        ▾ ]           │
│ Name       [ eastus-2          ]         │
│ VM size    [ Standard_D2s_v3 ▾ ]         │
│                                          │
│ Auto-shutdown                            │
│ ☑ Enabled                                │
│ Stop daily at  [ 11 PM ▾ ] region time   │
│                                          │
│ Recovery                                 │
│ ☐ Auto-probe Azure for stuck states      │
│   ⓘ Off by default. When on, the         │
│     dashboard queries Azure to resync if │
│     this tester gets stuck after a       │
│     restart. Off keeps stuck testers     │
│     visible as 'error' for manual review.│
│                                          │
│              [Cancel]   [Create]         │
└──────────────────────────────────────────┘
```

### Review screen + queue position display

When the chosen tester is busy, the Review screen shows a queue panel:

```
Tester:    eastus-2 (Azure / eastus)
           Standard_D2s_v3 · v0.24.2 · running
           ⚠ Queue position on launch: #3
              ├─ Now running: "Multi-Lang test #5"  · started 8m ago · ~12m remaining
              ├─ #1 queued:   "Java perf check"     · queued 5m ago
              ├─ #2 queued:   "Nightly regression"  · queued 2m ago
              └─ #3 yours:    will start in ~28 min
```

Position and ETA are pushed live over WebSocket — no polling.

## Tester Management page — region-grouped accordion (Layout C)

New top-level project page at `/projects/{pid}/testers`. Layout chosen by the user from three mockups (A: dense table, B: card grid, **C: region-grouped accordion ✓**).

### Page layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  AletheBench Demo / Testers                              [filter] [+ Create] │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ ▾ azure / eastus     3 testers · 1 running · 2 in queue   [+ add]      │  │
│  │   ─────────────────────────────────────────────────────────────────────│  │
│  │   ● idle      eastus-1       v0.24.2 · D2s_v3 · idle, no queue   ›    │  │
│  │   ● running   eastus-2       v0.24.2 · running Multi-Lang #5     ›    │  │
│  │                              · 2 queued                                │  │
│  │   ● stopped   eastus-canary  v0.24.0 → v0.24.2 ★ update          ›    │  │
│  │                              · auto-shutdown disabled                  │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ ▾ azure / westus     1 tester · idle                       [+ add]    │  │
│  │   ─────────────────────────────────────────────────────────────────────│  │
│  │   ● idle      westus-1       v0.24.2 · idle, no queue            ›    │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ ▸ azure / japaneast  1 tester · starting                   [+ add]    │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Region groups are collapsible. Region header shows live count summary.
- Clicking a tester row opens a slide-in detail drawer from the right.
- Live updates via WebSocket subscription — every status change pushes to all connected clients in the project.
- **Empty state** (no testers in project): friendly explanation that a tester is required + a one-click **"Create first tester in eastus (recommended)"** button. The button opens the Create modal pre-filled with `cloud=azure, region=eastus, name=eastus-1, vm_size=Standard_D2s_v3, auto_shutdown_enabled=true, auto_shutdown_local_hour=23`. The user just clicks Create. eastus is the recommended default because it's where most Azure quota is typically available; if the user's `cloud_account` is configured for a different default region, the button uses that instead.
- **Page header** has a `[ Refresh latest version ]` button (admin-only) that triggers an immediate refresh of the latest known installer version (Section 6d).

### Detail drawer

Sections inside the drawer:
- **Status** (with locked-by info if running)
- **Identity** (cloud, region, VM size, VM name, public IP, created by/at)
- **Version** (installed version, latest known, "Update available" badge with version diff. When badge is shown, includes a "View changelog →" link to the relevant GitHub release page (`https://github.com/irlm/networker-tester/releases/tag/v0.24.2`) so the user can read what changed before clicking Update.)
- **Cost estimate** (rough monthly figure from `GET /cost_estimate`, with a comparison to "always on" so the user sees the savings from auto-shutdown)
- **Usage** (total benchmarks run, average duration, last used)
- **Auto-shutdown** (schedule, next shutdown time, deferral count if > 0, [Edit schedule] [Postpone…] [Disable] buttons)
- **Recovery** (auto-probe toggle + Run probe now)
- **Queue** (running benchmark, queued benchmarks)
- **Recent activity** (audit log, last 10 entries with link to full history)
- **Danger zone** (Stop tester, Delete tester — refused while running, queued, or in any transient state)

When the tester is in `power_state='error'`, the drawer's status section shows a prominent **"Fix tester first"** panel with four actions, ordered from least to most invasive:

- **Run probe** — calls `POST /testers/{tid}/probe` to query Azure for the actual VM state. If Azure reports the VM is healthy and the probe finds the harness intact, the tester transitions back to `power_state='running', allocation='idle'`. If the probe finds the VM in some other state, the row is resynced to match (e.g. `Deallocated → power_state='stopped'`).
- **Reinstall tester** — calls `POST /testers/{tid}/upgrade` which re-runs the full install.sh on the existing VM. This is the supported recovery for "the harness got into a bad state but the VM is fine". On success the tester ends in `power_state='running', allocation='idle'`.
- **Force to stopped** — calls `POST /testers/{tid}/force-stop`. Marks the tester as `power_state='stopped', allocation='idle'` without running any Azure operation. The user is expected to use this only when they have externally verified that the VM is actually deallocated. Subsequent benchmark launches will go through the normal `ensure_tester_running` path, which exercises `vm_start` + SSH wait + harness sanity check, providing real validation.
- **Delete tester** — destroys the broken tester so a fresh one can be created.

There is **no "Mark as healthy" action**. A direct `error → idle` transition without verification is unsafe — it could reintroduce a tester whose VM is unreachable, whose harness is missing, or whose public IP has changed. Every recovery path either runs an actual probe (safe), re-runs install.sh (safe), or forces the tester back to a state that requires the next `ensure_tester_running` to validate it (safe). The user can never bypass validation; they can only choose how heavyweight the validation is.

## Background schedulers

Four small tokio tasks running inside the dashboard process. All crash-tolerant; all write to `service_log` for audit.

### Auto-shutdown loop

Runs every minute. Selects testers eligible for shutdown:

```sql
SELECT t.*
  FROM project_tester t
 WHERE t.auto_shutdown_enabled = TRUE
   AND t.next_shutdown_at < NOW()
   AND t.power_state = 'running'
   AND t.allocation  = 'idle'
   AND NOT EXISTS (
       SELECT 1 FROM benchmark_config c
        WHERE c.tester_id = t.tester_id
          AND c.status IN ('queued','pending','running')
   );
```

The drain check reads `benchmark_config.status` only — `current_phase` is purely presentational and never consulted by orchestration. The `(power_state='running' AND allocation='idle')` filter is the precondition for safe deallocation: the VM is currently up but no benchmark holds it.

**Hard rule:** a tester is shut down only if it is completely drained — VM running, allocation idle, AND zero benchmarks in any non-terminal status. If the queue is non-empty, the shutdown is deferred 5 minutes, `shutdown_deferral_count` is incremented, and re-tried. The shutdown task re-validates the drain check inside the per-tester task to catch races (a benchmark queued between SELECT and shutdown). Two layers ensure no surprise shutdowns.

**Shutdown action.** When a tester is eligible to shut down, the orchestrator runs `az vm deallocate` (NOT `az vm stop`). Deallocation releases the compute resources and stops billing for CPU/RAM immediately; the disk is preserved at standard storage rates (~$5/month for a Standard_D2s_v3's OS disk). This is the only "stop" mode that achieves the cost-savings intent of the schedule — `az vm stop` keeps the VM allocated and billing continues at the full hourly rate.

**Deferral cap.** After 3 consecutive deferrals, the scheduler:
- Writes a high-severity `service_log` entry tagged `tester_shutdown_stuck` with the tester_id, project_id, and the names of the benchmarks holding it open
- Surfaces a yellow banner on the Tester Management page row: *"Auto-shutdown deferred 3 times tonight — tester has been busy continuously since 11 PM"*
- Continues to defer (does NOT force-cancel any benchmark) — the rule "only stop when drained" is non-negotiable

The deferral count resets to 0 after a successful shutdown, so the next night starts fresh.

After successful deallocation, `next_shutdown_at` is recomputed for the same hour-of-day in the region's local timezone, 24h forward. `shutdown_deferral_count` is reset to 0.

### Queue dispatcher

Two trigger paths, both calling `promote_next(tester_id)`:

1. **Event-driven (primary)**: orchestrator calls dispatch directly when releasing a lock at the end of a benchmark.
2. **Periodic safety net**: every 30s, sweep loop scans for `idle` testers with queued benchmarks and dispatches them. Catches edge cases where the event path was missed (orchestrator crashed mid-cleanup, tester just woke up from auto-stop, deferred shutdown completed and queue still has waiters).

The dispatch function uses `FOR UPDATE SKIP LOCKED` to safely promote the oldest queued benchmark even under concurrent dispatcher races.

### Crash recovery on startup

The recovery task waits **5 minutes after dashboard startup** before scanning, giving in-flight background tasks (vm_start, install.sh, deallocate) time to resume cleanly after a process restart. Without this grace period, a fast restart during a long-running install would prematurely mark the tester as stuck.

After the grace period:

1. **Force-release stuck locks**: testers with `allocation='locked'` whose `locked_by_config_id` points at a benchmark in a terminal status get their lock released via `release()` and the dispatcher fired for the tester. The release path is the only code allowed to clear `(allocation, locked_by_config_id)`, even from the recovery loop.
2. **Handle stuck transient power states**: testers whose `power_state IN ('starting','stopping','upgrading','provisioning')` AND `updated_at` is older than 30 min:
   - If `auto_probe_enabled = TRUE` (opt-in): query Azure for the actual VM state and resync `power_state` to match (`Running → power_state='running'`, `Deallocated → power_state='stopped'`, etc.). If the probe itself fails, set `power_state='error'` with reason.
   - If `auto_probe_enabled = FALSE` (default): set `power_state='error'` with a message instructing manual recovery via the drawer's "Fix tester first" panel.

The 5-minute grace + 30-min staleness threshold means: a normal restart in the middle of an install never triggers recovery, but a restart that left a tester abandoned for half an hour does.

### Latest-version refresh

Every 6 hours, refreshes the "latest installer version" used by the management page's update badge. Resolution order:
1. **GitHub releases poll** (preferred) — `GET https://api.github.com/repos/irlm/networker-tester/releases/latest`. Rate-limit friendly. Used as the canonical source when reachable.
2. **Dashboard's own `CARGO_PKG_VERSION`** (fallback) — always available, used as the floor when GitHub is unreachable.

The higher of the two is the value the badge compares against. GitHub is preferred because the dashboard binary may be slightly behind the latest released installer (e.g., between a tag push and the dashboard redeploy), and the user should know about new installer versions even before their dashboard catches up.

Cached in app state; no per-request external calls.

**Manual refresh button.** Admins can trigger an immediate refresh from the Tester Management page header (`[ Refresh latest version ]`) without waiting for the next 6h tick. Calls a new `POST /api/projects/{pid}/testers/refresh-latest-version` endpoint that runs the same resolution logic and returns the new value. Useful immediately after publishing a new release.

## Push-based updates (no polling)

A new WebSocket subscription channel is added to the existing `/ws` infrastructure. All progress and queue updates are pushed; clients re-render only on event. Manual refresh button (on management page and progress page) calls a REST endpoint as the only active fetch path after initial subscribe.

### Subscribe / update messages

```
Client → Server: Subscribe
{
  "type": "subscribe_tester_queue",
  "project_id": "us049x9psa20iw",          ← REQUIRED, validated against membership
  "tester_ids": ["uuid-1", "uuid-2"]       ← all must belong to that project
}

Server → Client: Initial snapshot (sent immediately after subscribe)
{
  "type": "tester_queue_snapshot",
  "project_id": "...",
  "tester_id": "...",
  "seq": 12345,                            ← monotonic per-tester sequence number
  "running": { ... } | null,
  "queued": [ ... ]
}

Server → Client: Update (pushed when an event occurs)
{
  "type": "tester_queue_update",
  "project_id": "...",                     ← echoed back, never assumed
  "tester_id": "...",
  "seq": 12346,                            ← strictly greater than the snapshot's seq
  "trigger": "benchmark_finished" | "benchmark_queued" | "benchmark_cancelled" | "benchmark_moved",
  "running": { ... },
  "queued": [ ... ]
}

Server → Client: Phase update (used for ALL benchmarks, probe jobs, scheduled runs)
{
  "type": "phase_update",
  "project_id": "...",
  "entity_type": "benchmark_config" | "probe_job" | "schedule_run",
  "entity_id": "...",
  "seq": 89,                               ← per-entity monotonic sequence
  "phase": "deploy",
  "outcome": null,
  "message": "Installing nodejs server",
  "applied_stages": ["queued","starting","deploy","running","collect","done"]
}
```

### Sequence numbers and reconnect handling

Every push message includes a monotonic `seq` field, scoped per (`tester_id`) for queue messages and per (`entity_id`) for phase messages. The server keeps an **in-memory** counter that increments on each emitted update. Clients track the highest `seq` they've seen for each entity.

**Important durability caveat:** `seq` is monotonic **only within a single dashboard process lifetime**. It is not persisted to the database and resets to 1 on every dashboard restart. This is an intentional simplification — the snapshot mechanism on reconnect provides the actual correctness guarantee, and `seq` is only used as a fast in-process duplicate-detection hint. Clients must NOT treat a lower `seq` after a reconnect as an error — it just means the dashboard restarted, and the snapshot they receive on reconnect is the authoritative state. If we ever need cross-restart durability for `seq`, the path forward is a per-entity counter column on the database row (out of scope for this spec).

**Reconnect flow:**
1. Client reconnects after a network blip OR a dashboard restart and re-subscribes with its `tester_ids` list.
2. Server sends a fresh `tester_queue_snapshot` containing the *current authoritative state* and the *current process-lifetime `seq`*.
3. Client **always trusts the snapshot** as the new source of truth. Its locally-stored `seq` is reset to the snapshot's value. The UI re-renders from snapshot.
4. Subsequent updates resume normally with `seq` continuing from the snapshot.

**Out-of-order or duplicate handling within a single connection:** if a message arrives with `seq <= last_seen_seq` *during the same connection*, the client drops it as a duplicate. After a reconnect, this rule does not apply — the snapshot resets the baseline.

### Subscription limits and rate limiting

- **Max concurrent subscriptions per project**: 50 (sensible default; enough for several browsers/tabs across team members but bounds memory). Server tracks count per `(project_id, user_id)` and rejects new subscriptions with a clear error when exceeded.
- **Subscribe message rate limit**: 10 subscribe/unsubscribe messages per WebSocket per minute. Prevents thrash from buggy clients that re-subscribe in a tight loop.
- Both limits are configurable via env vars (`DASHBOARD_MAX_SUBS_PER_PROJECT`, `DASHBOARD_MAX_SUB_MSGS_PER_MIN`) with the defaults above.

### Project enforcement at every layer

Project isolation is non-negotiable and enforced at six independent layers as defence in depth:

1. **Subscribe message MUST include project_id**; server validates membership in `project_member` before accepting. There is no global-subscribe API shape.
2. **Server-side hub indexes by `(project_id, tester_id)`**, so cross-project leakage is structurally impossible.
3. **Every push message echoes `project_id`**; client-side filtering drops mismatches.
4. **Every REST endpoint** is under `/api/projects/{pid}/...` and uses the existing `require_project_role` middleware. 404 for cross-project access, even for platform admins.
5. **Every SQL query for testers includes a `project_id` filter** — code review check + a unit test enforces this.
6. **Cloud credentials always come from `cloud_account` for the tester's project**, never from env vars or global config. Switching projects swaps the entire visible tester list.

## Unified phase progress model

A single `Phase` enum lives in `crates/networker-common/src/phase.rs` and is used by every entity in the system that has progress to report:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Queued,    // waiting on resource (tester slot, agent connection, schedule trigger)
    Starting,  // resource is waking up (tester deallocate→start, agent reconnect)
    Deploy,    // installing per-run pieces (endpoints, language servers, proxies)
    Running,   // actual measurement happening
    Collect,   // results being parsed + persisted
    Done,      // terminal — colour determined by separate `outcome` field
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,        // green
    PartialSuccess, // yellow ("done with errors")
    Failure,        // red
    Cancelled,      // grey
}
```

### Mapping per surface

| Surface | Phase mapping |
|---|---|
| Application benchmark | All 6 stages, `starting` skipped if tester was already running |
| Network benchmark (HTTP/UDP/DNS) | `queued → running → collect → done`. `starting` and `deploy` skipped. |
| Mixed benchmark | All 6 stages — uses application path for the tester, runs network probes from the same tester |
| Scheduled / recurring runs | Each instance shows its own phase bar; schedule overview lists active instances with current phase |
| Agent-driven probe jobs | `queued → starting → running → collect → done`. `deploy` skipped. |
| CLI runs reporting to dashboard | `running → collect → done`. `queued` and `starting` skipped — CLI is already executing when it first contacts the dashboard. |

### Single React component

`<PhaseBar phase={phase} outcome={outcome} appliedStages={[…]} />` — used in every progress display:
- Benchmark progress page (replaces existing 6-stage bar)
- Tester management page (drawer status)
- Benchmarks list page (inline pill per row)
- Schedules page
- Agents page (per-active-job pill)
- Dashboard home recent-activity widget

Visual rules: exactly one stage active at a time, earlier stages "complete" (cyan filled), later stages "pending" (empty). On terminal state, all stages become complete and the final stage's label colour shows the outcome.

## Cleanup & deployment plan

### Reset migration

The schema changes are split across **two distinct artifacts** with different lifecycles, to prevent the destructive reset from being mistaken for a normal upgrade step:

#### 1. The schema migration (regular, repeatable, idempotent)

A normal application migration that any environment runs at startup. This is the only schema-changing artifact in the regular migration chain. It is non-destructive and safe to run against any database, populated or empty.

```sql
-- crates/networker-dashboard/migrations/NNNN_persistent_testers_schema.sql

-- New tester table with the full schema from the data model section
CREATE TABLE IF NOT EXISTS project_tester ( /* full schema */ );

CREATE INDEX IF NOT EXISTS idx_project_tester_project   ON project_tester(project_id);
CREATE INDEX IF NOT EXISTS idx_project_tester_power     ON project_tester(power_state) WHERE power_state IN ('running','stopped');
CREATE INDEX IF NOT EXISTS idx_project_tester_alloc     ON project_tester(allocation)  WHERE allocation IN ('idle','locked');
CREATE INDEX IF NOT EXISTS idx_project_tester_shutdown  ON project_tester(next_shutdown_at) WHERE auto_shutdown_enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_project_tester_last_used ON project_tester(project_id, last_used_at DESC NULLS LAST);

-- benchmark_config additions
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_id              UUID REFERENCES project_tester(tester_id) ON DELETE SET NULL;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_name_snapshot   TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_region_snapshot TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_cloud_snapshot  TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_vm_size_snapshot TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_version_snapshot TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS queued_at     TIMESTAMPTZ;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS current_phase TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS outcome       TEXT;

-- Add the constraint only if it doesn't exist (idempotent)
DO $$ BEGIN
    ALTER TABLE benchmark_config ADD CONSTRAINT app_configs_need_tester
        CHECK (
            benchmark_type != 'application'
            OR (tester_id IS NOT NULL OR tester_name_snapshot IS NOT NULL)
        );
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- Phase/outcome columns on other progress-tracking tables
ALTER TABLE probe_job    ADD COLUMN IF NOT EXISTS current_phase TEXT;
ALTER TABLE probe_job    ADD COLUMN IF NOT EXISTS outcome       TEXT;
ALTER TABLE schedule_run ADD COLUMN IF NOT EXISTS current_phase TEXT;
ALTER TABLE schedule_run ADD COLUMN IF NOT EXISTS outcome       TEXT;
```

This migration runs every time the dashboard starts. It is idempotent (safe to re-run) and non-destructive (no `TRUNCATE`, no `DROP`).

#### 2. The destructive bootstrap reset (one-time, environment-guarded, NOT in the migration chain)

A separate SQL script that lives outside the regular migration directory and is **never** run automatically. It exists only because this is a pre-prod environment that needs a clean slate, and operating on it requires an explicit environment variable plus a CLI command.

```sql
-- crates/networker-dashboard/bootstrap/reset-pre-prod.sql
--
-- ⚠️  DESTRUCTIVE — wipes ALL data in ALL tables.
-- This is a one-time pre-production reset, NOT a regular migration.
-- It will NEVER run automatically. To execute:
--
--    DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true \
--    cargo run -p networker-dashboard --bin reset-pre-prod
--
-- The binary refuses to run unless the environment variable is set,
-- and refuses to run against any database whose `project` table contains
-- more than 0 rows tagged 'production' in cloud_account.
--
-- Outside the regular migration chain. Reviewers must understand this
-- before approving any PR that touches it.

BEGIN;

-- Drop any tables from prior experiments that no longer exist in the new schema
DROP TABLE IF EXISTS old_ephemeral_vm_state CASCADE;
DROP TABLE IF EXISTS old_benchmark_vm_pool CASCADE;

-- Wipe everything in the active schema
TRUNCATE TABLE benchmark_config, benchmark_testbed, benchmark_run, probe_job,
               schedule_run, schedule, project_member, cloud_account,
               share_link, deploy, app_user, project, project_tester
   RESTART IDENTITY CASCADE;

COMMIT;

-- VACUUM outside the transaction
VACUUM FULL benchmark_config, benchmark_testbed, benchmark_run, probe_job,
            schedule_run, schedule, project_member, cloud_account,
            share_link, deploy, app_user, project, project_tester;
ANALYZE;
```

A small Rust binary `reset-pre-prod` lives at `crates/networker-dashboard/src/bin/reset_pre_prod.rs` and gates the script behind `DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true`. The binary refuses to run if the environment variable is unset OR if any project in the `project` table has a non-test cloud_account configured (best-effort production guard).

After the reset, the dashboard's existing `bootstrap_admin_user_from_env` path creates the first admin from `DASHBOARD_ADMIN_PASSWORD` so the first HTTP request can log in.

**The two artifacts are kept separate** so that the destructive script can never accidentally end up in the regular migration chain via a copy-paste error or a sloppy refactor.

### Single PR

One submission containing backend + migration + frontend + smoke tests. Two logical halves for review readability but they merge together as one release tag.

**Backend half:**
1. SQL migration file
2. New Rust types in `crates/networker-dashboard/src/db/`
3. All `services/*.rs` files (state, dispatcher, scheduler, recovery, install, regions, version_refresh)
4. New `api/testers.rs` REST handlers
5. WebSocket subscription handler additions
6. Phase enum in `crates/networker-common`
7. Background tasks spawned in `main.rs`
8. Unit tests for state machine + dispatcher race conditions
9. Integration test: create tester via API, run benchmark, verify lock acquire/release
10. Orchestrator rewrite: `execute_testbed_application` becomes a single tester-based path
11. Removal of ephemeral VM provisioning code from the orchestrator
12. CHANGELOG entry, version bump

**Frontend half:**
1. New `dashboard/src/pages/TestersPage.tsx` (region-grouped accordion)
2. New components: `TesterRegionGroup`, `TesterRow`, `TesterDetailDrawer`, `CreateTesterModal`
3. New `PhaseBar` component (replaces existing benchmark progress bar)
4. Wizard rewrite: new Tester step, removal of "create new VM" path
5. WebSocket subscription hooks (`useTesterSubscription`, `usePhaseSubscription`)
6. Navigation entry in `ProjectNav`
7. CHANGELOG entry, version bump

### CLI smoke test suite

The `networker-tester` crate is the original product. It must keep working as a standalone CLI even though we're rewriting the orchestrator. A new script `tests/cli_smoke.sh` exists and runs eight scenarios in order:

1. **Local HTTP/2 probe** against a local `networker-endpoint` instance
2. **HTTP/3 probe** with QUIC
3. **DNS probe**
4. **All modes against a real public endpoint** (`https://www.cloudflare.com`)
5. **Database persistence** with the SQLite feature
6. **Cargo workspace builds** including `--no-default-features` and `--all-features`, plus `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace --lib`
7. **CLI reports to dashboard** — verifies the new `Phase` enum is wired correctly when the CLI talks to a locally-running dashboard. The CLI sends `phase=running` on first contact, then `phase=collect` when results are being persisted, then `phase=done` with the appropriate outcome
8. **End-to-end persistent-tester flow** — creates a tester via API on a local mock cloud (or against a real Azure VM if `SMOKE_TEST_AZURE=1` is set), launches an application benchmark against it, verifies the lock acquire/release cycle and that the tester ends in `power_state='running', allocation='idle'`

The script returns a non-zero exit code on any failure and prints a summary of which scenarios passed and failed. The implementation playbook (a separate artifact, not this design document) decides when and how often the script is run.

### End-to-end validation surfaces

The dashboard exposes the following user-facing surfaces that must be reachable and functional after deploy:

- **Tester Management page** — list, create, detail drawer with all sections, delete
- **Wizard Tester step** — list available testers, create inline, queue position display
- **Benchmark Progress page** — `<PhaseBar />` updates live via WebSocket, shows queue position when applicable
- **Project navigation** — Testers entry visible to project members
- **Logs page** — `tester_action`-tagged events queryable

The implementation playbook owns the test plan for exercising these surfaces post-deploy; this design document does not prescribe how that testing happens.

## Edge cases

| Edge case | Handling |
|---|---|
| Tester in `error` state when benchmark launches | Wizard refuses to launch with "Tester is in error state — fix it on the management page first" linking to the detail drawer's Fix tester first panel |
| User clicks Upgrade while benchmarks are queued | API returns 409 Conflict; UI shows "Cannot upgrade while benchmarks are queued. Cancel or wait for them to finish first." |
| Dashboard restart in the middle of a long-running install | 5-minute grace period on crash recovery prevents premature error marking. After grace, only testers with `updated_at` older than 30 min are touched. |
| Very long-running benchmark (>24 h) | Tester stays alive past its scheduled shutdown via the deferral logic. After 3 deferrals (>15 min), a high-severity log entry surfaces this so the user notices, but the benchmark is never force-cancelled. Documented expected behaviour. |
| Concurrent benchmark launches from multiple tabs | FIFO queue ordered by `queued_at`. Two tabs hitting "Launch" at the same instant get sequential queue positions; no UI race needed. |
| Azure VM deallocated externally (CLI / portal) | Default behaviour: tester stays in its previous status until a benchmark tries to use it, at which point `ensure_tester_running` calls `az vm start` and the resync happens implicitly. If `auto_probe_enabled = TRUE`, the recovery loop catches it sooner. If the user wants instant detection, they enable auto-probe. |
| Multiple concurrent dispatcher calls for the same tester | `FOR UPDATE SKIP LOCKED` ensures only one wins; the others find no work and exit cleanly. |
| Subscription drops mid-update | Sequence numbers + reconnect snapshot ensure the client never displays a stale or out-of-order view. Worst case is a brief gap (≤1 second) on reconnect; the next snapshot resyncs everything. |
| User deletes a tester that has historical benchmark results | The benchmark_config rows keep their `tester_id` reference until the tester is deleted; on delete, the FK `ON DELETE RESTRICT` blocks the operation if any benchmark is queued/pending/running, and `SET NULL` on the column would orphan completed-run history. We use `RESTRICT` for in-flight work and accept that historical rows will show "deleted tester" in the UI. |
| networker-tester CLI compatibility | Smoke test gate verifies the CLI continues to work. The Phase enum is added to networker-common, which the CLI also depends on, so the CLI must be updated to send phases — that change is part of the same PR. |
| Cloud account credentials rotated mid-benchmark | The orchestrator caches the cloud_account row at the start of the benchmark, so a mid-run rotation doesn't break the in-flight run. The next run will use the new credentials. |
| User has 10 testers (the cap) and tries to create an 11th | API returns 429 with the message "You've reached the maximum of 10 testers per project. Delete an existing tester or contact a platform admin to raise the limit." |
| Auto-shutdown deferral cap exceeded for many nights in a row | Each night's deferral count resets at successful shutdown. If a tester is *literally never* idle for weeks, the deferral count stays >3 and the warning log fires every night. The user is expected to either provision a second tester to reduce queue load or accept that this tester is essentially always-on. |

## Open questions

None. All design decisions made and confirmed by the project owner during the brainstorming session on 2026-04-10.
