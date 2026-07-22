# Control-Plane Test-Coverage Survey — 2026-07

**Scope:** `src/Networker.ControlPlane` (ASP.NET minimal APIs, raw-WS hubs,
background services, provisioning, dispatch, auth, alerting, SSO, schema
migrator) and its tests in `tests/Networker.ControlPlane.Tests` (39 files, ~9k
LOC) + `tests/Networker.Tests` (13 files, ~2.8k LOC).

**Method — STRUCTURAL, not measured.** The solution targets **.NET 10**; the
local SDK is **.NET 8 only**, so I could **not build or run coverage**. This is
a read-of-tests-against-source risk map, not a line-%. All "tested / strong /
thin / none" verdicts are inferred from reading the test bodies and matching
them to source. Treat counts as approximate.

**Headline verdicts**
- **The crown-jewel risk paths are genuinely well tested** — project-scoped run
  dispatch + token leak, the Azure delete cascade, the schema migration chain,
  agent api-key auth, and the `ProjectAccessChecker` IDOR core all have strong,
  behavior-asserting tests. This area is in far better shape than a typical
  "prod backend."
- **The biggest *systemic* strength (not weakness) is DB fidelity:** the
  `Networker.Tests` HTTP-integration suite runs against **real Postgres via
  Testcontainers** (WebApplicationFactory), so it *does* catch the EF
  `.Select()`-projection / raw-SQL / JSONB class of bug. The SQLite-only tests
  are confined to `Networker.ControlPlane.Tests` unit-level logic where that
  class of bug largely can't hide (see §5).
- **The real gap is breadth of HTTP-level authz/isolation tests + untested
  background services.** Only ~4 endpoint modules have negative authz coverage
  through the real host; the other ~40 rely on a *shared* `ProjectAccessChecker`
  whose *core* is unit-tested but whose *per-route wiring* is not
  independently asserted. And ~8 background services that write DB state or
  touch cloud have **no** behavioral test.

---

## 1. Risk-weighted area map

