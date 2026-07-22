# Dead-Code Survey — C# Solution (2026-07)

**Scope:** `src/Networker.ControlPlane`, `src/Networker.Agent`, `src/Networker.Data`,
`src/Networker.Security`, `src/Networker.Contracts`, `src/Networker.Endpoint`,
their tests in `tests/`, plus `sdk/csharp` (checked only for internally-unused
members — it is a separately shipped product).

**Method & confidence caveat:** the solution targets .NET 10 and the local SDK
is .NET 8, so this survey could **not** build or use Roslyn analyzers. It is a
**structural (grep-based) reference trace**: every `public`/`internal` type
declared in the scoped projects (341 in ControlPlane, 58 in
Agent/Endpoint/Contracts/Security, plus all test-project types) was
cross-referenced against every `.cs`/`.ts`/`.tsx` file in `src/`, `tests/`,
`dashboard/src/`, and `sdk/` (excluding `bin/`/`obj/`/`node_modules/`). Types
with zero external references were then checked for in-file use (same-file
minimal-API handler DTOs are live — ASP.NET binds by type), and static
extension classes were re-checked at **method** granularity (extension methods
are invoked without the class name). Confidence levels below already account
for reflection / DI-by-convention / JSON-name binding risk, but a compiler
pass (e.g. `dotnet build` with IDE0051/IDE0052, or an unused-symbol Roslyn
scan once a .NET 10 SDK is available) should confirm before deleting.

**Headline:** the solution is unusually clean — the automated sweep found
**zero orphaned types** in Networker.Data, Contracts, Security, Endpoint,
Agent (besides one over-visible method), the test projects, and the
ControlPlane endpoint/DTO layer. The real findings concentrate in one place:
the **SignalR transport kept alongside the raw-WebSocket cutover**, plus the
explicitly-unwired legacy tester loops.

---

## Findings (ranked by size)

### 1. Legacy tester loops — `TesterDispatcherService` + `TesterRecoveryService` + `AddTesterLoops()` — **HIGH** (~216 lines)

- `src/Networker.ControlPlane/Background/TesterDispatcherService.cs` (whole file, 90 lines)
- `src/Networker.ControlPlane/Background/TesterRecoveryService.cs` (whole file, 126 lines; includes `TesterLoopsExtensions.AddTesterLoops` at lines 105–126)

Evidence: `grep -rn "AddTesterLoops\|TesterDispatcherService\|TesterRecoveryService" src tests`
→ hits only inside these two files plus one *comment* in `Program.cs:89-91`:

> "AddTesterLoops (legacy benchmark_config dispatch/recovery loops) is
> intentionally NOT wired — the unified C# schema has no benchmark_config
> table; the M3 dispatcher/redispatcher own run assignment."

The two `BackgroundService`s form a closed dead island (they reference live
code — `LeaderLockExtensions.KeyFor`, `TryRunGuardedAsync` — but nothing
references them). They query the M0-era `benchmark_config` / `project_tester`
tables that **do not exist** in the unified schema; if ever wired they would
throw. No test covers them. This matches the "legacy service loops still
pending" item tracked in the v0.28 known-bugs list. **Recommendation: delete
both files** (the doc comment in Program.cs can shrink accordingly).

### 2. Dual SignalR transport at `/hub/*` — **MEDIUM, deliberate keep, owner decision** (~600–950 lines entangled)

- `Program.cs:231-233` — `MapHub<BrowserHub>("/hub/dashboard")`, `MapHub<TesterQueueHub>("/hub/testers")`, `MapHub<AgentProtocolHub>("/hub/agent")`
- `src/Networker.ControlPlane/Realtime/AgentProtocolHub.cs` (182 lines)
- `src/Networker.ControlPlane/Realtime/BrowserHub.cs` (126 lines)
- `src/Networker.ControlPlane/Realtime/TesterQueueHub.cs` (289 lines)
- Plus bridging glue: `RawWs/RawWsTesterQueueLifetimeManager.cs` (143 lines), parts of `AgentProtocolExtensions.cs` (64), `EventBusServiceCollectionExtensions.cs` (110)

