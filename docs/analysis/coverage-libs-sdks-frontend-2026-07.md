# Test Coverage Survey — C# Support Libs, Multi-Language SDKs, Frontend

**Date:** 2026-07-22
**Scope:** READ-ONLY, risk-weighted. Three buckets: (A) C# support libraries,
(B) the five multi-language SDK endpoints, (C) the React/TS dashboard.
**Method:** Structural analysis of source vs. tests. The .NET 10 solution could
not be built (local SDK is .NET 8), so bucket A is structural. Frontend vitest
suite was executed: **29 files / 159 tests, all green, 2.74s**. SDK and C# test
suites were read, not run.

> Note on "risk": crypto/auth/data-integrity gaps rank highest, then
> wrong-number-display and RBAC bypass, then edge cases and polish. Line-% is
> deliberately not the metric — meaningful coverage is.

---

## Bucket A — C# Support Libraries

### A1. Networker.Security (CredentialCipher, AgentApiKeys) — STRONG, no gaps
Files: `src/Networker.Security/CredentialCipher.cs`;
tests `tests/Networker.Security.Tests/CredentialCipherTests.cs` +
`tests/Networker.ControlPlane.Tests/AgentApiKeyAuthTests.cs`.

Crypto coverage is comprehensive: round-trip (empty/1-byte/JSON/1 KB binary),
**per-encryption nonce randomness**, **wrong-key rejection**, **GCM tamper
detection**, key-rotation fallback (old-key recovery, no-fallback throw, primary
precedence), and a **Rust known-answer cross-check** (byte-for-byte vector
match). AgentApiKeys hashing is covered for determinism, key-sensitivity,
null-safety, and **length-mismatch rejection in the fixed-time compare**, plus
e2e auth (hash-only lookup, expiry enforcement) in the ControlPlane suite.
**Verdict: this is the best-tested library in the repo. No action.**

### A2. Networker.Agent (the tester-VM worker) — MEANINGFUL GAPS
Files: `RawWebSocketClient.cs`, `AgentWorker.cs`, `RunExecutor.cs`,
`CommandHandler.cs`, `TestConfigView.cs`, `TesterBinaryLocator.cs`;
tests `tests/Networker.Agent.Tests/{ConfigAndArgs,RunExecutorMapping,CommandHandler,ProtocolWire}Tests.cs`.

Well covered: **CLI arg-building** (config→argv, mode/runs/concurrency/timeout
rounding, insecure flag, payload defaults, byte-for-byte Rust parity), the
**control-message dispatch + agent-message wire encoding** (all 7 message
types), and **run error-mapping** (unsupported endpoint / missing binary /
unparseable JSON → error+failed; happy-path frame sequence).

Gaps, ranked:
- **[HIGH] `TesterBinaryLocator` is completely untested.** Search order (bare
  relative → cwd+parents → PATH), the `AGENT_TESTERPATH` override, and
  Windows `.exe` suffix handling have zero tests. Real bug: a misconfigured or
  non-existent configured path silently falls through to a PATH lookup, so the
  agent starts but every run dies with a cryptic "tester not found." *Test idea:*
  mock FS + PATH, assert each search branch and that the configured path is used
  verbatim with `.exe` on Windows.
- **[HIGH] `RawWebSocketClient` reconnect/back-pressure untested.** The send
  pump uses a bounded channel with `DropWrite`; nothing verifies that heartbeats
  survive a frame flood, nor that the reconnect loop / heartbeat pump tear down
  cleanly. Real bug: under a progress-frame spike, heartbeats are silently
  dropped and the control plane thinks the agent is dead (or a ghost-connected
  agent never recovers). *Test idea:* drive a slow sink, flood frames, assert a
  heartbeat still lands.
- **[MED] `AgentWorker` disconnect → CancelAllRuns untested.** No test proves
  in-flight `RunExecutor` tasks are cancelled when the socket drops. Real bug:
  orphaned run tasks / zombie tester processes on every disconnect. *Test idea:*
  mock connect→drop, assert cancellation tokens fire.

### A3. Networker.Data (SchemaMigrator + SQL migrations + EF model) — MED GAPS
Files: `src/Networker.Data/Migrations/SchemaMigrator.cs` + 40+ `V0NN_*.sql`;
tests `tests/Networker.Tests/SchemaMigrationTests.cs`.

Covered: fresh-DB full-chain apply, **rerun idempotency**, bookkeeping-table
shape matching the Rust contract, and an **EF-model equivalence** pass that
materializes every `DbSet<T>` against the migrated schema.