| Area | Component | Tested? | Strength | Notes |
|---|---|---|---|---|
| **Dispatch** | `RunDispatcher` (project scope + token) | Yes | **Strong** | `RunDispatcherProjectScopeTests` (429 LOC) + `ProjectIsolationDispatchTests` (real PG). Asserts cross-project run is NOT dispatched, plaintext token NEVER in a foreign-project frame, defense-in-depth mismatch guard, redispatch scoping. |
| **Dispatch** | `RunDispatcher` FK / tester binding | Yes | Strong | `RunDispatcherTesterFkTests` (551 LOC). SQLite-relational, real `ExecuteUpdateAsync` paths. |
| **Provisioning** | `CliComputeProvisioner.DeleteAsync` cascade | Yes | **Strong** | `CliProvisionerDeleteCascadeTests` (404 LOC). Fake-CLI argv log: VM→IP→NSG order, "already gone" tolerance, hard-fail skips cascade, in-use retry, scope-from-resource-id, `IsRetryableInUse`/`ParseAzureScope`. Guards #419 (never `nsg list`). |
| **Provisioning** | `CliComputeProvisioner` create argv | Yes | Strong | `CliProvisionerCreateArgsTests` (Windows path too). |
| **Provisioning** | `TesterCreateLogic`, `TesterState`, `CloudInitScripts`, `SshLanguageDetector`, `VmLifecycleRecorder`, `TesterInstallScripts` | Yes | Strong | Pure logic each has a dedicated file. |
| **Provisioning** | `DeployRunner`, `ProvisioningOrchestrator`, `TesterRecovery` | **No** | **None** | Orchestration + cloud-CLI exec + retry paths untested. |
| **Migration** | `SchemaMigrator` (V002–V045) | Yes | **Strong** | `SchemaMigrationTests` (663 LOC, real PG): full chain applies, rerun no-op, `_migrations` bookkeeping parity with Rust, **every EF DbSet queries clean** (schema-equivalence proxy), write round-trip, per-version column/index assertions, V040 SQL-backfill digest == C# hasher, **frozen SHA-256 per shipped script**, ProjectId36/Damm parity. |
| **Auth** | `ProjectAccessChecker` (IDOR core) | Yes | **Strong** | `ProjectAccessCheckerTests`: missing/soft-deleted project denies even platform-admin appropriately, non-member → no role, membership role mapping. This is the shared decision core behind ~all `{projectId}` routes. |
| **Auth** | `JwtTokenService`, `LoginRateLimiter` | Yes | Strong | Roundtrip, wrong-secret reject, expiry, Rust interop, per-IP block. |
| **Auth** | `UserStatusMiddleware`, `AuthRepository`, `GlobalRoleAuthorization`, `ProjectScopeAuthorization` (policy handlers) | **No** | **None** | Status gating (pending/disabled/must_change) + the handler-level wiring untested. |
| **Realtime** | `AgentConnectionRegistry`, `EventBus`, `RawSocketRegistry`, `AgentMessageProcessor`, `AgentAuthLimiter`, `AgentRawSocketRegistry` | Yes | Strong | Frame codec, stale-reconnect race, backpressure/eviction, per-IP block, seq/replay. |
| **Realtime** | `AgentProtocolHub`, `BrowserHub`, `TesterQueueHub`, `TesterQueueRegistry`, `*SocketEndpoint`, `RawWsTesterQueueLifetimeManager` | **No** | **None** | Only `RawWebSocketIntegrationTests` (dashboard connect+auth-reject) touches the *browser* WS end-to-end. Tester-queue + agent-hub endpoints untested at endpoint level. |
| **Alerting** | `AlertEvaluator`/`AlertRuleLogic`/`AlertWebhook`/`AlertNotifier` | Yes | Strong | `AlertRuleLogicTests`, `AlertWebhookTests`, + `AlertingEndpointsTests` (real PG: RBAC, secret masking, 409 on referenced-channel delete, test-fire). |
| **SSO** | `OidcFlowService`, `SsoExchangeCodeCache`, `AccountSecurity` | Yes | Strong | Endpoint resolution, CSRF state, iss/aud validation, single-use code, reset-token hashing/expiry. |
| **SSO** | `SsoFlowEndpoints`, `SsoProviderValidation` | **No** | **None** | The actual OAuth callback/exchange **HTTP routes** and provider-config validation untested. |
| **Background** | `TickMonitor`, `ScheduleTiming`, `VersionRefreshService`, `InactivityService` (policy), `OrphanReaperService` (scope) | Yes | Strong | Dedicated files; reaper scope is SQLite-3-table (adequate for its query). |
| **Background** | `AutoShutdownService`, `ReaperService`, `SchedulerService`, `TesterDispatcherService`, `TesterRecoveryService`, `WatchdogService`, `QueuedRunRedispatchService`, `LeaderElection` | **No** | **None** | All write DB state and/or gate cloud actions; none behaviorally tested. |
| **Notifications** | `AcsEmailSender` | Partial | **Thin** | `EmailSenderTests` covers env-driven selection + no-op logging only; real ACS send path unexercised. |
| **Security** | `AgentApiKeys` (hash/fixed-time), `CredentialJson`, `SecurityHeaders` | Yes | Strong | `AgentApiKeyAuthTests`, `CredentialJsonTests`, `SecurityHeadersTests`. |

---

## 2. Existing test-file → coverage map (high-signal files)

- `RunDispatcherProjectScopeTests.cs` — **P0 token-leak / cross-project dispatch.** Records exact wire frames per agent; asserts routing + payload. Strong.
- `RunDispatcherTesterFkTests.cs` — tester FK binding, `ExecuteUpdateAsync` paths. Strong.
- `CliProvisionerDeleteCascadeTests.cs` — **delete cascade / wrong-resource safety.** Fake-CLI invocation log. Strong.
- `OrphanReaperScopeTests.cs` — reaper account-scope + `name_is_ours` allow-list + NSG argv. Strong (SQLite, 3 tables only — fine for its scope query).
- `SchemaMigrationTests.cs` + `MigrationScriptFreezeTests` — **migration correctness + immutability.** Real PG + frozen checksums. Strong.
- `ProjectAccessCheckerTests.cs` — **IDOR decision core.** Strong (pure).
- `AgentApiKeyAuthTests.cs` — hash-at-rest, constant-time compare, hash-only lookup. Strong.
- `ControlPlaneIntegrationTests.cs` / `ProjectIsolationDispatchTests.cs` / `AlertingEndpointsTests.cs` / `SdkEndpointsTests.cs` / `PerfPerCostReportTests.cs` / `AppNetworkReportTests.cs` — **real-Postgres HTTP integration**, incl. 401/403/foreign-member negatives, encryption-at-rest, hand-computed aggregation. Strong — but only cover a handful of endpoint modules.

---

## 3. Top under-tested paths (ranked by blast radius)

