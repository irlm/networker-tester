# Cross-Project Data-Isolation RE-AUDIT — Networker.ControlPlane (LagHound)

**Date:** 2026-07-22 · **Method:** read-only, adversarial · **Scope:** the surfaces
that shipped *after* `docs/analysis/project-isolation-audit.md` (which found the §4
dispatch P0 — now verified fixed). Invariant under test: *a user or agent in project
A must NEVER read, write, or receive project B's data.*

**Verdict up front:** every one of the six requested surfaces is **ISOLATED** and
the prior P0 (cross-project SDK-token dispatch) is **fully closed** — verified in
code *and* by a dedicated test suite. One pre-existing, unrelated **P2 IDOR** on a
flat schedule-detail route (metadata only, no secret) is the sole open finding; it
is not in the six-surface scope but is reported for completeness.

## Summary table

| # | Surface | Verdict | Anchor |
|---|---------|---------|--------|
| 1 | SDK-endpoint CRUD | **ISOLATED** | `SdkEndpointsEndpoints.cs:123,134,140,151,156,167,176` |
| 2 | App-Network report | **ISOLATED** | `AppNetworkEndpoints.cs:108,134,174` |
| 3 | api_key rotate | **ISOLATED** | `TesterWriteEndpoints.RotateKey.cs:49,59`; route `TesterWriteEndpoints.cs:70-71` |
| 4 | RunDispatcher sdkprobe (prior P0) | **ISOLATED (fixed)** | `RunDispatcher.cs:274,365,612-624` |
| 5 | Real-client-IP / AgentAuthLimiter | **ISOLATED** | `AgentSocketEndpoint.cs:194-213`; `AgentAuthLimiter.cs` |
| 6 | perf-per-cost report | **ISOLATED** | `PerfPerCostEndpoints.cs:141,186` |
| — | flat `GET /api/v2/schedules/{id}` (out of scope) | **LEAK (P2 IDOR, metadata)** | `SchedulesEndpoints.cs:88-95` |

**Severity counts: P0 = 0, P1 = 0, P2 = 1 (pre-existing, out-of-scope, metadata-only).**

---

## 1. SDK-endpoint CRUD — **ISOLATED**

`SdkEndpointsEndpoints.cs`. Every handler is BOTH policy-gated AND row-filtered by
`ProjectId == projectId`:

| Handler | Policy | Row scoping |
|---|---|---|
| POST create | `ProjectOperator` (`:123`) | writes `ProjectId = projectId` (`:98`) |
| GET list | `ProjectMember` (`:140`) | `.Where(c => c.ProjectId == projectId)` (`:134`) |
| GET detail | `ProjectMember` (`:156`) | `c.Id == id && c.ProjectId == projectId` (`:151`) → 404 (`:153`) |
| DELETE | `ProjectOperator` (`:176`) | `c.Id == id && c.ProjectId == projectId` (`:167`) → 404 (`:170`) |

**Adversarial (IDOR):** a project-A member passing a project-B `{id}` to detail/delete
hits the compound `Id == id && ProjectId == projectId` predicate → no row → flat 404
(`:154`, `:170`), never project B's row. No existence oracle (404, not 403).

**Token never leaks / never decrypted on any read:** `ToDto` (`:265-286`) emits
`token_set` (bool) + `token = mask | null` (`:279-280`); the ciphertext columns
`TokenEnc`/`TokenNonce` are **absent** from the DTO. Create requires a token (`:87`),
encrypts immediately via `CredentialCipher.Encrypt` (`:92`) and stores only
ciphertext+nonce (`:108`). Grep confirms **the only `TokenEnc` read outside this file
is the dispatcher** (§4) — there is no GET path anywhere that returns the plaintext.

## 2. Application Network Performance report — **ISOLATED**

`AppNetworkEndpoints.cs` + `Reports/AppNetworkLogic.cs`. Policy `ProjectMember`
(`:108`). The raw Npgsql aggregation anchors on `WHERE r.project_id = $1` (`:134`,
`$1` = route projectId, bound param `:182`). The optional `config_id` is a **bound
param** (`:154`,`:185`) — no interpolation, no injection, no cross-project widening.
`RequestAttempt`/`ServerTimingResult` join off `test_run`, which is project-scoped.

