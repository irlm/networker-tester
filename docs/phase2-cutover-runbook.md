# Phase 2 — M6 Cutover Runbook (Rust dashboard → C# control plane)

> **Status (2026-07): the cutover is complete** — prod (laghound.com, with
> alethedash.com as the compatibility bridge — see `branding.md`) is
> served by the C# control plane. This file stays at this path because the
> nightly soak check and the decommission standing order reference its section
> numbers. Sections 1–3 and 6 are historical; **§4 (soak checklist), §5
> (rollback), and §7 (decommission criteria) remain operative** until the Rust
> control-plane crates are decommissioned. Current architecture:
> [`architecture.md`](architecture.md); release/deploy mechanics:
> [`release-flow.md`](release-flow.md).

Operational runbook for cutting production traffic over from the Rust
`networker-dashboard` to the C# `Networker.ControlPlane`, hardening delivered in
M6: **per-tick leader election** (Postgres advisory locks), **background-service
observability** (`/api/health/background`), and a **readiness probe**
(`/api/health/ready`).

Audience: the operator running the cutover. Everything here assumes the shared
production PostgreSQL/TimescaleDB database — both control planes read and write
the SAME schema during the transition window.

## Contents

1. [Prerequisites](#1-prerequisites)
2. [Leader election & background-service ops](#2-leader-election--background-service-ops)
3. [Cutover sequence](#3-cutover-sequence)
4. [Soak checklist](#4-soak-checklist)
5. [Rollback](#5-rollback)
6. [The WebSocket surface (raw-WS vs SignalR)](#6-the-websocket-surface-raw-ws-vs-signalr)
7. [Decommission criteria](#7-decommission-criteria)
8. [Known gaps / open items](#8-known-gaps--open-items)

---

## 1. Prerequisites

### 1.1 Environment variables (C# control plane)

| Variable | Required | Behaviour |
|---|---|---|
| `DASHBOARD_JWT_SECRET` | **Yes — fail-closed** | HS256 signing key, shared with the Rust dashboard so tokens minted by either side validate on both. Outside `ASPNETCORE_ENVIRONMENT=Development` (and **unset counts as Production**) the app **refuses to start** without it. Generate: `openssl rand -base64 32`. |
| `DASHBOARD_CREDENTIAL_KEY` | **Yes — fail-closed** | 64 hex chars; AEAD key for cloud-account secrets, byte-compatible with the Rust cipher (C# must decrypt rows Rust wrote). Same fail-closed rule as the JWT secret. `DASHBOARD_CREDENTIAL_KEY_OLD` optionally enables key rotation. |
| `ConnectionStrings__Networker` (or `DASHBOARD_DB_URL_NPGSQL`) | Yes | Npgsql connection string to the **shared** database, e.g. `Host=…;Port=5432;Database=networker_core;Username=…;Password=…`. Falls back to localhost dev defaults if unset — never rely on the fallback in prod. |
| `DASHBOARD_BACKGROUND_SERVICES` | No (default **on**) | `0`/`false` makes a replica **API-only** (no scheduler/watchdog/reaper/… loops registered). With M6 leader election this is no longer required for safety with multiple replicas, but keep it as the explicit ops switch: during cutover run the C# loops **disabled** until step 3.4 flips ownership. |
| `ASPNETCORE_ENVIRONMENT` | Yes (`Production`) | Anything other than `Development` enforces the fail-closed secrets. |
| `ASPNETCORE_URLS` | Yes | The "separate port" for side-by-side deploy, e.g. `http://0.0.0.0:5030` (Rust stays on `:3000`). |
| `DASHBOARD_PUBLIC_URL` | **Yes for provisioning** | The publicly reachable base URL (`https://laghound.com` in prod). The tester-provisioning bootstrap derives the agent WebSocket URL from it; unset, new tester agents are pointed at `ws://localhost:3000` and never come online (the control plane logs a warning). The prod deploy asserts it into `/etc/alethedash-cs.env` idempotently, replacing a stale value (e.g. the pre-cutover `https://alethedash.com`) if present. |
| `AZ_CMD`, `AWS_CMD`, `GCLOUD_CMD` | No | Absolute-path overrides for the cloud CLIs the provisioner shells out to. Unset, the bare names resolve via the service's `PATH` — the codified unit (`deploy/alethedash-cs.service`) extends `PATH` with `/usr/local/bin` and `/snap/bin` (snap-installed `gcloud`). A CLI that fails to launch reports which binary was missing and which of these vars overrides it. |

Agent side (C# `Networker.Agent`): `AGENT_DASHBOARDURL` (base URL; the agent
connects to `{url}/ws/agent`), `AGENT_TESTERPATH`, `AGENT_NAME` — note the
config-binder naming: `AGENT_DASHBOARDURL`, **not** `AGENT_DASHBOARD_URL`.
Agents authenticate with their `api_key` inside the hub handshake.

### 1.2 Program.cs wiring (one-time, before the cutover build)

M6's ops infrastructure is wired with two lines:

```csharp
// service registrations — after AddNetworkerAuth (needs its NpgsqlDataSource):
builder.Services.AddOpsInfrastructure();   // TickMonitor + PgAdvisoryLeaderLock

// endpoint mapping — alongside the other Map* calls:
app.MapOpsEndpoints();                     // /api/health/background, /api/health/ready
```

Until these lines land, the background services run **unguarded** (identical to
pre-M6 behaviour — the lock and monitor are optional dependencies), and the two
ops endpoints don't exist. **Do not start the cutover without them.**

### 1.3 Pre-flight checks

- [ ] `dotnet test Networker.sln -c Release` green on the cutover SHA.
- [ ] `GET /api/health/ready` on the C# instance returns `200 {"status":"ready"}`
      against the production DB (readiness probe wired into the deploy/LB).
- [ ] `GET /api/health` (Rust) and `GET /api/health` (C#) both `ok` — same DB.
- [ ] Login round-trip: a JWT minted by the Rust dashboard is accepted by a C#
      `GlobalViewer` endpoint (shared `DASHBOARD_JWT_SECRET` proven, not assumed).
- [ ] A cloud-account secret written by Rust decrypts in C# (`DASHBOARD_CREDENTIAL_KEY`
      proven, not assumed) — e.g. `POST /api/cloud-accounts/{id}/validate`.
- [ ] Backup/snapshot of the database taken.

---

## 2. Leader election & background-service ops

### 2.1 Design (per-tick advisory locks)

Every background loop wraps **each tick** in
`PgAdvisoryLeaderLock.TryRunAsLeaderAsync(key, tick)`:

1. open a pooled connection;
2. `SELECT pg_try_advisory_lock(key)` — non-blocking; if another session holds
   the key, **skip this tick** (retry on the service's own next interval);
3. run the tick;
4. `SELECT pg_advisory_unlock(key)` **on the same connection** (advisory locks
   are session-scoped), then release the connection.

There is no lease, renewal, or fencing: if the process dies mid-tick the DB
session drops and Postgres frees the lock, so any surviving replica takes over
on its next tick. Ticks from different replicas interleave over time — that is
fine; the property enforced is that two ticks of the same loop never run
**concurrently** (the double-fire hazard: duplicate schedule fan-out, double VM
deallocation, duplicate deploys).

`DASHBOARD_BACKGROUND_SERVICES=0` remains as the coarse manual gate on top.
**Critically, the Rust dashboard does not take these locks** — advisory locks
only exclude C# replicas from each other. Rust-vs-C# loop ownership during the
transition is managed with the Rust `scheduler` disable / C# env gate (see 3.4).

### 2.2 Per-service lock keys

Keys are `FNV-1a 64-bit` over UTF-8 of `"networker-controlplane:" + name`
(`LeaderLockKeys.KeyFor`), frozen by `LeaderLockKeysTests`:

| Service | Tick interval | Advisory key (int8) |
|---|---|---|
| `scheduler` | 30 s | `204212623316596031` |
| `queued-redispatch` | 30 s | `-3975542237568181939` |
| `watchdog` | 60 s | `2528921118521860045` |
| `agent-reaper` | 60 s | `5672273927518729125` |
| `auto-shutdown` | 60 s | `-6779850655081117222` |
| `orphan-reaper` | 10 min | `-3722790822933648360` |
| `workspace-inactivity` | 24 h (first pass +5 min) | `5344226851487828108` |
| `provisioning-orchestrator` | 5 s | `-6476070105187748186` |

Inspect held locks:

```sql
SELECT (classid::bigint << 32) | objid::bigint AS key, pid, granted
  FROM pg_locks WHERE locktype = 'advisory';
```

### 2.3 Observability endpoints

**`GET /api/health/background`** (auth: GlobalViewer policy — any valid user):

```json
{
  "services": [
    {
      "name": "scheduler",
      "last_tick_at": "2026-07-14T12:03:11Z",
      "seconds_since_tick": 12.4,
      "ticks_total": 118,
      "last_items": 0,
      "last_note": "launched=0 seeded=0 skipped_no_agent=0 failed=0",
      "last_error": null,
      "last_error_at": null,
      "expected_interval_secs": 30,
      "healthy": true
    }
  ],
  "all_healthy": true,
  "background_services_enabled": true
}
```

- `healthy` = ticked within **3× its expected interval** (never-ticked services
  get the same 3× grace from loop start, which covers every startup delay).
- `last_error` is sticky until the next error so a recovered loop still shows
  its history during soak review.
- An API-only replica correctly reports an empty `services` list with
  `background_services_enabled: false`.

Per-service `last_items` / `last_note` meanings: scheduler = due schedules
(`launched/seeded/skipped_no_agent/failed`); queued-redispatch = runs
redispatched; watchdog = runs reaped (`reaped_running/reaped_queued`);
agent-reaper = agents flipped offline (`candidates/reaped`); auto-shutdown =
drained candidates (`stopped/deferred/failed`); orphan-reaper = resources
deleted (`identified/deleted/failed`); workspace-inactivity = actions taken
(`live/warned/suspended/hard_deleted`); provisioning-orchestrator = runs moved
(`kicked/resolved`).

**`GET /api/health/ready`** (public): `200 {"status":"ready","db":"ok"}` when
the DB answers `SELECT 1`, else `503` — wire this as the LB/deploy readiness
probe so a replica that cannot reach the DB never receives traffic.

---

## 3. Cutover sequence

Each step is independently reversible. Do not batch steps.

### 3.1 Deploy C# side-by-side (API-only)

- Deploy `Networker.ControlPlane` on its own port (`ASPNETCORE_URLS`, e.g.
  `:5030`) against the shared DB, with `DASHBOARD_BACKGROUND_SERVICES=0`.
- Rust dashboard keeps serving all traffic on `:3000` and keeps owning all
  background loops. C# reads/writes only when called.
- Validate: `/api/health/ready` 200; `/api/health/background` shows
  `background_services_enabled: false`, empty services; spot-check reads
  (`/api/projects`, `/api/test-runs`) against Rust responses for the same JWT.

### 3.2 Point ONE agent at C#

- Pick a low-value agent; set its dashboard URL to the C# instance
  (`AGENT_DASHBOARDURL=http://<c#-host>:5030`) and restart it.
  (Current constraint: that agent must be the C#/SignalR agent — see §6.)
- Validate on the C# side: agent row flips `online`, heartbeats update
  `last_heartbeat`, `/ws/dashboard` subscribers see the `AgentStatus` event.

### 3.3 Validate one full launch → run → complete loop through C#

- `POST /api/test-configs/{id}/launch` on the C# instance targeting the moved
  agent; watch the run go `queued → running → completed` with results persisted
  and live `JobUpdate` events on the browser hub.
- Repeat with a scheduled config (create a `* * * * *` schedule, watch the C#
  scheduler fire it once loops are enabled in 3.4, then delete it).
- Repeat with a `Pending`-endpoint config only if provisioning is in scope for
  the initial cutover; otherwise defer to the soak.

### 3.4 Flip background-loop ownership Rust → C#

This is the only step where two writers could overlap — do it in this order:

1. Stop the Rust dashboard's scheduler/reaper loops (restart Rust with its
   scheduler disabled, or accept a short full-stop of the Rust process).
2. Restart C# with `DASHBOARD_BACKGROUND_SERVICES` unset (loops on).
3. Watch `/api/health/background` until all 8 services report `healthy: true`
   (inactivity shows healthy-from-start; first real pass comes 5 min later).
4. Verify in `pg_locks` that the advisory keys appear/disappear as ticks run.

### 3.5 Flip the frontend proxy `/api` + `/ws` to C#

- Precondition: the `/ws` surface decision from §6 is resolved (bridge shipped
  or frontend moved to the SignalR client). `/api` alone can be flipped earlier
  — the REST surface is drop-in.
- Repoint the production reverse proxy (or `dashboard/vite.config.ts` targets
  in dev: `http://localhost:3000` → the C# port) for `/api` and `/ws`.
- Move the remaining agents (one batch at a time, watching agent-status flaps).
- Rust dashboard now serves nothing but stays running, warm, as the rollback
  target.

### 3.6 Soak

Run the [soak checklist](#4-soak-checklist) for the agreed window (see
[decommission criteria](#7-decommission-criteria) — N clean days).

---

## 4. Soak checklist

Watch daily (or alert on):

- **`all_healthy` on `/api/health/background`** — the single most important
  signal. Any `healthy: false` or a growing `last_error` needs a root cause
  before the soak clock continues.
- **Stuck-queued metric** — `SELECT count(*) FROM test_run WHERE status='queued'
  AND created_at < now() - interval '5 minutes';` should be ~0 (the watchdog
  fails them; a persistent nonzero count means dispatch or redispatch is broken).
  Also watch the watchdog's `reaped_queued` note: a steady climb = runs are
  being created that no agent ever claims (the v0.28 scheduler-churn bug's
  signature).
- **Agent status flaps** — agents oscillating online/offline (reaper note
  `reaped>0` every tick, or `AgentStatus` event bursts) indicate heartbeat
  interval vs 90 s staleness mismatch or proxy WS idle timeouts.
- **Event-bus replay on reconnect** — kill a browser tab's connection, reconnect
  with `?since=<seq>`, confirm the missed window replays and no seq gaps are
  logged by the frontend. Do the same for an agent reconnect (run events must
  resume, no orphaned `running` rows).
- **Run outcomes parity** — daily counts of `completed` / `failed` runs compared
  against the pre-cutover baseline week.
- **Provisioning** (if in scope): deployments reach `completed`, runs re-queue,
  no orphan VM/NIC/IP growth (orphan-reaper `identified` should trend to 0).
- **No Rust-dashboard writes** — confirm the Rust process (if still running) is
  not writing: its logs show no dispatch/reap activity, or simply stop it after
  the proxy flip.
- **DB health** — advisory locks released (no long-`granted` rows in
  `pg_locks`), no connection-pool exhaustion (each tick briefly holds one
  extra connection for the lock).

---

## 5. Rollback

Rollback is a config flip, not a deploy:

1. Repoint the proxy `/api` + `/ws` back to the Rust dashboard (`:3000`).
2. Point agents back at the Rust URL.
3. Restart C# with `DASHBOARD_BACKGROUND_SERVICES=0` (or stop it) and re-enable
   the Rust scheduler loops.
4. The shared DB needs no migration in either direction — both sides speak the
   same schema. Clean up only artifacts of the failed window (e.g. runs failed
   by the C# watchdog during the incident) case-by-case.

Order matters in reverse too: give Rust the background loops back **before**
stopping the C# loops is acceptable for the reapers (idempotent, guarded
updates), but never let both schedulers run simultaneously — that double-fires
schedules.

---

## 6. The WebSocket surface (raw-WS vs SignalR)

**Target state (the M6 plan):** the legacy raw-WS endpoints keep their exact
paths and JSON frames — `/ws/dashboard` (browser event feed with `?since=`
replay), `/ws/testers`, `/ws/agent` (the `networker-common` agent protocol) — so
the React frontend and the Rust agents connect **unmodified**, served by a thin
raw-WebSocket bridge in the C# app that translates to the internal EventBus /
agent registry. Native SignalR hubs then live at **`/hub/*`** for future
clients (the C# agent, upcoming integrations), which get reconnection/groups/
backplane for free.

**Current state (honest):** the C# app maps **SignalR hubs directly at
`/ws/dashboard`, `/ws/testers`, `/ws/agent`**. SignalR's handshake is not raw
WS, so today:

- the React frontend's `new WebSocket(...)` hooks **cannot** talk to the C#
  `/ws/*` endpoints;
- the Rust agent **cannot** connect to the C# agent hub; only the C#
  (`Networker.Agent`) SignalR agent can.

Before step 3.5's `/ws` flip, one of these must ship: (a) the raw-WS bridge at
`/ws/*` (+ move SignalR to `/hub/*`) — preferred, zero client churn; or (b) the
frontend adopts `@microsoft/signalr` and all agents are replaced with the C#
agent — more churn, no bridge to maintain. This is open item #1 below.

The event **contract** already matches (seq numbers, replay-on-`since`, dedup by
`seq <= maxApplied`, same event JSON shapes) — the gap is framing/handshake,
not semantics.

---

## 7. Decommission criteria

Delete `crates/networker-dashboard`, `crates/networker-common`, and the
`networker-log` surface (and the Rust agent) only when **all** hold:

1. **N clean soak days** (recommend N = 14) with `all_healthy: true`, no
   rollback events, and no P1/P2 incidents attributed to the C# control plane.
2. **Endpoint parity diff-verified**: the recorded Rust↔C# response diff suite
   has been run across the full REST surface on production-shaped data, with
   every intentional difference documented.
3. **No Rust-dashboard writes observed** for the entire soak window (process
   stopped, or DB audit shows zero writes from its connection role).
4. **All agents** migrated and stable (no status flaps for the soak window).
5. **Rollback path retired deliberately**: a tagged Rust build + config to
   restore it exists (`legacy/rust` branch, `rust-legacy-v0.28.13` tag) — the
   crates are deleted from `main`, not from history.
6. Open items below either closed or explicitly accepted as post-decommission
   work by the owner.

---

## 8. Known gaps / open items

Honest list — none of these block the side-by-side deploy; items 1–2 block the
full `/ws` flip / feature parity claims:

1. **Raw-WS bridge** (§6): SignalR sits on `/ws/*`; the legacy raw-WS clients
   (React frontend, Rust agents) can't connect. Bridge at `/ws/*` + hubs to
   `/hub/*`, or migrate the clients.
2. **Email stubs**: the workspace-inactivity loop logs
   `TODO email stub` instead of sending the 90-day member warning and the
   hard-delete admin notice (no mailer wired). Suspend/delete actions DO run —
   users are currently not warned by email first.
3. **`detect-languages` stub**: `POST .../benchmark-catalog/detect-languages`
   does not SSH-probe the target (Rust `ssh_detect_languages` not ported);
   returns a stubbed response.
4. **`service_log` lines**: the ops-log DB table is not part of the EF model —
   admin log views and the agent-command `log` SSE stream poll status only;
   Rust-side service log lines are not persisted/served by C#.
5. **Cloud SDK migration**: provisioning still shells out to
   `install.sh`/`az`/`aws`/`gcloud` for parity; `Azure.ResourceManager` /
   `AWSSDK` / `Google.Cloud.Compute` behind `IComputeProvisioner` is
   deliberately post-cutover.
6. **Shared-config endpoint mutation**: promote rewrites
   `test_config.endpoint_ref` in place (matches Rust); the per-run resolved
   endpoint column is the planned fix.
7. **Leader election scope**: advisory locks exclude C# replicas from each
   other only — they do NOT coordinate with the Rust dashboard's loops (managed
   manually in 3.4), and `/api/health/background` is per-replica (each replica
   reports its own ticks).
8. **Program.cs wiring**: `AddOpsInfrastructure()` + `MapOpsEndpoints()` must be
   added by the Program.cs owner (§1.2) — the services degrade gracefully
   (unguarded ticks, invisible monitor) until then.