**No client anywhere connects to `/hub/*`:** `grep -ri "signalr|HubConnection|/hub/"`
across `dashboard/src`, `src/Networker.Agent`, and all of `sdk/` returns zero
client-side hits. The dashboard uses raw WS (`/ws/dashboard`, `/ws/testers` —
`dashboard/src/hooks/useTesterSubscription.ts:52`,
`usePhaseSubscription.ts:47`), and the C# agent uses raw WS `/ws/agent`
(`RawWebSocketClient`). The hubs are production-mapped endpoints with zero
traffic.

**Deliberate-keep evidence** (`Program.cs:228-230`):

> "The SignalR hubs stay available at /hub/* for future SignalR-native
> clients (e.g. the C# agent skeleton). Same underlying processors/registries
> — the two transports share seq streams, connection registry, and
> persistence."

**Entanglement warning:** the hub types are load-bearing beyond the mappings —
`EventBus.cs:52` holds `IHubContext<BrowserHub>`, `AgentConnectionRegistry.cs:63`
holds `IHubContext<AgentProtocolHub>`, and the raw-WS tester-queue fan-out is
implemented *as a decorator over SignalR's hub lifetime manager*
(`RawWsTesterQueueLifetimeManager`). Removing the hubs is a refactor, not a
deletion. There is also duplicated snapshot logic maintained in two places:
`TesterQueueHub.BuildSnapshotAsync` (lines 177–227) vs
`TesterQueueSocketEndpoint.BuildSnapshotAsync` (lines 283–336).

**Owner decision needed:** either commit to SignalR-native clients (then wire
finding #3), or retire the `/hub/*` mappings and invert the raw-WS layer to be
primary — which would also unlock #3 and the lifetime-manager decorator.

### 3. `TesterQueueBroadcaster` — registered singleton, never resolved — **HIGH as dead DI / MEDIUM disposition (possible feature gap)** (~57 lines)

- Definition: `src/Networker.ControlPlane/Realtime/TesterQueueHubExtensions.cs:87-142`
- Registration: same file, line 62 (`services.AddSingleton<TesterQueueBroadcaster>()`, via `AddTesterQueueHub()` called at `Program.cs:57`)

Evidence: `grep -rn "TesterQueueBroadcaster|NotifyQueueUpdateAsync|NotifyPhaseAsync" src tests`
→ zero injections, zero calls; only doc-comment mentions
(`RawWsTesterQueueLifetimeManager.cs:12`, `RawWsExtensions.cs:46`) and one
test comment (`RawWsBridgeTests.cs:120`).

**Why this matters beyond dead code:** this class is the *only* producer of
outbound `tester_queue_update` and `phase_update` messages
(`TesterQueueUpdateMessage`/`PhaseUpdateMessage` are constructed nowhere else
in `src/`), and `RawSocketRegistry.BroadcastTesterGroup` is only invoked by
the lifetime-manager mirror of hub group sends. Meanwhile the dashboard
actively subscribes to these message types
(`usePhaseSubscription.ts`, `useTesterSubscription.ts`). Structurally, the
live-delta pipeline has **no producer** — subscribers get the on-connect
snapshot and then rely on polling. Owner should decide: wire the broadcaster
into dispatch/provisioning (closing a realtime gap), or delete it and the
delta message records. Do not silently delete without that call.

### 4. `AgentVersion.FromAssembly` needlessly `public` — **HIGH (visibility fix, ~0–17 lines)**

- `src/Networker.Agent/AgentVersion.cs:29` — `public static string FromAssembly(Assembly assembly)`

Evidence: `grep -rn "FromAssembly" src tests sdk` → only self-use at line 21
(`Current` initializer). The two sibling implementations got this right:
`src/Networker.Endpoint/ServerInfo.cs:23` and
`src/Networker.ControlPlane/Endpoints/VersionEndpoints.cs:125` are both
`private static`. Demote to `private` (or inline).

### 5. Documented env vars never read — **docs cleanup**

| Var | Documented | Read in `src/` | Verdict | Confidence |
|-----|-----------|----------------|---------|------------|
| `DASHBOARD_CORS_ORIGIN` | `docs/setup-guide.md:736` | No (0 grep hits) | Intentionally deferred — `docs/analysis/csharp-fidelity-audit.md:76` marks it "intentional" (Kestrel binds via `ASPNETCORE_URLS`, nginx fronts static). Remove or annotate the setup-guide row. | HIGH |
| `DASHBOARD_HIDE_SSO_DOMAINS` | `docs/setup-guide.md:735` | No (0 grep hits) | Planned feature (`docs/superpowers/plans/2026-03-21-v014-auth-overhaul.md:479-481`), never implemented. Owner: implement or drop from docs. | MEDIUM |
| Rust-era vars (`DASHBOARD_BIND_ADDR`, `DASHBOARD_STATIC_DIR`, `SSO_MICROSOFT_*`, `SSO_GOOGLE_*`, …) | `docs/setup-guide.md:714-716` | No | Already correctly labeled `[LEGACY]` in docs — no action. | HIGH |

All other `Environment.GetEnvironmentVariable` / `Configuration[...]` reads in
`src/` trace to used values (including the undocumented-but-live
`DEPLOY_CONCURRENCY` in `TesterWriteEndpoints.Create.cs:208` and
`INSTALL_SH_PATH` in `DeployRunner.cs:299`).

### 6. Verified LIVE — do not remove (negative findings worth recording)

- **`AgentVersionGate`** (`src/Networker.ControlPlane/Dispatch/RunDispatcher.cs:793-813`,
  floor `"0.28.0"`): current version is 0.28.57 (`Cargo.toml:11`), but 0.28.0
  is the C#-era protocol baseline — pre-0.28.0 Rust agents silently drop
  `assign_run` (comment at lines 785–791). Active compatibility gate. Revisit
  only when the last pre-0.28.0 agent is retired.
- **All `*Extensions` DI classes** flagged by the type-name scan
  (`BackgroundServicesExtensions`, `CloudLifecycleExtensions`,
  `LeaderLockExtensions`, `ProvisioningExtensions`, `ReconciliationExtensions`,
  `VersionRefreshExtensions`, `VmLifecycleRecorderExtensions`,
  `HttpContextAuthExtensions`, …) are live via extension-method call sites in
  `Program.cs` and endpoints (method-granularity re-check: every public static
  method has ≥1 external caller).
- **~75 request/response records** with no cross-file references
  (`CreateProjectRequest`, `UrlTest*` views, `PrecheckRequest`, …) are all
  bound in same-file minimal-API handlers — live by ASP.NET type binding.
- **`SsoFlowEndpoints`** — mapped via `MapSsoEndpoints()` at `Program.cs:215`.
- **Networker.Data**: all 33 entities are DbSet-mapped; `SchemaMigrator`,
  `ProjectId36`, `V025ProjectIdMigration` all referenced. No dead helpers.
- **Test scaffolding**: every fixture (`ControlPlaneFixture`,
  `SchemaMigrationFixture`), fake (`FakeTime`, `FakeProviderClient`,
  `CollectingSink`, `WatchdogTickHarness`, …) is used via
  `IClassFixture<T>`/instantiation. Automated orphan scan over all
  test-declared types: zero hits.
- **`sdk/csharp`**: internal members (`ByteBudget`, `TokenBucket`,
  `ConcurrencyGate`, …) all consumed by `LagHoundMiddleware`. No
  internally-dead members.
- **Historic PoC hubs** (`AgentHub`/`DashboardHub` inline in Program.cs,
  referenced by `AgentProtocolExtensions.cs:24-44` doc comments): already
  deleted — cleanup completed; only stale doc comments remain (minor: refresh
  the comments in `AgentProtocolExtensions.cs` / `EventBusServiceCollectionExtensions.cs`
  that still narrate the supersession).

---

## Totals

| # | Candidate | Confidence | Est. lines |
|---|-----------|------------|-----------|
| 1 | Legacy tester loops (2 files) | HIGH | 216 |
| 2 | SignalR `/hub/*` transport (deliberate keep) | MEDIUM — owner decision | ~600 direct, ~950 with glue |
| 3 | `TesterQueueBroadcaster` dead DI (or unwired feature) | HIGH dead / MEDIUM disposition | 57 |
| 4 | `AgentVersion.FromAssembly` visibility | HIGH | 0–17 |
| 5 | Doc rows for unread env vars | HIGH/MEDIUM | docs only |

**Immediately removable with high confidence: ~275 lines** (items 1, 3, 4).
**Contingent on the SignalR owner decision: ~600–950 more.**

Re-run this survey with a Roslyn unused-symbol pass once a .NET 10 SDK is
installed; grep-based tracing cannot see dead *private* members, dead branches
inside live methods, or unused NuGet references.
