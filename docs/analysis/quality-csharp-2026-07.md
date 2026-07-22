# C# Control Plane — Code Quality Review (2026-07)

Deep read-only review of `src/Networker.ControlPlane`, `src/Networker.Agent`,
and `src/Networker.Data` (the production backend serving laghound.com). Scope:
concrete correctness / robustness / performance defects. Explicitly out of
scope (previously audited): authz/project isolation, CredentialCipher, delete
cascade, SchemaMigrator, style.

Overall verdict: a well-built codebase. Every background loop has a per-tick
try/catch + TickMonitor (no tick can kill a loop), every fire-and-forget site
captures exceptions, `PgAdvisoryLeaderLock` is correct, shell-outs have
timeouts + tree-kill, and the hot read endpoints are single-query projections
with keyset pagination. The defects below are edge-condition and race-window
bugs, not systemic sloppiness — but four of them can lose run results or wedge
runs permanently.

---

## Ranked findings (15)

### P1 — data loss / stuck runs

**F1. Stale-socket disconnect cleanup kills a reconnected agent's live runs**
`ControlPlane/Realtime/RawWs/AgentSocketEndpoint.cs:170-174` (same pattern
`Realtime/AgentProtocolHub.cs:152-153`)
`registry.Unregister(...)` is correctly compare-and-remove-guarded against a
newer registration, but `HandleDisconnectAsync(agentId)` runs *unconditionally*
in the `finally`. A dead socket can take up to 120 s (the idle timeout) to be
noticed; if the agent has already reconnected on a new socket, the old socket's
cleanup marks the agent `offline` and fails **all** its `running`/`queued` runs
("Agent disconnected during execution") — runs alive on the new connection.
*Fix:* make `AgentConnectionRegistry.Unregister` return whether it removed the
mapping; skip `HandleDisconnectAsync` (at minimum the run-failing half) when a
newer connection already re-registered.

**F2. Agent send path can silently lose `run_finished` (results lost)**
`Agent/RawWebSocketClient.cs:87` + `:242` and `:104-106` (+ `AgentWorker.cs:76`)
Two independent loss paths:
(a) The outbound channel uses `BoundedChannelFullMode.DropWrite` — in that mode
`TryWrite` on a full channel returns **true** and discards the item, so the
"channel full or closed" log at `:244` is unreachable for the full case. A
burst of `attempt_event`s can silently eat the terminal `run_finished`; the
control-plane run sits `running` until the watchdog fails it, with zero log
evidence anywhere.
(b) The disconnect `finally` does `TryComplete()` then immediately
`connCts.Cancel()`, making the pump's `ReadAllAsync(token)` throw and discard
everything still queued; `CancelAllRuns("disconnect")` then guarantees nothing
is re-sent after reconnect. A run finishing in the disconnect window loses its
result permanently.
*Fix:* use the `Channel.CreateBounded(options, itemDropped)` overload to log
drops and send terminal frames via blocking `WriteAsync`; drain the completed
channel (loop `TryRead` + send while the socket is writable) before cancelling
the pump. Longer term: spool undelivered terminal frames and replay after
reconnect.

**F3. Provisioning pipeline wedges permanently — four stuck states, no
timeout/recovery anywhere**
`ControlPlane/Provisioning/ProvisioningOrchestrator.cs:337-339`, `:254-264`,
`:367-374`; `Provisioning/DeployRunner.cs:138-142`, `:273-281`
(a) A deployment whose status is `cancelled` hits the orchestrator's "leave
alone; re-check next tick" default arm — its run stays `provisioning` forever.
(b) The deploy runs on a detached in-process `Task.Run`; a control-plane
restart mid-deploy (every release deploys!) orphans the deployment at
`pending`/`running` forever, and with it the run.
(c) `DeployRunner`'s cancellation path rethrows without calling `FinishAsync`,
and `FinishAsync` itself passes the *cancelled* `ct` into
`ExecuteUpdateAsync`, so terminal state cannot be persisted during shutdown.
(d) A `completed` deployment with no captured endpoint IPs makes
`PromoteAsync` log-and-return — retried every 5 s forever (stuck run + log
spam). The watchdog only covers `running`/`queued`; nothing times out
`provisioning`.
*Fix:* treat `cancelled` as terminal (fail the run); persist deployment
failure under `CancellationToken.None` in a `finally`; add a startup/tick
sweep failing deployments stuck in `pending`/`running` beyond an age cutoff
(~30 min) and runs in `provisioning` whose deployment is gone or stale; fail
the run on completed-with-no-IPs (the condition is permanent).

