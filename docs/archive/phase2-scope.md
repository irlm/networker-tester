# Phase 2 Build-Out Scope — C# Control Plane

> **Historical — migration completed 2026-07; see [`architecture.md`](../architecture.md).**
> This was the forward-looking M0–M6 build-out plan; the milestones shipped and the cutover is done.

**Goal:** bring `src/Networker.ControlPlane` to full parity with the Rust
`networker-dashboard` (~41.6k LOC) behind the *same* wire contract, then flip
the frontend + agents over and **delete the Rust dashboard crate**. The Rust
probe engine (`networker-tester`) stays.

**Definition of done:** every REST endpoint + all three WebSocket hubs + all
background loops served by C#; React frontend and all agents pointed at the C#
control plane; a clean soak; `networker-dashboard` (plus `networker-common`,
`networker-log`) removed from the build. Rollback at any point = point config
back at the still-running Rust dashboard (the wire contract never changes).

---

## The surface to reach parity with

Mapped from the live Rust crate (read-only audit, 2026-07-14):

| Area | Count / shape | Notes |
|---|---|---|
| **REST endpoints** | **~150** across ~40 domains | v2 API (test-configs/runs/schedules/comparison-groups) is the modern core; v1 (agents/testers/deployments/cloud-*) still active |
| **WebSocket hubs** | **3** | `/ws/agent` (agent protocol), `/ws/dashboard` (browser event bus w/ replay+seq), `/ws/testers` (project-scoped queue, rate-limited) |
| **Background loops** | **6** | agent-reaper, auto-shutdown, version-refresh, scheduler (+6 sub-routines), provisioning-orchestrator, cloud-orphan-reaper |
| **DB tables (EF model)** | **~40** | PoC scaffolds 6 (Project, TestConfig, TestRun, ProjectTester, Agent, CloudAccount) |
| **Auth** | JWT (HS256, 24h) + global roles (Admin/Operator/Viewer) + project roles + per-request DB role/status sync + `must_change_password` + SSO (Microsoft/Google/OIDC) + AEAD credential encryption w/ key rotation | |
| **Provisioning** | Azure/AWS/GCP via `az`/`aws`/`gcloud` + `install.sh --deploy` shell-out; `Pending→Network` endpoint rewrite; deploy-runner log streaming | |

**PoC coverage today:** 3 read endpoints (`/api/health`, `/testers`, `/test-runs`),
6 EF entities, an agent hub with 1 method (`ReportResult`) + `Heartbeat`, a stub
browser hub. I.e. the skeleton is proven; the body is Phase 2.

---

## Cross-cutting foundations (Milestone 0 — must precede feature slices)

These are the load-bearing decisions. Get them wrong and every later slice pays.

