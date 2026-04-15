# TestConfig unification — migration plan

**Status:** draft for approval · no code changes until sign-off.

**Driving decision:** unify `Job` (Tests) and `BenchmarkConfig` (Benchmarks) under one canonical `TestConfig` primitive with an optional `methodology` field. UI consolidates to a single **Run** surface; endpoint-type is a filter, not a product.

**Inventory:** `.critique/refactor/01-inventory.md` — 15 tables, 50+ Rust types, 18+ endpoints, 19 frontend call sites, CLI, WS protocol.

---

## Target architecture

### Canonical primitive

```rust
struct TestConfig {
    id: Uuid,
    project_id: Uuid,
    name: String,
    endpoint: EndpointRef,        // ← the axis that actually varies
    workload: Workload,           // modes, concurrency, duration, payload sizes
    methodology: Option<Methodology>, // None = simple test; Some = benchmark mode
    schedule: Option<Schedule>,
    created_at, updated_at, created_by,
}

enum EndpointRef {
    Network { host: String, port: Option<u16> },   // raw network (dns/tcp/tls/http*/udp)
    Proxy   { proxy_id: Uuid },                    // nginx/IIS in front of an API
    Runtime { runtime_id: Uuid },                  // language/framework stack
}

struct Workload {
    modes: Vec<Mode>,
    runs: u32,
    concurrency: u32,
    timeout_ms: u32,
    payload_sizes: Vec<u32>,
    capture_mode: CaptureMode,
}

struct Methodology {
    warmup_runs: u32,
    measured_runs: u32,
    cooldown_ms: u32,
    target_error_pct: f32,
    outlier_policy: OutlierPolicy,
    quality_gates: QualityGates,
    baseline_ref: Option<Uuid>,   // compare against prior TestRun
}
```

### Data-model mapping

| Current | New | Notes |
|---|---|---|
| `job` table | `test_config` (methodology = NULL) | simple test definitions |
| `benchmark_config` table | `test_config` (methodology = NOT NULL) | benchmark definitions |
| `test_run` table | unified `test_run` | adds nullable `artifact_id` for benchmark-methodology runs |
| `benchmark_artifact` + samples/summaries | **kept** — linked from `test_run.artifact_id` | only exists when methodology was set |
| `schedule` (polymorphic) | `schedule` (references `test_config_id` only) | no more JobConfig-or-benchmark-id split |
| `RequestAttempt` + phase result tables | **kept** — linked from `test_run_id` | simple-test output, unchanged |

### Output contract

- Every `test_run` gets the same flat result row (success/failure counts, started/completed, attempts).
- If `methodology IS NOT NULL`, it **additionally** produces a linked `benchmark_artifact` with phases, samples, summaries, quality gates.
- Comparison / baselining logic moves from "benchmark-only" to any `test_run` that has an artifact (or that meets a minimum sample count).

---

## Migration strategy — expand / contract, not big-bang

Production is live (alethedash.com). The refactor runs in 7 phases over expand-contract boundaries. **Each phase ships independently, passes CI, and keeps both backends working until the final cutover.**

### Phase 0 — safety net (1 day)

- Freeze breaking schema changes on `main`.
- Tag `pre-unification` so rollback point is explicit.
- Turn on a feature flag `UNIFIED_TEST_CONFIG` (default off).
- Snapshot production DB.

**Acceptance:** flag exists, toggling it still returns current behavior.

### Phase 1 — new schema, additive only (2–3 days)

- Add `test_config` table (superset of `job` + `benchmark_config`).
- Add `test_run.test_config_id UUID NULL` and `test_run.artifact_id UUID NULL`.
- Add new enums (`EndpointKind`, `MethodologyKind`).
- **No drops, no renames.** Old tables untouched.

**Acceptance:** migrations apply cleanly up + down; CI green on empty new tables.

### Phase 2 — dual-write path (3–5 days)

- Every write that currently creates a `Job` also writes a mirror `TestConfig` (methodology = NULL).
- Every write that creates a `BenchmarkConfig` also writes a mirror `TestConfig` (methodology = NOT NULL).
- Every `TestRun` write sets `test_config_id` pointing to the mirror.
- Keep old writes as source of truth; new rows are derived.

**Acceptance:** dual-writes land; mirror row count matches source; reconciliation job passes.

### Phase 3 — backfill (1 day, long-running)

- Batch-migrate all historical `job` + `benchmark_config` rows into `test_config`.
- Patch `test_run.test_config_id` retroactively.
- Verify integrity: `COUNT(test_config) == COUNT(job) + COUNT(benchmark_config)`.

**Acceptance:** backfill complete, zero orphan runs, FK integrity check passes.

### Phase 4 — new read API (3–5 days)

- Add `/api/v2/test-configs`, `/api/v2/test-runs` endpoints reading from new tables.
- **v1 endpoints stay untouched** — dual-serve.
- Unified WebSocket message schema for live run progress (no more job-vs-benchmark polymorphism).