Gaps, ranked:
- **[MED] No per-migration dependency validation.** A new `V0NN` that references
  a table introduced in a later `V0MM` (MM>NN) would only fail at first apply on
  a fresh DB — after the release ships. *Test idea:* parse each migration's
  table/FK references, build a DAG, assert acyclic + every reference is
  satisfied by an earlier file.
- **[LOW-MED] The V025 ProjectId (UUID→base36) code migration is opaque.** EF
  queries succeed, but the transformation output format is never asserted
  against the Rust base36 encoding — drift would break older protocol parsers.
  *Test idea:* after migration, assert a project id column matches the expected
  base36 shape.
- **[LOW] No embedded-script integrity check** (accidental deletion/corruption
  of a `.sql` resource is caught only at apply time). *Test idea:* golden-list
  the embedded script names/content.

### A4. Networker.Endpoint (diagnostic server) — MED GAPS
Files: `src/Networker.Endpoint/Endpoints.cs` + UDP services;
tests `tests/Networker.Endpoint.Tests/{Endpoint,VersionSingleSource}Tests.cs`.

Covered: `/health` constant-work shape, `/download` (path+query, default size,
fill byte), `/upload` echo, `/echo` (GET/POST + header echo), `/status/{code}`,
landing HTML, and the version single-source guard.

Gaps, ranked:
- **[MED] Benchmark `/api/*` endpoints largely untested.** Only `/api/users`
  presence is checked; `/api/transform`, `/aggregate`, `/search`,
  `/upload/process`, `/delayed`, `/validate` have no field/shape assertions.
  Real bug: a renamed field (snake_case vs PascalCase) silently breaks
  Application-Benchmark runs. *Test idea:* per endpoint, assert required fields
  present + typed.
- **[MED] No upper bound tested on `/download?bytes=` or `/delay?ms=`.** Real
  bug: `?bytes=999999999` OOMs the endpoint (DoS vector). *Test idea:* assert
  `ms` capped at 30 s and `bytes` capped, rejecting over-limit.
- **[LOW] UDP echo/throughput services untested** (port binding, echo response).

### A5. Networker.Contracts (versioned JSON seam) — MED GAP
Files: `ProbeRunResult.cs`, `ProbeContractJsonContext.cs`;
tests `tests/Networker.Tests/ContractRoundTripTests.cs` +
`tests/Networker.Agent.Tests/ProtocolWireTests.cs`.

Covered: golden-fixture round-trip (top-level + per-phase timings), tolerance of
unmodelled Rust fields, and graceful null on missing optional/`schema_version`.

Gaps:
- **[MED] Schema-evolution not tested.** Only the *current* `tester-golden.json`
  is exercised; there are no fixtures for prior/future schema versions. Real
  bug: a tester/dashboard version skew (rollback) silently ignores new fields or
  nulls a removed one, producing garbage analysis. *Test idea:* add past +
  future-field fixtures; assert graceful degradation.
- **[LOW-MED] Field-name drift is only guarded by the one golden file.** A new
  nullable property added without its `[JsonPropertyName]` snake_case would
  deserialize to null and the test still passes. *Test idea:* reflect over
  `ProbeRunResult` and assert every property carries the expected snake_case
  name.

---

## Bucket B — Multi-Language SDK Endpoints

Contract: `docs/sdk/contract-v1.md` (7 security-critical properties). **Headline:
all five languages ship both conformance and safety suites, and every one of the
7 properties is asserted in every language.** This is unusually good — the SDKs
are the best-covered surface in the repo relative to risk.

Security-property coverage matrix (✓ = asserted, ⚠ = present but thin):

| Property | Go | JS | Python | Rust | C# |
|---|:--:|:--:|:--:|:--:|:--:|
| 404-invisibility (bare 404 on unauth) | ✓ | ✓ | ✓ | ✓ | ✓ |
| rate-limit BEFORE auth | ✓ | ✓ | ✓ | ✓ | ✓ |
| kill switch (`LAGHOUND_DISABLED`) | ✓ | ✓ | ✓ | ✓ | ✓ |
| token rotation (prev token valid) | ✓ | ✓ | ✓ | ✓ | ✓ |
| byte budget (429 + Retry-After) | ✓ | ✓ | ✓ | ✓ | ✓ |
| concurrency cap | ✓ | ✓ | ⚠ | ✓ | ✓ |
| Server-Timing header | ✓ | ✓ | ✓ | ✓ | ✓ |

Ranking best → thinnest (all are acceptable):
1. **Rust** — separate `killswitch.rs` binary, `safety.rs` streaming
   memory-bound via frame-size polling, full conformance loop.
2. **Go** — single 900+ line `conformance_test.go`, richest edge cases
   (blocking-reader concurrency cap, upload Content-Length-over-cap without
   reading body, Server-Timing metric/byte bounds).