1. **`SsoFlowEndpoints` — OAuth callback/exchange routes (`Sso/SsoFlowEndpoints.cs`).** No HTTP test. Bug class: mishandled state/nonce or code-exchange → **account takeover / auth bypass**. *Test:* boot host, drive callback with mismatched `state` → 400, and a valid single-use code → session issued once, replay → 401.
2. **`UserStatusMiddleware` (`Auth/UserStatusMiddleware.cs`).** No test. Bug: a `disabled`/`pending`/`must_change_password` user still reaches mutating routes → **revoked user retains access**. *Test:* disabled user's JWT → 403 on a protected route.
3. **`AutoShutdownService` + `ReaperService` (`Background/`).** No behavioral test; both **delete/deallocate cloud VMs** off DB queries. Bug: mis-scoped sweep deallocates a *live* tester or another account's VM. *Test:* seed running + due-for-shutdown testers across two accounts → only the due, in-scope one is acted on.
4. **`TesterDispatcherService` (`Background/`).** No test (distinct from `RunDispatcher`). Bug: mis-assign a run to an offline/foreign tester → **run lost or cross-project execution**. *Test:* queued run + one eligible + one foreign tester → assigned only to eligible.
5. **`LeaderElection` (`Background/`).** No test. Bug: two instances both win the lock → **double-dispatch / double cloud provisioning**. *Test:* two contenders on one lock key → exactly one `IsLeader`.
6. **`QueuedRunRedispatchService` (`Background/`).** No dedicated test (scope covered inside `RunDispatcherProjectScopeTests.Redispatch_*`). Bug: redispatch resurrects a cancelled/finished run or crosses projects. *Test:* cancelled + queued-foreign runs are skipped.
7. **`DeployRunner` / `ProvisioningOrchestrator` (`Provisioning/`).** No test. Bug: non-zero CLI exit or partial failure leaves a **half-provisioned VM with no DB row → leak**. *Test:* fake CLI fails mid-flow → tester row not left in `running`, resource recorded for reaper.
8. **`SsoProviderValidation` (`Sso/`).** No test. Bug: accepts a malformed/hostile issuer/redirect config → **open redirect / token to attacker IdP**. *Test:* reject non-https / wildcard redirect / missing issuer.
9. **`UrlTestsEndpoints` detail routes (`Endpoints/UrlTestsEndpoints.cs`, `GET .../url-tests/{run_id}`, `.../sections`).** Detail routes load by `run_id` **without** project scoping (deliberate Rust parity, but unguarded). Bug: **cross-project run detail leak** if a `run_id` is guessed/leaked. *Test:* foreign-project run_id → 404 (or confirm the parity decision is intended and documented).
10. **`ProjectScopeAuthorization` / `GlobalRoleAuthorization` policy handlers (`Auth/`).** No handler-level test. Bug: a policy mis-wire silently authorizes. The *core* (`ProjectAccessChecker`) is tested, but the handler that *invokes* it is not. *Test:* handler denies non-member, allows platform-admin, honors soft-delete.
11. **`AgentProtocolHub` / agent-socket endpoint auth (`Realtime/RawWs/AgentSocketEndpoint.cs`).** Frame codec + limiter are tested, but the **endpoint's connect-time authentication** is not exercised end-to-end. Bug: an unauthenticated/foreign-key agent connects and receives assign_run. *Test:* bad api-key handshake → closed, no registry entry.
12. **`TesterQueueSocketEndpoint` / `RawWsTesterQueueLifetimeManager`.** No test. Bug: a tester subscribes to a **foreign project's queue** and sees its runs. *Test:* subscribe with project-A creds, publish project-B item → not delivered.
13. **`MembersEndpoints` / `InvitesEndpoints` / `CloudConnectionsEndpoints` mutations.** Isolation is *implemented* (query filters, verified by the endpoint inventory), but there is **no automated foreign-id→404 test** through the real host for these specific modules. Bug: a future refactor drops the `project_id` filter unnoticed. *Test:* one foreign-id 404 assertion per mutating route (see §4).
14. **`AcsEmailSender` real send path.** Thin. Bug: invite/reset emails silently fail in prod. *Test:* a fake ACS transport asserts payload/recipient; error → surfaced, not swallowed.
15. **`Notifications`/alert `AlertNotifier` webhook-failure retry** (webhook *shape* tested, delivery-failure behavior less so). Bug: a flapping webhook wedges the evaluator. *Test:* notifier tolerates a 500 and continues.

---

## 4. Isolation/authz coverage per mutating endpoint (foreign-id → 404?)

**Implemented (per source inventory) — but which have an *automated* negative test?**