**F4. Wedged tester process starves the agent permanently (no overall run
deadline)**
`Agent/RunExecutor.cs:247`, `:506` (+ `AgentWorker.cs:37`)
The stdout read loop has no overall deadline — the workload `timeout` is only
a per-request `--timeout` for the tester binary. A tester that hangs without
EOF-ing stdout parks the run task forever; four such runs exhaust
`MaxConcurrentRuns = 4` and the agent can never run anything again until a
disconnect fires. Related: `WaitAndDrainAsync` awaits `WaitForExitAsync()`
with no token right after `KillTree` (whose failures are swallowed at
`:517-520`) — an unkillable process holds a slot the same way.
*Fix:* wrap each invocation in a linked CTS with a computed deadline
(`timeout × runs × modes + slack`), `KillTree` on expiry and emit a `failed`
terminal; give the post-kill wait a ~10 s token.

### P2 — duplicate/wrong results, orphaned cloud resources, races

**F5. Redispatcher re-assigns already-assigned runs → duplicate execution**
`ControlPlane/Dispatch/RunDispatcher.cs:166-172` (+ `Agent/AgentWorker.cs:179`)
`RedispatchQueuedAsync` selects on `Status == "queued" && CreatedAt < now-15s`
only — it never excludes runs with a `WorkerId` already stamped. A run
assigned to a busy agent that hasn't sent `run_started` within ~15-45 s is
re-sent every 30 s, and `SelectTargetAgentAsync` may pick a *different* agent
(candidate order is unspecified). The agent's `_running.TryAdd` dedupe covers
only in-flight runs — a re-delivery after completion re-executes the entire
run and emits a second `run_started`/`run_finished` pair.
*Fix:* add `&& r.WorkerId == null` to the redispatch candidate query (or an
assigned-at timestamp with its own staleness cutoff); agent side, keep a
bounded LRU of recently completed run ids and drop repeats with a warning.

**F6. Late/duplicate agent frames resurrect terminal runs**
`ControlPlane/Realtime/RawWs/AgentMessageProcessor.cs:371-378` (`OnRunStarted`),
`:436-440` (`OnRunFinished`), `:519-524` (`OnError`)
None of these `ExecuteUpdate`s guard on current status. A `run_started`
arriving after cancel/watchdog-fail flips `failed`→`running` (with no live
owner, so the watchdog fails it again 120 s later); an `error` frame after
`completed` flips a successful run to `failed`; `run_finished` accepts
non-terminal statuses (`queued`/`running`/`provisioning` are in the allowed
set).
*Fix:* status preconditions in the `Where` clauses (`OnRunStarted`:
`Status == "queued"`; `OnRunFinished`/`OnError`: `Status IN (queued, running)`)
and restrict `run_finished` to terminal statuses.

**F7. AutoShutdown can deallocate a VM mid-deployment/bootstrap**
`ControlPlane/Background/AutoShutdownService.cs:128-135`
The drain check only excludes non-terminal `test_run`s. A tester with an
in-flight `Deployment` in `running` (install.sh, up to 30 min) or whose own
`status` is still `provisioning` passes as `power_state='running' AND
allocation='idle'` and gets deallocated under the installer — the deploy fails
and the tester can wedge.
*Fix:* extend the sweep predicate (and the `stillDrained` re-check at
`:203-209`) to exclude testers with a non-terminal deployment or
`provisioning`/`deploying` status.