1. **Complete the EF model (all ~40 tables).** Re-scaffold database-first from
   the live schema, lock the model, then switch to **model-first migrations**
   with a *baseline* migration that exactly matches the current Rust-managed
   schema (so C# and Rust agree on the DB during the shared-DB cutover period).
2. **JWT interchangeability.** Same secret (`DASHBOARD_JWT_SECRET`) + same
   algorithm (HS256) + same claim names (`sub`, `email`, `role`,
   `is_platform_admin`, `exp`, `iat`) so a token minted by *either* side
   validates on *both* during cutover. `AddAuthentication().AddJwtBearer(...)`.
3. **Credential-encryption byte-compatibility.** ⚠️ Shared DB means C# must
   **decrypt what Rust encrypted** in `cloud_account`. Port the Rust
   `crypto::encrypt/decrypt` AEAD scheme *exactly* (cipher, nonce layout, key
   derivation) and honor the key-rotation (`credential_key` + `_old`) and the
   `~/.config/networker/credential.key` fallback. Verified with a round-trip
   test against a row the Rust side wrote.
4. **Auth middleware + RBAC.** `ClaimsPrincipal`→`AuthUser`; a per-request
   middleware that re-reads role/status from `dash_user` (Rust does this every
   request — not idiomatic in .NET, needs a short-TTL cache); a
   `ProjectMember`/`ProjectOperator`/`ProjectAdmin` authorization policy + handler
   that resolves `{projectId}` → membership; user-status gates (pending/disabled/
   denied) + `must_change_password` gate; admin bootstrap from env.
5. **SignalR skeleton + auth handshake.** Three hubs with connection-time auth
   (JWT for browser/tester, API-key for agent), groups keyed per-project /
   per-agent / per-(project,tester).
6. **Config + observability.** Mirror env vars (DB URLs, JWT/credential keys,
   admin bootstrap) with `ValidateOnStart`; OpenTelemetry traces/metrics (serves
   the standing perf-observability requirement); log sink compatible with the
   `perf_log` / service-log tables.

---

## Milestones (dependency- and risk-ordered; each independently shippable)

Each milestone runs C# on a separate port against the shared DB, validated by
diffing responses against the Rust dashboard, then wired into the frontend/agents
slice-by-slice. Rollback per slice = config flip.

### M1 — Read-only parity (safe, high value, zero writes)
All GET/list endpoints for the hot frontend paths: projects list/detail,
dashboard summary, testers list/detail/queue/cost/regions, test-runs
list/detail/artifact/attempts/compare, test-configs list/get, agents list/detail,
deployments list/detail, cloud status/inventory, tls-profiles, url-tests,
vm-history, logs, perf-log GET, leaderboard (public), version, modes, zones.
→ C# can serve **read traffic** while Rust still owns writes. Parity = response diff.

### M2 — Realtime / SignalR parity
- **Browser event bus** (`/ws/dashboard`): the full `DashboardEvent` set
  (JobUpdate, AttemptResult, JobComplete, AgentStatus, DeployLog/Complete,
  BenchmarkUpdate/Regression). ⚠️ **Highest realtime risk:** reimplement the
  **replay ring buffer + monotonic `seq` + `?since=` reconnect** — SignalR has no
  built-in replay.
- **Tester queue hub** (`/ws/testers`): project-scoped subscribe/snapshot/update/
  phase, per-connection subscription state, rate limits, slow-subscriber ejection.
- **Agent hub** (`/ws/agent`): the full bidirectional protocol (in: Heartbeat,
  RunStarted/Progress/Finished, AttemptEvent, Error, CommandLog/Result; out:
  Welcome, AssignRun, CancelRun, Command, Cancel, HeartbeatPing, Shutdown).
  Extends the PoC's minimal hub. → cut over **one** agent to validate.

### M3 — Write path: the core test lifecycle (the product loop)
test-configs CRUD + **launch**, test-runs cancel/compare, schedules CRUD +
trigger, comparison-groups. The **dispatcher** (`dispatch_or_provision`,
queued-run redispatch, assign-to-agent over SignalR). Background services:
scheduler (schedules→runs), stale-job watchdog, queued redispatcher,
agent-reaper. → benchmarking works **end-to-end in C#**.

### M4 — Provisioning + VM lifecycle (highest external-integration risk)
testers CRUD + start/stop/upgrade/probe/force-stop/postpone/schedule/cost;
deployments CRUD + start/stop/check/update; cloud-accounts + cloud-connections
CRUD + validate (uses the M0 crypto). Deploy-runner (stream `install.sh` output →
SignalR), provisioning-orchestrator (`Pending→Network`), auto-shutdown loop,
cloud-orphan-reaper, version-refresh.
- **Decision — shell-out vs SDK:** ship parity first by **reusing `install.sh` +
  `az`/`aws`/`gcloud` shell-out** (fast, low-risk, identical behavior), then
  migrate to **`Azure.ResourceManager` / `AWSSDK.EC2` / `Google.Cloud.Compute`**
  behind an `IComputeProvisioner` as a *follow-on* (big reliability/testability
  win the analysis called for, but a significant lift — don't gate cutover on it).
- **Recommended refactor:** store the resolved endpoint on `test_run` instead of
  mutating `test_config` mid-flight (shrinks the riskiest mutation surface).

### M5 — Admin, orgs, access-control, SSO
users/roles, projects CRUD, members/invites (+ bulk import + accept flows),
command-approvals (+ SSE `/events/approval`), visibility-rules, share-links (+
public resolve), admin (metrics/workspaces/system-config/smoke-test), SSO +
SSO-admin (Microsoft Entra/Google/OIDC), bench-tokens, benchmark-catalog,
tester-precheck, agent-commands (dispatch + SSE stream), update endpoints,
pending-projects. Workspace-inactivity lifecycle loop (90d warn→120d suspend→365d
delete).

### M6 — Cutover + decommission
Shadow/parity validation across the whole surface → flip frontend + **all** agents
to C# → soak → **delete `networker-dashboard`, `networker-common`, `networker-log`**
and the Rust agent. (Endpoint crate → Phase 3.) Rollback = config flip back.

---

## Top risks (the things that actually shape the plan)

| # | Risk | Why | Mitigation |
|---|---|---|---|
| 1 | **Shared-DB dual-writer** | During cutover both Rust and C# touch the same tables; two writers on one concern = corruption | Cut over **one concern at a time**; exactly one writer per table-group at any moment; read-only C# (M1) is safe to run in parallel indefinitely |
| 2 | **Browser event replay/seq** | SignalR has no replay; the frontend relies on `?since=` gap recovery | Reimplement ring buffer + monotonic seq in the hub (M2); port the exact dedup contract the React client expects |
| 3 | **Credential encryption compat** | C# must read secrets Rust wrote, or cloud ops break | Port the AEAD scheme byte-for-byte (M0) + round-trip test against a Rust-written row |
| 4 | **Per-request DB role/status sync** | Rust re-checks the DB every request; .NET trusts the JWT | Custom middleware + short-TTL cache (M0) |
| 5 | **Provisioning state machine** | `Pending→Network` config mutation mid-flight is subtle | Store resolved endpoint on `test_run` (M4) |
| 6 | **Cloud CLI → SDK** | SDK is the reliability win but a big lift | Shell-out first for parity; SDK migration is a separate, later effort |

**Kill criteria (from the migration plan, still binding):** if M3 (the core loop)
slips past ~2× estimate with no working end-to-end cutover, freeze C#, keep Rust
in prod, reassess. Any measurable fidelity regression in a moved component → roll
that component back to Rust. If velocity doesn't actually improve on the next 5
bugs, the premise fails.

---

## Rough sizing (solo)

Honest range — the 150-endpoint surface is wide but many are simple CRUD; the cost
is concentrated in M0 (foundations), M2 (realtime replay), and M4 (provisioning).

| Milestone | Rough effort | Ships |
|---|---|---|
| M0 Foundations | 2–3 wk | model + auth + crypto + hub skeleton |
| M1 Read parity | 1–2 wk | C# serves frontend reads |
| M2 Realtime | 2–3 wk | live updates + agent protocol |
| M3 Core loop | 2–3 wk | benchmarking end-to-end in C# |
| M4 Provisioning | 3–4 wk (shell-out) | VM lifecycle in C# |
| M5 Admin/SSO/ACL | 2–3 wk | full feature parity |
| M6 Cutover | 1–2 wk | delete the Rust dashboard |
| **Total** | **~13–20 wk** | (SDK migration is additional, post-cutover) |

This is larger than the original ~6–10wk Phase-2 estimate because that predated
the full endpoint/hub/loop audit above. The strangler structure means it **ships
continuously** — value lands from M1 onward, not at the end.

---

## Recommended first step

**Start M0.** Concretely, the first PR: complete the EF model (scaffold all ~40
tables + baseline migration) and stand up the auth foundation (JwtBearer with the
shared secret + the project-scope authorization policy). Those two unblock every
subsequent slice, are independently testable (Testcontainers, already wired), and
carry no cutover risk. The credential-encryption port is the third M0 item and the
one to spike early, since it's the highest-uncertainty foundation.