3. **JS** — conformance + safety + example; **RSS-based memory bound** via a
   `memprobe.ts` child process; kill-switch cache test.
4. **C#** — conformance + safety + unit; unit tests validate the **constant-time
   token compare** and Server-Timing rounding/limits; GC-allocation memory bound.
   (Coverage is solid; the separate de-flake effort is about flakiness, not
   coverage.)
5. **Python** — dual **ASGI + WSGI** coverage with bounded-drain assertions on
   both; the one soft spot is the concurrency cap (present but sparser than the
   others) and **no explicit streaming memory-bound test** like Rust/JS/C# have.

Per-language gaps to close:
- **[LOW] Python:** add a streaming memory-bound test (large download → RSS/alloc
  delta stays small) and strengthen the concurrency-cap assertion (hold N slots,
  assert the N+1th gets 429, assert release frees a slot). Real bug: a future
  refactor buffers the whole download in memory / drops the cap and only Python
  wouldn't catch it.
- **No language is dangerously under-tested.**

---

## Bucket C — Frontend Dashboard (React + TS + Vite)

Vitest ran clean: **29 files / 159 tests**. Coverage is concentrated in
`lib/` formatting utils, the RBAC-gated surfaces that already have `.rbac`
tests, and the API client. Large swaths of pages/hooks are untested, but most
untested pages are read-only. The two things that actually matter — an RBAC
render gap and untested number-crunching — are called out below.

### C1. RBAC / role-gating — one real client-side gap
Tested (thorough): `AlertsPage.rbac`, `InfrastructurePage.rbac`,
`SettingsTabs.rbac`, `Sidebar.rbac`, `useProject.rbac` (fails **closed** on
unknown role), `TesterDetailDrawer.rbac`. These assert viewers see badges, not
controls, and operators/admins see the danger-zone actions.

- **[P0 / verified] `ProjectMembersPage` renders admin controls with no role
  gate.** Confirmed by reading the source: it calls `useProject()` only for
  `projectId` and never reads `isProjectAdmin`/`isOperator`. The Invite / Add
  Existing / role `<select>` / Remove / CSV-import / bulk-send controls render
  for any role that can reach the route. Same pattern applies to
  `ShareLinksPage`, `CommandApprovalsPage`, `CloudAccountsPage`,
  `TlsProfilesPage`, `UsersPage`, `PerfLogPage`, `SystemDashboardPage` — gating
  today is **navigation-only** (SettingsTabs/Sidebar hide the link); the pages
  themselves don't gate on render. The server API is the true authority, so this
  is defense-in-depth, not an auth bypass — but it contradicts the project's
  "gate controls behind isOperator" rule and the precedent set by the other
  `.rbac` tests. Real bug: a viewer who deep-links (or whose link-hiding
  regresses) sees fully-wired admin forms. *Test idea:* render each page as
  `viewer`, assert the mutating controls are absent (and add the `if
  (!isProjectAdmin) return <PermissionDenied/>` gate they assert against).

### C2. Data-transform / formatting utils — the highest-value untested code
Tested: `format` (timeAgo/duration/hostLabel), `ansi` (stripAnsi), `brand`,
`runStatus`, `modes-manifest`.

- **[P0] `lib/analysis.ts` (~288 lines) is completely untested.** This is the
  math behind every run/protocol stat: `computeStats` (percentiles via linear
  interpolation), `primaryMetricValue`/`primaryMetricLabel` (per-protocol field
  extraction across 14+ protocols), `isThroughputProtocol`, `computeProtocolStats`
  (success-rate division), `formatMs`/`formatThroughput`/`formatBytes`. Real
  bugs: off-by-one percentile rank at small n; a missing protocol handler
  swapping latency vs. throughput; division-by-zero when all attempts fail;
  `formatThroughput(1234)` rounding to "1.2 GB/s" instead of "1.23". These are
  wrong-number-on-screen bugs on the primary results view. *Test idea:* pin
  `computeStats([1..5])` against a known percentile table; assert
  `primaryMetricValue` for every protocol variant.
- **[P1] `lib/benchmark.ts` untested** — `formatBenchmarkDelta` risks a missing
  `%` suffix and no NaN/Infinity guard before `Intl.NumberFormat`. *Test idea:*
  `formatBenchmarkDelta(5.2) === "+5.2%"`.
- **[P2] `docs/search.ts`, `stableUpdate.ts`, `requestSource.ts` untested** —
  scoring weights, JSON-stringify fingerprint stability, and request-source
  attribution respectively. Lower blast radius.