**F8. Successfully created VM's resource id discarded on partial failure —
orphan VM with no DB record**
`ControlPlane/Provisioning/CliComputeProvisioner.cs:604-613`, `:908`
Windows/Azure: if `az vm extension set` fails after `az vm create` succeeded,
`VmCreateResult.Fail(...)` throws away the known `resourceId` — the VM exists
and bills, but the caller records nothing to delete or track (the orphan
reaper's name-prefix allow-list is then the only safety net). AWS: a
cancellation during the public-IP poll (`Task.Delay` at `:908`) rethrows and
discards `instanceId` entirely.
*Fix:* carry the partial resource id on the failure result so the caller can
record it or issue a compensating delete; on cancel during the IP poll return
`Created(instanceId, "")`.

**F9. Empty `--subscription ""` under ambient auth → start/deallocate hard-fails
every 60 s tick forever**
`ControlPlane/Provisioning/CliComputeProvisioner.cs:339-340, 350`
When `LoadCredentialsAsync` returns null (ambient CLI auth), `BuildAzure`
passes `--subscription ""` — az errors before dispatch, AutoShutdown treats
the non-zero exit as `realFailure`, rolls the row back to `running`, and
retries every tick indefinitely. The same bug class was already fixed 30 lines
above for the NSG/IP cascade (comment dated 2026-07-21) but not here.
*Fix:* omit `--subscription`/`--resource-group` when empty (`--ids` is
self-describing), or reuse `ParseAzureScope(tester.VmResourceId)`.

**F10. `CancelAsync` clobbers terminal run status**
`ControlPlane/Dispatch/RunDispatcher.cs:220-222`
`ExecuteUpdateAsync(SetProperty(Status, "cancelled"))` has no status guard: a
cancel racing (or arriving after) completion rewrites `completed`/`failed`
history to `cancelled` — and never sets `FinishedAt`.
*Fix:* guard `Where(... && Status IN (queued, running, provisioning))`, set
`FinishedAt`, and report the run's actual state when 0 rows are affected.

**F11. Cloud secrets exposed via argv and world-readable temp files**
`ControlPlane/Provisioning/CliComputeProvisioner.cs:511-513`, `:716-719`,
`:1123-1124`; `Provisioning/DeployRunner.cs:95-98`
`sensitiveArgs: true` redacts only the log line — the SP client secret
(`az login -p …`) and the Windows admin password sit on argv, world-visible in
`ps`/`/proc/*/cmdline` for the duration of multi-minute CLI calls. The GCP
service-account key and `deploy-{id}.json` (contains the minted agent API key)
are written to shared `/tmp` via manual paths → umask-default 0644, unlike the
`GetTempFileName()` (0600) paths elsewhere.
*Fix:* pass Azure SP creds via env (`AZURE_CLIENT_ID`/`SECRET`/`TENANT_ID`),
feed passwords via `@file` with 0600 perms; create the key/deploy files with
`File.OpenHandle(..., UnixCreateMode: UserRead|UserWrite)`.

**F12. Agent run-task robustness: zombie tester on I/O error, output cap
defeated by single-line JSON, CTS dispose race**
`Agent/RunExecutor.cs:247-250`, `:287`; `Agent/AgentWorker.cs:188-191` vs
`:221-225`
(a) Only OCE triggers `KillTree` between spawn and exit — an `IOException`
from the stdout pipe propagates and `using var process` disposes *without
killing*; the tester keeps running unbounded ("kill-on-drop" comment doesn't
hold in .NET).
(b) The 128 MiB cap is checked after `ReadLineAsync` returns, but
`--json-stdout` emits one giant line — it buffers fully in memory before the
cap is consulted (and the counter counts UTF-16 chars, not bytes).
(c) `await _runSlots.WaitAsync(runCts.Token)` and the token reads sit outside
the run task's `try`, while `HandleCancelRun` does `Cancel(); Dispose();`
eagerly — losing that race throws `ObjectDisposedException` as an unobserved
task exception: no log, no terminal frame.
*Fix:* post-spawn `catch (Exception) { KillTree(process); … }`; enforce the
cap chunk-wise via `ReadAsync`; `HandleCancelRun` should only `Cancel()` and
let the run task's `finally` own the dispose.

### P3 — maintainability / minor

**F13. Duplicate inactivity-warning rows permanently break the daily lifecycle
pass**
`ControlPlane/Background/InactivityService.cs:222-224`
`ToDictionary(w => w.ProjectId…)` throws on a duplicate `(project, type)` pair
(possible historically, or from two replicas before the optional leader lock).
The loop survives, but *every* subsequent daily pass fails at the same line —
warn/suspend/delete silently stops platform-wide.
*Fix:* `GroupBy(...).ToDictionary(g => g.Key, g => g.Max(w => w.SentAt))`.

**F14. `"pending"` endpoint-kind literal + skip logic copied across four files**
`Background/WatchdogService.cs:217`, `Background/SchedulerService.cs:174`,
`Dispatch/RunDispatcher.cs:31`, `Provisioning/ProvisioningOrchestrator.cs:59`
Each re-declares the literal and re-implements "skip pending-endpoint runs" —
precisely the copy-drift pattern behind #377-#379. Smaller dups:
`FirstEndpointIp`/`FirstEndpointHost` and `RawJson` exist twice each.
*Fix:* one `EndpointKinds` constants class + shared `IsPendingEndpoint`
helper; consolidate the JSON helpers.

**F15. Missing CancellationToken on hot queries**
`ControlPlane/Endpoints/TestRunsEndpoints.cs:107` (the Runs-page list query —
the hottest read in the UI) and ~27 similar no-token
`ToListAsync()`/`SaveChangesAsync()` calls across `Endpoints/`; also
`AgentSocketEndpoint.cs:157` calls `HandleFrameAsync` without the connection
token. Abandoned requests keep their Postgres queries running.
*Fix:* accept `CancellationToken ct` in the handler signatures (minimal-API
binds it) and thread it through.

---

## Additional minor notes (below the ranked cut)

- 4xx error envelopes are inconsistent: bare `Results.NotFound()` /
  `BadRequest()` (empty body) in AdminEndpoints:140, AlertsEndpoints:119,
  BenchTokensEndpoints:83 etc., vs the `{ "error": … }` envelope elsewhere
  (500s are uniform via `UseErrorEnvelope`). Add `ApiErrors.*` helpers.
- `RawSocketRegistry.UnsubscribeTesterGroup` (:257-271): the retire race can
  drop a group that a concurrent subscribe just populated — the new subscriber
  is stranded until reconnect; the "benign race" comment overstates safety.
- `InactivityService` (:104-115): warning rows are never cleared on
  reactivation — a second inactivity spell can suspend at 120 d without a
  fresh 90 d warning.
- `VersionRefreshService.cs:157`: fallback `new HttpClient()` is never
  disposed (slow socket leak on hosts without `IHttpClientFactory`).
- `CliComputeProvisioner.cs:298`: `IsRetryableInUse` matches the bare word
  `"reference"` — retries many non-retryable Azure errors (+36 s under the
  leader lock).
- `DeployRunner.cs:206-208`: if install.sh leaves a grandchild sharing stdout,
  the pipe drain blocks until the 30 min timeout and a *successful* deploy is
  persisted `failed`. After `WaitForExitAsync`, bound the drain with a short
  grace and use the real exit code.
- `AutoShutdownService.cs:190-194`: the "re-load fresh" reads the already-
  tracked (stale) instance via EF identity resolution — use `AsNoTracking()`
  on the sweep or clear the tracker.
- `AgentVersionGate` (assessed, no defect): the permissive Rust-parity parse
  is correct; only quirk is `0.28.0-rc1` counting as ≥ 0.28.0.

---

## /hub/* SignalR duplicate transport — recommendation

**Remove the three `MapHub` routes now; strip the SignalR internals in a
follow-up. This is dead weight with an active drift cost.**

Evidence:
- Zero consumers. The C# agent explicitly speaks raw WS
  (`Agent/RawWebSocketClient.cs:12` — "NOT SignalR"); the React dashboard has
  no SignalR client. Nothing in-repo or fielded dials `/hub/dashboard`,
  `/hub/testers`, or `/hub/agent`.
- Drift has already happened: `AgentProtocolHub.OnConnectedAsync` accepts only
  the legacy `?key=` query (no `X-LagHound-Agent-Key` header support, :86) and
  feeds the brute-force limiter the raw `RemoteIpAddress` (:87) instead of the
  raw endpoint's `X-Real-IP`-aware `ResolveClientIp` — behind nginx that
  collapses this path's limiter to a single 127.0.0.1 bucket. It also shares
  the F1 disconnect race. Every agent-protocol change must now be made twice.
- It is exposed surface serving no one.

Refactor cost, staged:
1. **Cheap (do now):** delete the three `MapHub` calls (`Program.cs:232-234`)
   and the `AgentProtocolHub`/`BrowserHub`/`TesterQueueHub` classes (~600
   lines). All protocol logic already lives in the shared
   `AgentMessageProcessor`/registries — nothing behavioral is lost.
2. **Moderate (follow-up):** `EventBus` holds `IHubContext<BrowserHub>`,
   `AgentConnectionRegistry` holds `IHubContext<AgentProtocolHub>`, and
   `RawWsTesterQueueLifetimeManager` decorates the SignalR lifetime manager —
   removing `AddSignalR()` entirely means re-plumbing those three seams to
   raw-only. Worth doing, but separable.

The stated rationale ("future SignalR-native clients, e.g. the C# agent") is
obsolete: the C# agent already chose raw WS.