**The prior P2 (test_config join without project_id) is CLOSED:** `LoadAggregatesAsync`
now joins `JOIN test_config c ON c.id = s.config_id AND c.project_id = $1` (`:174`,
with an explicit belt-and-suspenders comment `:168-173`). `LoadOverallAsync` (`:216`)
computes only medians and touches no config table, so it needs no such join. A
stray/mis-written `test_run` can no longer surface another project's config name. The
`LogInformation` (`:84-88`) logs projectId + config_id + anomaly count only — no rows,
no token.

## 3. api_key rotate — **ISOLATED**

Route `POST /api/projects/{projectId}/testers/{testerId}/rotate-key`, gated
`ProjectOperator` (`TesterWriteEndpoints.cs:70-71`). Handler
`TesterWriteEndpoints.RotateKey.cs`:

- Tester lookup is project-scoped: `t.ProjectId == projectId && t.TesterId == testerId`
  → flat 404 for missing/foreign (`:49-54`).
- **The agent row is ALSO project-scoped:** `a.TesterId == testerId && a.ProjectId ==
  projectId` (`:59`) → 404 if no same-project agent (`:61-63`). An operator can only
  ever rotate an agent in their own project — the exact requirement.

**Adversarial:** a project-A operator passing a project-B `testerId` fails the tester
predicate (not in A) → 404, before any agent is touched. Even if a tester id were
shared, the agent predicate re-asserts `ProjectId == projectId`. No cross-project
key rotation is reachable. The new plaintext key is returned once (`:81-87`) and never
re-serialized; the old hash is overwritten (`:66-68`) so the prior key dies instantly;
the live WS connection is dropped (`:73`). Operator-gated, doubly project-scoped.

## 4. RunDispatcher sdkprobe dispatch — **ISOLATED (prior P0 fixed)**

`RunDispatcher.cs`. The prior audit's P0 — `SelectTargetAgentAsync` selecting *any*
online agent regardless of project, leaking the decrypted LagHound token cross-tenant
— is fully remediated:

1. **Selection is project-scoped in the query:** `SelectTargetAgentAsync(string
   projectId, …)` filters `.Where(a => onlineIds.Contains(a.AgentId) && a.ProjectId ==
   projectId)` (`:365`). Both the tester-affinity branch (`:381-388`) and the
   `compatible[0]` fallback (`:390`) draw *only* from same-project agents. No
   same-project compatible agent → returns `null` (`:375`) → run **stays queued**
   (`TryAssignAsync` `:275-283`), never crosses projects.
2. **All call sites pass `run.ProjectId`:** inline dispatch (`TryAssignAsync:274`),
   redispatcher (`RedispatchQueuedAsync` → `TryAssignAsync`, `:200`), and even cancel
   fan-out (`:240`) route only to the run's own project.
3. **Token-attach guard (defense-in-depth) holds:** `SerializeForAssignAsync` splices
   `laghound_token` only when `AgentIsInProjectAsync(agentId, run.ProjectId)` AND
   `cfg.ProjectId == run.ProjectId` (`:612-616`). On mismatch it **refuses the token**,
   ships the run without it, and logs an error **without the plaintext** (`:619-624`).
   `AgentIsInProjectAsync` (`:400-410`) re-reads `agent.project_id` and compares
   ordinally.
4. **Regression tests exist:** `tests/Networker.ControlPlane.Tests/RunDispatcherProjectScopeTests.cs`
   — `ProjectA_sdkprobe_run_never_dispatched_to_projectB_agent_and_token_never_leaks`
   (`:277`), `..._dispatches_to_projectA_agent_with_token` (`:310`),
   `Token_withheld_when_config_project_differs_from_run_project` (`:348`), and
   `Redispatch_does_not_route_projectA_run_to_projectB_agent` (`:396`).

`run.project_id` itself is written from `cfg.ProjectId` at launch (`:84`), so the
run↔config project agreement the guard asserts is established at creation.

## 5. Real-client-IP / AgentAuthLimiter — **ISOLATED (no cross-tenant state)**

`AgentSocketEndpoint.ResolveClientIp` (`:194-213`): prefers nginx's single-valued
`X-Real-IP`, then the left-most `X-Forwarded-For` hop, then the socket peer. This
value feeds only (a) the per-IP brute-force limiter and (b) `api_key_last_used_ip`
audit stamping — **never** an authz decision. Auth is by hashed key
(`AuthenticateAsync`, constant-time hex compare) *before* the upgrade (`:87-100`);
a spoofed IP cannot grant access.

