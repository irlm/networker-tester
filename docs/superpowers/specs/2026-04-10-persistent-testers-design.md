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

- `project_tester` table — the new domain object, with status state machine, version tracking, schedule, lock holder
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

    -- lifecycle state
    status              TEXT NOT NULL DEFAULT 'provisioning',
                        -- provisioning | idle | starting | running | stopping | stopped | upgrading | error
    status_message      TEXT,
    locked_by_config_id UUID REFERENCES benchmark_config(config_id) ON DELETE SET NULL,

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

CREATE INDEX idx_project_tester_project  ON project_tester(project_id);
CREATE INDEX idx_project_tester_status   ON project_tester(status) WHERE status IN ('idle','running','stopped');
CREATE INDEX idx_project_tester_shutdown ON project_tester(next_shutdown_at) WHERE auto_shutdown_enabled = TRUE;
CREATE INDEX idx_project_tester_last_used ON project_tester(project_id, last_used_at DESC NULLS LAST);
```

**Status state machine:**

```
   provisioning ──┬─→ idle ──┬─→ starting ──→ running ──→ idle (release)
                  │          │      │             │
                  │          │      └────────→ error ←─┘
                  │          │
                  │          ├─→ stopping ──→ stopped ──→ starting
                  │          │      │           │
                  │          │      └────────→ error
                  │          │
                  │          └─→ upgrading ──→ idle
                  │                  │
                  │                  └─→ error
                  │
                  └─→ error
```

**Lock semantics — single source of truth.** "Tester in use" is defined precisely as:

```
status = 'running' AND locked_by_config_id IS NOT NULL
```

This pair is the **single authoritative answer** to "is the tester busy?" — every API response, UI label, scheduler check, dispatcher decision, and audit log entry uses this exact condition. The orchestrator transitions in via `try_acquire` (atomic conditional UPDATE — race-free) and out via `release` (which clears both fields together). No code path may touch `status='running'` without also setting/clearing `locked_by_config_id`.

### Modifications to `benchmark_config`

```sql
ALTER TABLE benchmark_config
    ADD COLUMN tester_id     UUID REFERENCES project_tester(tester_id) ON DELETE RESTRICT,
    ADD COLUMN queued_at     TIMESTAMPTZ,
    ADD COLUMN current_phase TEXT,                     -- queued | starting | deploy | running | collect | done
    ADD COLUMN outcome       TEXT;                     -- success | partial | failure | cancelled (only when phase='done')

-- Application benchmarks REQUIRE a tester (SQL-level enforcement)
ALTER TABLE benchmark_config
    ADD CONSTRAINT app_configs_need_tester
    CHECK (benchmark_type != 'application' OR tester_id IS NOT NULL);
```

**Queue semantics:** when a benchmark is launched against a busy tester, the row gets `status='queued'`, `tester_id=<chosen>`, `queued_at=NOW()`. The dispatcher promotes the oldest queued benchmark when the tester frees.

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
       Returns immediately with tester in 'provisioning' status; background task does the actual install.

POST   /api/projects/{pid}/testers/{tid}/start    — stopped → starting → idle (auto-start before benchmark or manual)
POST   /api/projects/{pid}/testers/{tid}/stop     — idle → stopping → stopped
                                                  — refuses 409 if status='running'
                                                  — refuses 409 if any benchmark is in queued/pending status for this tester
POST   /api/projects/{pid}/testers/{tid}/upgrade  — re-run install.sh on the existing VM; idle → upgrading → idle
                                                  — refuses 409 if status != 'idle'
                                                  — refuses 409 if any benchmark is queued, pending, or running for this tester
                                                  — body: { confirm: true } required to proceed
DELETE /api/projects/{pid}/testers/{tid}          — destroys Azure resources
                                                  — refuses 409 if status='running' or 'provisioning' or 'starting' or 'stopping' or 'upgrading'
                                                  — refuses 409 if any benchmark queued/pending/running for this tester
                                                  — refusing on transient states prevents orphaned Azure resources
```

### Schedule control