### C3. Hooks — subscription/state layer largely untested
Tested: `useTesterSubscription`/`usePhaseSubscription` (seq filtering, entity
filtering), `useProject` (RBAC). Untested and logic-bearing:
- **[P1] `useWebSocket`** (reconnect/backoff, token injection, socket teardown),
  **`usePolling`** (stale-closure on interval change), **`useDeployEvents`** /
  **`useRenderLog`** (ordering/dedup). Real bug: zombie sockets leaking memory,
  or a polling loop running an old callback after deps change. *Test idea:*
  socket `close` → assert a new connection opens; change poll interval → assert
  the old tick stops.

### C4. API client (`api/client.ts`) — well covered
Tested: Bearer-header injection, 401 session-wipe (but **not** on `/auth/login`),
4xx/5xx → `ApiError`, network error → status 0, `AbortError` passthrough, 204 →
undefined, friendly-error mapping (nginx HTML → "Server unavailable"), and
`getAgents` legacy-shape normalization. Untested: concurrent-request dedup,
timeout handling, redirect following — all lower risk. `api/testers.ts` /
`vmHistory.ts` are thin wrappers over the tested `request()`.

### C5. Biggest untested source areas (mostly read-only, lower risk)
`RunDetailPage`, `DashboardPage`, and the Bench*/report pages carry real
display logic (all consume the untested `analysis.ts` — so C2 covers the root
risk). Wizards (`DeployWizard`, `InfraDeployWizard`) have untested multi-step
state machines: [P2] step-order/back-forward duplication.

---

## "Add These First" — per bucket

**Bucket A (C# libs):**
1. `TesterBinaryLocator` unit tests (search order, override, Windows `.exe`).
2. `RawWebSocketClient` back-pressure + reconnect/heartbeat teardown tests.
3. `AgentWorker` disconnect→CancelAllRuns test.
4. Migration dependency DAG validation test.
5. `/api/*` endpoint shape tests + download/delay upper-bound caps.

**Bucket B (SDKs):**
1. Python: add a streaming memory-bound test.
2. Python: strengthen the concurrency-cap assertion (N+1 → 429, release frees).
   *(Everything else is already covered in every language.)*

**Bucket C (frontend):**
1. `lib/analysis.ts` full unit suite (percentiles, per-protocol metric, success
   rate, throughput/ms formatting).
2. RBAC render gate + tests for `ProjectMembersPage`, `ShareLinksPage`,
   `CommandApprovalsPage`, `CloudAccountsPage` (viewer → controls absent).
3. `lib/benchmark.ts` formatting tests (`%` suffix, NaN/Infinity).
4. `useWebSocket` / `usePolling` lifecycle tests.

---

## Top ranked gaps across all three buckets

| # | Risk | Bucket | Gap | Bug it catches |
|---|------|--------|-----|----------------|
| 1 | P0 | C-frontend | `lib/analysis.ts` untested (all stat/percentile/format math) | Wrong numbers on the primary results/benchmark views (bad percentiles, latency↔throughput swap, div-by-zero) |
| 2 | P0 | C-frontend | `ProjectMembersPage` (+ 3 sibling admin pages) render admin controls with no role gate — **verified in source** | Viewer deep-links and sees fully-wired invite/remove/role-change forms; violates "gate behind isOperator" |
| 3 | HIGH | A-agent | `TesterBinaryLocator` completely untested | Misconfigured/missing tester path → agent runs but every job fails "not found" |
| 4 | HIGH | A-agent | `RawWebSocketClient` back-pressure/reconnect untested | Heartbeats silently dropped under frame flood → control plane marks a live agent dead / ghost connection never recovers |
| 5 | MED | A-agent | `AgentWorker` disconnect→CancelAllRuns untested | Orphaned run tasks / zombie tester processes on every disconnect |
| 6 | MED | A-endpoint | Benchmark `/api/*` shapes untested + no download/delay caps | Renamed field silently breaks App-Benchmark; `?bytes=huge` OOMs endpoint |
| 7 | MED | A-data | No per-migration dependency validation | A new migration referencing a later table fails only at first apply, post-release |
| 8 | MED | A-contracts | No schema-evolution fixtures | Tester/dashboard version skew silently nulls fields → garbage analysis |
| 9 | P1 | C-frontend | `useWebSocket`/`usePolling` lifecycle untested | Zombie sockets leak memory; stale-closure race after interval change |
| 10 | LOW | B-python SDK | No streaming memory-bound + thin concurrency-cap test | A future buffering/cap regression is caught in every SDK language *except* Python |

**Dangerously under-tested SDK languages: none.** All five SDKs conformance-test
all 7 security properties. The only relative soft spot is **Python** (missing the
streaming memory-bound test that Rust/JS/C# have, and a thinner concurrency-cap
assertion) — a gap to close, not a danger.