**Acceptance:** v2 endpoints return data equivalent to v1 for every query; contract tests pass on both.

### Phase 5 — frontend cut (5–7 days)

- Dashboard switches to v2 endpoints behind the `UNIFIED_TEST_CONFIG` flag.
- New unified **Run** page replaces Tests + Benchmarks tabs (per IA recommendation).
- Endpoint-type segmented control on creation form.
- Old pages stay behind flag-off route for rollback.
- Schedules page is updated to the unified config (no more polymorphic `JobConfig | benchmark_config_id` schedule type).

**Acceptance:** flag=on shows unified UI; flag=off shows old UI; both work end-to-end.

### Phase 6 — cutover (1 day)

- Flip `UNIFIED_TEST_CONFIG` to default on in prod.
- Monitor error rates for 48h.
- CLI (`networker-tester` binary) switches to v2 config shape; old config shape silently maps with a deprecation log.

**Acceptance:** zero v1 writes in last 24h; no elevated error rate.

### Phase 7 — contract (2–3 days)

- Delete v1 endpoints.
- Drop the old `job`, `benchmark_config` tables.
- Drop the `Schedule` polymorphism.
- Delete mirror-write code from phase 2.
- Remove feature flag.

**Acceptance:** grep shows no references to `Job` / `BenchmarkConfig` types; CI green; frontend bundle size drops.

---

## Risk register

| Risk | Mitigation |
|---|---|
| **Production data loss during backfill** | Phase 0 snapshot; phase 3 runs in transaction with checksum verify; `--dry-run` flag first |
| **Dashboard-agent WS protocol break** | New protocol version bump; agents advertise supported versions; dashboard speaks both during transition |
| **CLI users' config files break** | v2 shape has a compat layer that accepts v1 for one release; deprecation log; installers push v2 shape in `v0.28.0` |
| **Schedules fire incorrectly during dual-write** | Schedules read canonically from old tables until phase 5; no schedule writes through new path until cutover |
| **Rollback needs** | Every phase is reversible via down-migration until phase 7; feature flag allows instant UI rollback until phase 6 |
| **Comparison/regression breaks** | Regression detector reads `test_run.artifact_id` post-unification; runs without an artifact are simply excluded (no behavior change) |
| **Long-running backfill blocks writes** | Run in batches of 10k, off-peak, with `SKIP LOCKED` |

---

## Timeline

| Phase | Effort | Elapsed (cumulative) |
|---|---|---|
| 0 Safety net | 1d | 1d |
| 1 New schema | 2–3d | 4d |
| 2 Dual-write | 3–5d | 9d |
| 3 Backfill | 1d | 10d |
| 4 v2 read API | 3–5d | 15d |
| 5 Frontend cut | 5–7d | 22d |
| 6 Cutover | 1d + 48h watch | 25d |
| 7 Contract | 2–3d | 28d |

**~4–6 weeks of focused work** for one engineer. Could compress if parallelized across backend (phases 1–4) and frontend (phase 5 prep).

---

## Out of scope (explicit)

- **Pre-ship UI polish** (/colorize, /onboard, /polish) — pause and re-do against the unified model in phase 5.
- **Sidebar + RunsPage normalize commits already on `design-system-rollout`** — keep; they're model-independent.
- **Endpoint registration system** (Network / Proxy / Runtime catalogs) — the new `EndpointRef` just references existing tables; no new registration UI yet.

---

## Version strategy

- Phases 1–4: minor bumps `v0.27.24 → v0.27.28` (additive, no user-visible breaking changes).
- Phase 5: `v0.28.0-beta` behind flag.
- Phase 6: `v0.28.0` GA.
- Phase 7: `v0.28.1` (cleanup).

Per CLAUDE.md, every PR bumps `Cargo.toml` workspace version + `CHANGELOG.md` + `INSTALLER_VERSION` in both install.sh and install.ps1.

---

## Open questions for user

1. **WS protocol negotiation** — are any self-hosted customers on old agent versions? (If yes, compat window needs widening.)
2. **CLI users in the wild** — do we know anyone using `networker-tester` binary directly with config files? (Affects phase 6 compat-layer lifetime.)
3. **Feature flag scope** — per-project, per-user, or global? (I'd recommend global + env-var override, simpler to reason about.)
4. **Schedule cutover** — do any production schedules exist today that span both jobs and benchmarks? (Query first before phase 5.)

---

## Decision needed

**Approve this plan as-is → I start Phase 0 on a new branch `backend-unification`.**

**Want adjustments →** tell me what to change and I'll revise.

**Want to defer →** I can park this, finish the `/normalize` + `/colorize` + `/onboard` + `/polish` rollout on the current codebase, and come back to the refactor. Downside: we polish a UI we're about to restructure.