```
PATCH  /api/projects/{pid}/testers/{tid}/schedule
       Body: { auto_shutdown_enabled?, auto_shutdown_local_hour? }

POST   /api/projects/{pid}/testers/{tid}/postpone
       Body: { until?: ISO8601 } | { add_hours?: int } | { skip_tonight?: true }
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
    Acquired               → proceed
    AlreadyLockedByOther   → enqueue, return Queued status
    NotIdle(curr_status)   →
      if curr_status == 'stopped':            start_tester(tester); retry acquire
      if curr_status == 'starting'|'upgrading': enqueue, return Queued
      if curr_status == 'error':              fail benchmark

  // 3. Ensure tester is awake and reachable
  ensure_tester_running(tester)             ← deallocate→start, wait for SSH (~60s), only if needed
  refresh_bench_token(tester)               ← per-run token, scp'd to VM

  // 4. For each (proxy, language): deploy + run + collect
  for each (proxy, lang) {
    deploy_benchmark_proxy(tester.ip, proxy)    ← install.sh --benchmark-proxy-swap
    deploy_benchmark_server(tester.ip, lang)    ← install.sh --benchmark-server
    run_chrome_benchmark(tester.ip, ...)
  }

  // 5. Release lock — tester returns to 'idle', NEVER destroyed
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
    // The conditional UPDATE is the only operation that can flip the tester
    // to 'running' — Postgres serialises it, so two concurrent acquire calls
    // can never both succeed. The follow-up SELECT below is intentionally a
    // separate statement (not RETURNING more columns) for clarity: if the
    // UPDATE failed, we want to know *why* and the diagnostic logic is easier
    // to read in two steps. The cost is one extra round-trip on the
    // contention path, which is rare and not performance-critical.
    let row = client.query_opt(
        r#"
        UPDATE project_tester
           SET status = 'running',
               locked_by_config_id = $2,
               last_used_at = NOW(),
               updated_at = NOW()
         WHERE tester_id = $1
           AND status = 'idle'
           AND locked_by_config_id IS NULL
         RETURNING tester_id
        "#,
        &[&tester_id, &config_id],
    ).await?;

    if row.is_some() { return Ok(AcquireOutcome::Acquired); }

    // Didn't get it — figure out why
    let cur = client.query_one(
        "SELECT status, locked_by_config_id FROM project_tester WHERE tester_id = $1",
        &[&tester_id],
    ).await?;
    let status: String = cur.get(0);
    let locker: Option<Uuid> = cur.get(1);
    match status.as_str() {
        "stopped"            => Ok(AcquireOutcome::NeedsStart),
        "running" if locker.is_some() => Ok(AcquireOutcome::AlreadyLockedBy(locker.unwrap())),
        other                => Ok(AcquireOutcome::NotIdle(other.to_string())),
    }
}
```

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

When the tester is in `error` state, the drawer's status section shows a prominent **"Fix tester first"** panel with three actions:
- **Run probe now** — calls `POST /testers/{tid}/probe` to query Azure for the current state and resync
- **Mark as healthy** — manual override; transitions error → idle after the user has verified externally
- **Delete tester** — destroys the broken tester so a fresh one can be created

This makes the recovery path obvious without forcing users to dig through admin tools.

## Background schedulers

Four small tokio tasks running inside the dashboard process. All crash-tolerant; all write to `service_log` for audit.

### Auto-shutdown loop

Runs every minute. Selects testers eligible for shutdown:

```sql
SELECT t.*
  FROM project_tester t
 WHERE t.auto_shutdown_enabled = TRUE
   AND t.next_shutdown_at < NOW()
   AND t.status = 'idle'
   AND NOT EXISTS (
       SELECT 1 FROM benchmark_config c
        WHERE c.tester_id = t.tester_id
          AND (
              c.status IN ('queued','pending','running')
              OR c.current_phase IN ('queued','starting','deploy','running','collect')
          )
   );
```

The drain check considers **both** `benchmark_config.status` AND `current_phase`. A benchmark mid-deploy on the tester (`status='running'`, `current_phase='deploy'`) blocks shutdown just as effectively as one in the queue. The two checks are redundant in the happy path but defend against any race where one column lags the other.

**Hard rule:** a tester is shut down only if it is completely drained — `status='idle'` AND zero benchmarks in any non-terminal state. If the queue is non-empty, the shutdown is deferred 5 minutes, `shutdown_deferral_count` is incremented, and re-tried. The shutdown task re-validates the drain check inside the per-tester task to catch races (a benchmark queued between SELECT and shutdown). Two layers ensure no surprise shutdowns.

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

1. **Force-release stuck locks**: testers in `running` status whose `locked_by_config_id` points at a benchmark in a terminal state get their lock released and the dispatcher fired for the tester.
2. **Handle stuck transient states**: testers in `starting`/`stopping`/`upgrading` whose `updated_at` is older than 30 min:
   - If `auto_probe_enabled = TRUE` (opt-in): query Azure for the actual VM state and resync (`Running → idle`, `Deallocated → stopped`, etc.). If the probe itself fails, mark as `error` with reason.
   - If `auto_probe_enabled = FALSE` (default): mark as `error` with a message instructing manual recovery.

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

Every push message includes a monotonic `seq` field, scoped per (`tester_id`) for queue messages and per (`entity_id`) for phase messages. The server keeps an in-memory counter that increments on each emitted update. Clients track the highest `seq` they've seen for each entity.

**Reconnect flow:**
1. Client reconnects after a network blip and re-subscribes with its `tester_ids` list.
2. Server sends a fresh `tester_queue_snapshot` with the *current* `seq`.
3. Client compares against its locally-stored last-seen `seq`. If the snapshot's `seq` is higher, the client knows it missed events between the disconnect and now and the snapshot is the source of truth; the UI re-renders from snapshot. If equal, no events were missed.
4. Subsequent updates resume normally with `seq` continuing from the snapshot.

**Out-of-order or duplicate handling:** if a message arrives with `seq <= last_seen_seq`, the client drops it. This is unusual under normal operation but possible during reconnect race windows.

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