Every project-scoped mutating module **filters on `project_id` or calls
`ProjectAccessChecker`** in source (good). The gap is *test* coverage of that
wiring through the real host:

- **Has a real-host foreign/unauthorized negative test:** `AlertsEndpoints`
  (viewer-write 404, cross-project), `SdkEndpointsEndpoints` (delete
  missing/foreign → 404), `TestConfigWriteEndpoints`/`TestRunWriteEndpoints`
  launch (foreign tester → 400 via `ProjectIsolationDispatchTests`), plus the
  generic 401/403 member gates in `ControlPlaneIntegrationTests`.
- **Isolation implemented but NO dedicated automated foreign-id→404 test:**
  `TesterWriteEndpoints.*` (start/stop/force-stop/upgrade/delete/rotate-key/
  schedule), `ProjectWriteEndpoints` (PUT/DELETE), `DeploymentWriteEndpoints`,
  `MembersEndpoints`, `InvitesEndpoints`, `CloudAccountsEndpoints`,
  `CloudConnectionsEndpoints`, `SchedulesEndpoints`, `ShareLinksEndpoints`,
  `ComparisonGroupsEndpoints`, `AppNetworkEndpoints`, `ApprovalsEndpoints`,
  `VisibilityRulesEndpoints`, `SsoAdminEndpoints`, `AdminEndpoints`.

Net: correctness rests on the (well-tested) *shared* checker; the *per-route
attachment* of that check is asserted for only ~4 modules. A dropped filter in
any of the above would not be caught by the current suite.

**`UrlTestsEndpoints` detail** is the one place isolation is *not* implemented
(by design, Rust parity) — flagged as gap #9.

---

## 5. Weak-test flags

- **SQLite-only unit tests** (`RunDispatcher*`, `OrphanReaperScope`,
  `AgentApiKeyAuth`) hand-roll a minimal schema and exercise pure/relational
  logic. They **would not** catch Postgres-specific EF projection / JSONB /
  raw-SQL bugs — **but** those risks are separately covered by the
  real-Postgres `Networker.Tests` integration suite and `SchemaMigrationTests`.
  So the SQLite tests are acceptable *for what they assert*; the systemic risk
  is that new **Postgres-specific query behavior in an untested endpoint** has
  no net under it.
- **Thin:** `RegressionAnalyzerTests` (emission shape only, no comparison
  logic), `EmailSenderTests` (selection/no-op only).
- **Happy-path-only:** `SecurityHeadersTests` (no negative cases — acceptable,
  headers are static).
- **No fault-injection** on background loops: none of the reaper/shutdown/
  dispatcher services are tested for the "partial failure / cancellation /
  second-instance" paths where they do damage.

---

## 6. Add these tests first (prioritized)

1. **`UserStatusMiddleware`** — disabled/pending user → 403 on a protected route. *(auth bypass, cheap)*
2. **`SsoFlowEndpoints`** — state-mismatch → 400; single-use code replay → 401. *(account takeover)*
3. **`AutoShutdownService` + `ReaperService`** — cross-account/live-tester scope: only the in-scope, due resource is acted on. *(wrong-VM delete)*
4. **`TesterDispatcherService` + `LeaderElection`** — single-assignment + single-leader under contention. *(double-dispatch)*
5. **One foreign-id→404 test per mutating endpoint module** listed in §4 (a
   parametrized real-host fixture) — the highest-leverage breadth fix; freezes
   the isolation wiring the whole app depends on.
6. **`UrlTestsEndpoints` detail route** — decide + test: foreign run_id → 404, or
   document the parity exception.
7. **`DeployRunner`/`ProvisioningOrchestrator`** — mid-flow CLI failure leaves no
   orphaned VM / leaves a reaper-recoverable record.
8. **Agent + tester-queue socket endpoints** — connect-time auth + cross-project
   subscription isolation.

---

## Bottom line

The **highest-consequence paths are well defended** (dispatch token isolation,
delete cascade, migrations, api-key auth, IDOR core), and the integration suite
runs on **real Postgres**, so the feared SQLite-vs-Postgres fidelity gap is
**mostly closed for tested endpoints**. The **biggest systemic weakness** is
*coverage breadth*, not fidelity: (a) ~8 background services that mutate DB /
gate cloud actions have **no behavioral test**, and (b) per-endpoint
authz/isolation is *implemented* but automatically *verified* for only ~4 of
~40 modules — everything else trusts a shared checker whose route-level
attachment is unguarded. A single dropped `project_id` filter or a mis-scoped
reaper sweep is the most likely way a real bug reaches prod today.
