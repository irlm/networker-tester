# Project-Isolation Security Audit — Networker.ControlPlane (LagHound)

**Scope:** Multi-tenant control plane serving prod `laghound.com`. Invariant under
test: *a user or agent in project A must NEVER read, write, or receive project B's
data* — with special weight on the LagHound SDK feature, which stores
**AES-256-GCM-encrypted customer tokens** (`test_config.token_enc`/`token_nonce`)
and hands the decrypted plaintext to a remote agent at dispatch time.

**Method:** read-only, adversarial — assume a malicious member of project A probing
for project B's data/tokens, and a hostile/compromised agent bound to project B.

**Verdict up front:** the *HTTP/API surface* is strictly project-scoped and the
authz linchpin is sound. **One P0** exists in the run-dispatch path: target-agent
selection is not project-scoped, so a project-A `sdkprobe` run — carrying the
decrypted project-A LagHound token — can be dispatched to an agent bound to a
different project. This is a cross-project **token leak** to the executing agent.
Everything else is ISOLATED or defense-in-depth (P2).

---

## Summary table

| # | Path | Verdict |
|---|------|---------|
| 1 | Authz policies (`Auth/*`, `AuthPolicies`, `ProjectRoleHandler`, `ProjectAccessChecker`) | **ISOLATED** |
| 2 | SDK-endpoint CRUD (`SdkEndpointsEndpoints.cs`) | **ISOLATED** |
| 3 | App-network report (`AppNetworkEndpoints.cs` + SQL) | **NEEDS-REVIEW** (P2 defense-in-depth; no active leak) |
| 4 | Dispatch + token decryption (`RunDispatcher.cs`) | **LEAK (P0)** — agent-selection is not project-scoped |
| 5 | Agent identity / WS auth (`AgentSocketEndpoint`, `AgentMessageProcessor`) | **ISOLATED** for auth; inherits #4 for work-routing |
| 6a | Perf-per-cost report (#486) | **ISOLATED** |
| 6b | Cross-cutting IDOR (flat routes, list queries) | **ISOLATED** |
| 6c | `LaunchRequest.tester_id` not validated to config's project | **NEEDS-REVIEW (P2)** |
| 6d | Role gating vs. role-based-filtering requirement | **ISOLATED** (viewer read / operator write enforced server-side) |

Severity counts: **P0 = 1, P1 = 0, P2 = 2** (plus one P2 defensive gap in #3).

---

## 1. The authz policies — the linchpin — **ISOLATED**

Files: `Auth/AuthExtensions.cs`, `Auth/ProjectScopeAuthorization.cs`,
`Auth/ProjectAccessChecker.cs`, `Auth/AuthRepository.cs`, `Auth/AuthUser.cs`,
`Auth/JwtTokenService.cs`, `Auth/Role.cs`.

**What the JWT carries.** `JwtTokenService.CreateToken`
(`JwtTokenService.cs:74-96`) emits only identity claims: `sub`, `email`, `role`,
`is_platform_admin`, `exp`, `iat`. **No project_member claims are in the token.**
This is the correct design: there is no per-project grant baked into a bearer token
that could go stale or be replayed after a membership change.

**How the project role is actually enforced.** `ProjectRoleHandler`
(`ProjectScopeAuthorization.cs:26-59`) reads the `{projectId}` **route** value
(line 42), then calls `ProjectAccessChecker.GetRoleForProjectAsync(httpContext,
projectId, …)` (line 50) and only succeeds when
`role.HasPermission(requirement.MinRole)` (line 53). The effective role is resolved
**fresh from the DB per request** by `AuthRepository.GetMemberRoleAsync`
(`AuthRepository.cs:124-133`):

```sql
SELECT role FROM project_member WHERE project_id = @pid AND user_id = @uid
```

Both parameters are bound: `@pid` = the **route's** projectId, `@uid` = the JWT's
`sub`. There is **no path where "member of SOME project" satisfies the check** — the
query is keyed on the exact project in the URL. `ResolveEffectiveRole`
(`ProjectAccessChecker.cs:79-93`) is the pure decision core: project must exist;
soft-deleted → no access unless platform admin; platform admin → implicit Admin;
otherwise the caller's `project_member.role` (null = not a member = no access).

**Adversarial check — can a project-A member pass a project-B projectId and win?**
No. The role is looked up for `(projectB, callerUserId)`; a non-member yields `null`
→ `context.Succeed` never called → 403. Membership is not transferable across
projects and is never read from the token.

**Flat routes** (no `{projectId}`) use the same engine via
`ProjectAccessChecker.HasRoleAsync`, loading the row first and mapping no-access to
**404, not 403** (documented at `ProjectAccessChecker.cs:13-25`) so resource
existence is not leaked. Verified in the launch and delete handlers
(`TestConfigWriteEndpoints.cs:190,231`).

**JWT signature.** HS256 with raw-HMAC compare in constant time
(`JwtTokenService.cs:120-139`, `CryptographicOperations.FixedTimeEquals`); secret
fails **closed** outside Development (`AuthExtensions.cs:35-42`). Sound.

**Platform-admin bypass** is intentional and consistent with the Rust original
(implicit Admin on every project). It is a *global* super-user, not a cross-project
leak between ordinary tenants; acceptable but worth noting as the one identity that
transcends project scope.

Verdict: **ISOLATED.** The linchpin enforces "member of *this* project with the
required role," resolved live from `project_member`, never from the token.

---

## 2. SDK-endpoint CRUD — **ISOLATED**

File: `SdkEndpointsEndpoints.cs`.

Every handler is both policy-gated **and** row-filtered by `ProjectId == projectId`:

| Handler | Policy | Row scoping |
|---|---|---|
| `POST …/sdk-endpoints` | `ProjectOperator` (`:123`) | writes `ProjectId = projectId` (`:98`) |
| `GET …/sdk-endpoints` (list) | `ProjectMember` (`:140`) | `.Where(c => c.ProjectId == projectId)` (`:134`) |
| `GET …/sdk-endpoints/{id}` | `ProjectMember` (`:156`) | `c.Id == id && c.ProjectId == projectId` (`:151`) |
| `DELETE …/sdk-endpoints/{id}` | `ProjectOperator` (`:176`) | `c.Id == id && c.ProjectId == projectId` (`:167`) |

A foreign or non-existent config id therefore **404s** — the detail read returns
`ApiError.NotFound` (`:153-154`) and delete returns `Results.NotFound()` (`:170`),
never another project's row.

**Token write-only guarantee — confirmed.** `ToDto` (`:265-286`) never emits the
plaintext or ciphertext: it returns `token_set` (bool) and `token = mask | null`
(`:279-280`). `token_enc`/`token_nonce` are not in the DTO at all. There is **no
GET endpoint anywhere that returns the decrypted token** (the only decrypt call site
is the dispatcher, §4). Create requires a token (`:87-90`), encrypts immediately
with `CredentialCipher.Encrypt` (`:92`), and stores only ciphertext + nonce
(`:107-108`).

Verdict: **ISOLATED.** CRUD is doubly-scoped and the token is write-only on the API.

---

## 3. App-network report `GET …/reports/app-network` — **NEEDS-REVIEW (P2, no active leak)**

Files: `AppNetworkEndpoints.cs`, `Reports/AppNetworkLogic.cs`.

Policy: `ProjectMember` (`AppNetworkEndpoints.cs:108`). Good — member-read, any role.

The raw Npgsql aggregation (`BaseCte`, `:125-149`) is the sensitive part. Its
scoping anchor is:

```sql
FROM test_run r
JOIN RequestAttempt a ON a.RunId = r.id AND a.Success
JOIN ServerTimingResult st ON st.AttemptId = a.AttemptId
WHERE r.project_id = $1                      -- ← the route projectId (bound param)
  AND r.status = 'completed'
  AND LOWER(a.Protocol) = 'sdkprobe'
  ...
  {CONFIG_FILTER}                            -- AND r.test_config_id = $2 (bound)
```

`$1` is the **route** projectId (`:176`), and the optional `config_id` filter is a
**bound parameter** (`:154`, `:179`) — no string interpolation of user input, so no
injection and no cross-project widening via `config_id`. The attempt/timing joins
hang off `test_run`, which is project-scoped. So **runs and attempts are correctly
constrained to the route's project.**

**The gap (P2, defense-in-depth):** the final projection joins the config name via
`JOIN test_config c ON c.id = s.config_id` (`:168`) **without** re-asserting
`c.project_id = $1`. This does not currently leak, because `test_run.project_id` is
written from `cfg.ProjectId` at launch time (`RunDispatcher.cs:83`), so a run's
project always equals its config's project — the `config_id` in `split` already
belongs to `$1`'s project. But this is an *invariant assumption*, not an enforced
constraint: if a bug or manual DML ever produced a `test_run` whose `project_id`
disagrees with its `test_config.project_id`, this join would surface the other
project's **config name** in the group output. The token is never touched here, so
the blast radius is a config *name*, not a token.

**Fix:** add `AND c.project_id = $1` to the `test_config` join in both
`LoadAggregatesAsync` (`:168`) and — for parity — anywhere the report widens. Cheap,
and it turns an assumption into an enforced predicate.

The `LogInformation` at `:84-88` logs `projectId` + `config_id` + an anomaly count
only — no token, no row data. Fine.

Verdict: **NEEDS-REVIEW** — no active cross-project leak (runs/attempts are scoped),
but the `test_config` join should carry `project_id` as belt-and-suspenders (P2).

---

## 4. Dispatch + token-decryption path — **LEAK (P0)**

File: `RunDispatcher.cs`. This is the highest-risk path and it has the audit's only
cross-project data-flow defect.

**The token decrypt + delivery flow.** For a `sdkprobe` run,
`SerializeForAssignAsync` (`:537-603`) decrypts the stored LagHound token and splices
the **plaintext** into the wire payload:

```
IsSdkProbeWorkload(workloadJson) && cfg.TokenEnc … && cfg.TokenNonce …   (:560)
  → WithLagHoundToken(workloadJson, cfg.TokenEnc, cfg.TokenNonce)         (:562)
      → token = UTF8(_cipher.Decrypt(tokenEnc, tokenNonce))              (:666)
      → obj["laghound_token"] = token                                    (:691)
```

The decrypted token then rides inside the `assign_run` envelope to whatever agent
`TryAssignAsync` selected. **The stored row is never mutated (copy-on-write), and the
token is never logged** (`WithLagHoundToken` logs only an exception *type* on
decrypt failure, `:672-675`; `SerializeForAssignAsync` logs nothing) — token-redaction
in logs is correctly handled (audit item 4c: PASS).

**The defect — target selection ignores project.** The agent that receives the
payload is chosen by `SelectTargetAgentAsync` (`:333-370`):

```csharp
var online   = _agents.OnlineAgents();                       // global set, no project
var rows     = _db.Agents.Where(a => onlineIds.Contains(a.AgentId))
                         .Select(a => new { a.AgentId, a.Version, a.TesterId });
var compatible = rows.Where(a => AgentVersionGate.IsCompatible(a.Version));

if (preferredTesterId is Guid tid) {                          // tester AFFINITY only
    var bound = compatible.FirstOrDefault(a => a.TesterId == tid);
    if (bound is not null) return bound.AgentId;
}
return compatible[0].AgentId;                                 // ← ANY online agent
```

There is **no filter on `agent.ProjectId == run.ProjectId`** anywhere in this method,
in `TryAssignAsync` (`:262-318`), in `DispatchAsync` (`:115-150`), or in
`RedispatchQueuedAsync` (`:153-214`). `AgentConnectionRegistry.OnlineAgents()`
(`AgentConnectionRegistry.cs:133`) returns the global connection set with no project
notion. The `Agent` entity **does** carry `ProjectId` (`Agent.cs:39`) — it is simply
never consulted at selection time.

**Consequence (adversarial).** When no agent is bound to the run's specific
`tester_id` (the common case for a plain SDK endpoint, which has no pinned tester),
the code falls through to `compatible[0]` — the first online, version-compatible
agent **regardless of project**. If project B has an online agent and project A does
not (or A's tester-affine agent is offline), a project-A `sdkprobe` run is dispatched
to **project B's agent, delivering project A's decrypted LagHound customer token to a
machine controlled by a different tenant.** Project B's agent operator can read the
`X-LagHound-Token` from the `assign_run` frame / process args. This is a **P0
cross-project secret leak**.

Even the "affinity" branch does not save it: affinity is only a *preference*, and
`preferredTesterId` (from `LaunchRequest.tester_id`) is itself not validated to
belong to the run's project (see §6c), so it cannot be relied on as a scoping
mechanism.

Audit sub-items:
- (a) run-for-project-A only to an agent bound to A — **FAILS**: no such constraint.
- (b) decrypted token never sent to another project's agent — **FAILS**: it can be.
- (c) token redacted in logs — **PASS**.
- (d) an agent can't request an arbitrary foreign config — **PASS**: agents never
  *pull* configs; the control plane *pushes* `assign_run`. An agent cannot ask for a
  config id. The exposure is entirely on the push side (a/b).

**Note on provenance:** the Rust original was the same shape
(`provisioning.rs` `any_online_agent` / `any_online_agent_min_version` with no
project predicate), so this is a pre-existing architectural property carried into
C#, not a C# regression. It becomes **critical now** because the SDK feature is the
first workload that ships a **decrypted per-project secret** in the dispatch payload;
before, a mis-routed run only wasted the wrong VM's cycles.

**Fix (required, P0):**
1. In `SelectTargetAgentAsync`, constrain the candidate set to agents whose
   `ProjectId == run.ProjectId` (thread `run.ProjectId` in — `TryAssignAsync` already
   has `run`). Both the affinity branch and the `compatible[0]` fallback must draw
   only from that project's agents. If no compatible **same-project** agent is
   online, leave the run **queued** (return `null`) rather than crossing projects.
2. Defense-in-depth: in `SerializeForAssignAsync`, assert `cfg.ProjectId ==
   run.ProjectId` and the selected `agent.ProjectId == run.ProjectId` immediately
   before splicing the token; refuse to attach `laghound_token` otherwise.
3. Add an integration test: project-A `sdkprobe` run with only a project-B agent
   online must **not** dispatch (stays queued) and must **never** emit
   `laghound_token` to the project-B agent.

Verdict: **LEAK (P0).**

---

## 5. Agent identity / WS auth — **ISOLATED (auth); inherits §4 for routing)**

Files: `Realtime/RawWs/AgentSocketEndpoint.cs`, `AgentMessageProcessor.cs`,
`Security/AgentApiKeys.cs`.

**Every agent row is project-scoped** (`Agent.ProjectId`, `Agent.cs:39`).

**WS auth is sound.** `GET /ws/agent?key=…` authenticates *before* the upgrade
(`AgentSocketEndpoint.cs:71-88`): `AuthenticateAsync` (`AgentMessageProcessor.cs:132-154`)
hashes the presented key to SHA-256 hex, looks up `agent.api_key_hash`, then
re-verifies with a constant-time compare (`AgentApiKeys.FixedTimeEqualsHex`). Unknown
key → HTTP 401, no upgrade. The plaintext column is never used for the lookup.

**Can an authenticated agent receive work for a project it isn't bound to?**
The agent has **no pull path** — it cannot request a config or a run; it only
receives what the control plane pushes. So an agent cannot *ask* for foreign work.
However, because §4's push-side selection ignores project, the control plane **can
mis-push** a foreign project's run (and its token) to this agent. The *auth* is
isolated; the *work-routing* into the agent inherits the §4 P0. Nothing in the
inbound message handlers (`OnRunStarted`/`OnRunFinished`/etc.) re-checks that the run
belongs to the agent's project — they key on `run_id`/`worker_id` only — which is
acceptable *if* selection is fixed, but today compounds §4 (a mis-routed agent
happily drives the foreign run to completion and writes its results).

Verdict: **ISOLATED** for authentication and for the "agent can't self-select foreign
work" property; the residual risk is entirely the §4 push-side selection bug.

---

## 6. Cross-cutting checks

### 6a. Perf-per-cost report (#486) — **ISOLATED**
`PerfPerCostEndpoints.cs`: `ProjectMember` policy (`:39`), and the SQL
(`:179-186`) filters `WHERE r.project_id = $1` (bound param) with the
`project_tester` join hanging off `r.tester_id`. `$1` is the route projectId. Runs,
attempts, and tester groups are all scoped to the route's project. No token is
involved. Sound.

### 6b. IDOR on flat `config_id`/`run_id`/`tester_id` routes — **ISOLATED**
Spot-checked the flat write routes: `POST …/test-configs/{id}/launch` loads the
config's `ProjectId` and requires `Operator` on **that** project before acting
(`TestConfigWriteEndpoints.cs:227-234`); `DELETE …/test-configs/{id}` does the same
(`:186-193`); `TestRunWriteEndpoints.cs:39` resolves the run's project for the same
gate. All map no-access → **404**. List endpoints checked (SDK list, agents list,
reports) all carry an explicit `ProjectId ==`/`r.project_id = $1` predicate. No
list query was found returning rows without a project filter.

### 6c. `LaunchRequest.tester_id` not validated to the config's project — **NEEDS-REVIEW (P2)**
`launch` threads `req.TesterId` straight into `LaunchAsync`
(`TestConfigWriteEndpoints.cs:240`) → `run.TesterId` (`RunDispatcher.cs:97`) with no
check that the tester belongs to the config's project. Today the only *effect* of
`tester_id` is affinity/`FirstOrDefault` in `SelectTargetAgentAsync`, and the run's
`project_id` still comes from the config — so a foreign tester_id does not by itself
cross projects (it just fails to find an affine agent and falls through). But once §4
is fixed by scoping selection to the run's project, an unvalidated foreign
`tester_id` should also be rejected (or ignored) so it cannot influence routing.
Recommend: on launch, verify `project_tester.project_id == cfg.project_id` for a
supplied `tester_id`, else 400. P2 (becomes more relevant after the §4 fix).

### 6d. Role gating vs. the role-based-filtering requirement — **ISOLATED**
Writes require `ProjectOperator` (SDK create/delete `:123`,`:176`; test-config
create/launch/delete), reads require `ProjectMember` (any role incl. viewer). This
matches CLAUDE.md / the `[[role-based-filtering]]` rule: **viewer read-only, operator
write**, enforced **server-side by policy** (not merely hidden in the UI). A viewer
cannot POST/DELETE an SDK endpoint or launch a run. Confirmed at the policy
attributes above.

---

## Overall verdict

**The platform's HTTP/API surface is strictly project-scoped and the authorization
linchpin is correct** — project role is resolved live from `project_member` keyed on
the route's projectId, never trusted from the JWT; SDK CRUD is doubly-scoped;
tokens are write-only on every read path; the two reports scope their raw SQL to the
route's project via bound parameters.

**The SDK feature is NOT yet strictly project-scoped end-to-end**, because the
**run-dispatch path (§4) selects the executing agent without any project constraint**.
For an SDK-endpoint run this means the **decrypted customer LagHound token can be
delivered to an agent bound to a different project** — a P0 cross-project secret leak.
The token's at-rest crypto, masking, and log-redaction are all correct; the failure
is purely in *where the plaintext is sent*. Until `SelectTargetAgentAsync` (and the
redispatcher) restrict candidates to `agent.ProjectId == run.ProjectId`, and the
dispatcher asserts project agreement before splicing `laghound_token`, the isolation
invariant does not hold for the SDK feature.

### Findings requiring action
- **P0 — `RunDispatcher.SelectTargetAgentAsync` (RunDispatcher.cs:333-370) is not
  project-scoped**; `compatible[0]` (`:369`) can return an agent of another project,
  and `SerializeForAssignAsync` (`:537-603`) will hand it project A's **decrypted**
  LagHound token. Fix: scope candidate agents to `run.ProjectId` in selection and
  redispatch; refuse to attach the token unless the selected agent's project equals
  the run's project; add a cross-project-dispatch integration test.
- **P2 — App-network `test_config` join lacks `project_id`** (AppNetworkEndpoints.cs:168):
  add `AND c.project_id = $1` (no active leak today; enforces the invariant).
- **P2 — `LaunchRequest.tester_id` not validated to the config's project**
  (TestConfigWriteEndpoints.cs:240): validate `project_tester.project_id ==
  cfg.project_id` on launch.