The deploy is treated as a fresh install. The migration is destructive and bootstraps from zero:

```sql
-- crates/networker-dashboard/migrations/NNNN_persistent_testers_reset.sql
BEGIN;

-- 1. Drop any tables from prior experiments that no longer exist in the new schema
DROP TABLE IF EXISTS old_ephemeral_vm_state CASCADE;
DROP TABLE IF EXISTS old_benchmark_vm_pool CASCADE;
-- (Add other obsolete table names here if discovered during implementation)

-- 2. Wipe everything in the active schema — pre-prod reset, no data to preserve
TRUNCATE TABLE benchmark_config, benchmark_testbed, benchmark_run, probe_job,
               schedule_run, schedule, project_member, cloud_account,
               share_link, deploy, app_user, project
   RESTART IDENTITY CASCADE;

-- 3. New tester table with the full schema from the data model section
CREATE TABLE project_tester ( /* full schema */ );
CREATE INDEX idx_project_tester_project    ON project_tester(project_id);
CREATE INDEX idx_project_tester_status     ON project_tester(status) WHERE status IN ('idle','running','stopped');
CREATE INDEX idx_project_tester_shutdown   ON project_tester(next_shutdown_at) WHERE auto_shutdown_enabled = TRUE;
CREATE INDEX idx_project_tester_last_used  ON project_tester(project_id, last_used_at DESC NULLS LAST);

-- 4. New columns on benchmark_config
ALTER TABLE benchmark_config
    ADD COLUMN tester_id     UUID REFERENCES project_tester(tester_id) ON DELETE RESTRICT,
    ADD COLUMN queued_at     TIMESTAMPTZ,
    ADD COLUMN current_phase TEXT,
    ADD COLUMN outcome       TEXT;

ALTER TABLE benchmark_config ADD CONSTRAINT app_configs_need_tester
    CHECK (benchmark_type != 'application' OR tester_id IS NOT NULL);

ALTER TABLE probe_job    ADD COLUMN current_phase TEXT, ADD COLUMN outcome TEXT;
ALTER TABLE schedule_run ADD COLUMN current_phase TEXT, ADD COLUMN outcome TEXT;

COMMIT;

-- 5. Reclaim disk space and refresh statistics after the bulk wipe.
-- Done outside the transaction because VACUUM cannot run inside one.
VACUUM FULL benchmark_config, benchmark_testbed, benchmark_run, probe_job,
            schedule_run, schedule, project_member, cloud_account,
            share_link, deploy, app_user, project, project_tester;
ANALYZE;
```

The dashboard's existing `bootstrap_admin_user_from_env` path creates the first admin from `DASHBOARD_ADMIN_PASSWORD` after the wipe so the first request after deploy can log in.

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

### CLI smoke test — hard pre-merge gate

The `networker-tester` crate is the original product. It must keep working as a standalone CLI even though we're rewriting the orchestrator. A new script `tests/cli_smoke.sh` runs eight scenarios in order:

1. **Local HTTP/2 probe** against a local `networker-endpoint` instance
2. **HTTP/3 probe** with QUIC
3. **DNS probe**
4. **All modes against a real public endpoint** (`https://www.cloudflare.com`)
5. **Database persistence** with the SQLite feature
6. **Cargo workspace builds** including `--no-default-features` and `--all-features`, plus `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace --lib`
7. **CLI reports to dashboard** — verifies the new `Phase` enum is wired correctly when the CLI talks to a locally-running dashboard. The CLI must send `phase=running` on first contact, then `phase=collect` when results are being persisted, then `phase=done` with the appropriate outcome
8. **End-to-end persistent-tester flow** — creates a tester via API on a local mock cloud (or against a real Azure VM if `SMOKE_TEST_AZURE=1` is set), launches an application benchmark against it, verifies the lock acquire/release cycle and that the tester ends in `idle` status. This is the integration test for the new orchestrator path; it must pass before the PR is mergeable.

These run as the **first action** when implementation starts (baseline check) and after every meaningful commit. Any failure reverts the offending commit before continuing.

### Post-deploy validation via Chrome MCP

After the release ships and the database is reset, the user logs in with the `DASHBOARD_ADMIN_PASSWORD` temp password and changes it. Then I (Claude) drive the new flows via Chrome MCP to verify end-to-end:

1. Create a project + cloud_account configuration
2. Create the first tester in eastus (the long path — ~10 min for Chrome install)
3. Verify the management page shows `provisioning → idle` transitions live
4. Launch a multi-language application benchmark against the new tester
5. Verify all 6 languages produce results
6. Verify the tester returns to `idle` and is NOT destroyed
7. Launch a second benchmark — verify it's much faster (no Chrome reinstall)
8. Queue a third benchmark while the second is running — verify queue position display + auto-promote
9. Verify auto-shutdown defers when a benchmark is queued
10. Verify the unified phase bar updates live across the wizard, progress page, and management page

If any step fails, I fix forward; we don't ship a half-working tester pool.

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