`AgentAuthLimiter` keys its sliding window purely on the source **IP string**
(`ConcurrentDictionary<string,…>`, `AgentAuthLimiter.cs:31`) — there is **no project
or tenant dimension in the limiter state**, so there is no per-tenant bucket that
could bleed across projects. Worst case of IP mis-attribution behind a proxy is a
shared rate-limit bucket (a self-DoS/availability nuance, explicitly acknowledged in
the `ResolveClientIp` doc-comment), **not** a data-isolation break: no project B data
is ever read, written, or received via this path. Limiter is process-local singleton,
consistent with the single-control-plane deployment.

## 6. perf-per-cost report — **ISOLATED**

`PerfPerCostEndpoints.cs`. Policy `ProjectMember` (`:141`). SQL anchors
`WHERE r.project_id = $1` (`:186`, bound `:194`); `project_tester` joins off
`r.tester_id` (`:179`), attempts off `r.id`. The `completedRuns` context count is
also `r.ProjectId == projectId` (`:126`). All rows scoped to the route's project. No
token, no cross-project widening (no user-controlled interpolated filter).

---

## Cross-cutting IDOR sweep (every flat `{id}` route re-checked)

Flat routes (no `{projectId}` in the path) correctly load the row first, then gate on
its owning project via `ProjectAccessChecker.HasRoleAsync`, mapping no-access → **404**
(non-oracle). Verified:

- `test-configs/{id}` GET (`TestConfigsEndpoints.cs:62`), PATCH (`:119`), DELETE
  (`:190`), launch (`:231`).
- `test-runs/{id}` detail (`:172`), attempts (`:227`), artifact (`:257`); cancel
  (`TestRunWriteEndpoints.cs:41`).
- `alert-channels/{id}` + `alert-rules/{id}` PATCH/DELETE/test (`AlertsEndpoints.cs:111,
  155,184,301,376`).
- `comparison-groups/{id}` detail (`:117`), launch (`:152`).
- schedules `{id}` PATCH (`SchedulesEndpoints.cs:111`), DELETE (`:144`), trigger (`:170`).

**`launch` `tester_id` validation — the prior P2 is CLOSED:** a pinned
`LaunchRequest.tester_id` is now rejected with 400 unless it belongs to the config's
project (`TestConfigWriteEndpoints.cs:236-251`, "tester_id does not belong to this
config's project"). A foreign tester id can no longer influence routing.

### The one open finding (out of scope, pre-existing) — **P2 IDOR, metadata-only**

`GET /api/v2/schedules/{id:guid}` (`SchedulesEndpoints.cs:88-95`) requires only
`RequireAuthorization()` — it loads the row by id and returns it **without any
row-level project-membership check** (unlike its sibling PATCH/DELETE/trigger, and
unlike the analogous `test-configs/{id}` GET which *does* check). Any authenticated
user (member of *any* project) can read another project's schedule DTO: `project_id`,
`test_config_id`, `cron_expr`, `timezone`, `last_run_id`, `next_fire_at`
(`ToDto` `:201-214`). No secret is exposed (no token, no result data), so the blast
radius is scheduling **metadata**, but it still violates the strict invariant and is a
genuine cross-project read (IDOR). The code comment at `:86-87` self-documents it as a
known "follow-up." **Fix:** mirror the sibling handlers — `!await access.HasRoleAsync(
ctx, row.ProjectId, ProjectRole.Viewer, ct)` → 404. This surface was not in the
requested six and is not a regression from the prior audit; flagged here for
completeness.

---

## Overall verdict

**ISOLATED.** All six re-audited surfaces both require the project auth policy AND
constrain every row by `ProjectId == projectId` (foreign ids 404, never return another
project's row); the encrypted SDK token is write-only on every read path and is
decrypted only at dispatch behind a same-project agent guard. The prior audit's **P0
is closed and test-covered**, and both prior P2s (app-network config-name join,
launch tester_id validation) are also closed. The only residual issue is a
pre-existing, out-of-scope **P2 metadata IDOR** on the flat schedule-detail GET.

### Findings requiring action

- **P2 (out of scope, pre-existing) — flat `GET /api/v2/schedules/{id}` lacks row-level
  project authz** (`SchedulesEndpoints.cs:88-95`): any authenticated user can read any
  project's schedule metadata. Add `access.HasRoleAsync(ctx, row.ProjectId,
  ProjectRole.Viewer, ct)` → 404 on no-access, matching its PATCH/DELETE/trigger
  siblings. No P0/P1.
