# Frontend Quality Review — dashboard/ (2026-07)

Deep code-quality review of the React/TS SPA (`dashboard/src`). Scope: concrete
fixable defects — lint debt, WebSocket/SSE lifecycle, stale closures,
error-handling black holes, api-client drift, render performance, type safety.
Out of scope (recently done elsewhere): RBAC, `lib/analysis.ts`, styling.

## Evidence baseline

| Check | Result |
|---|---|
| `npx tsc --noEmit` | clean (exit 0) |
| `npx eslint src` | 2 errors + 1 warning (the 3 known pre-existing items, confirmed below) |
| `npx vitest run` | 33 files, 190 tests, all passing (3.05s) |
| API drift spot-check | 6 hot client routes verified against C# `Map*` handlers — no drift (see §API) |
| Table keys/memo | Runs/attempts tables use stable keys (`run.id`, `attempt_id`); `key={i}` appears only on skeleton rows — no measurable render defect |
| Type safety in api layer | No `as any` / `@ts-ignore` / `@ts-expect-error` in `src/api` or `src/stores` — clean |

---

## P1 — Broken behavior

### 1. `useWebSocket`: project switch while disconnected keeps the old project's seq watermark → live feed silently drops events

`dashboard/src/hooks/useWebSocket.ts:126-133`

The project-change effect early-returns when `wsRef.current` is null:

```ts
useEffect(() => {
  // Skip initial mount — the effect above handles that
  if (!wsRef.current) return;
```

But `wsRef.current` is *also* null whenever a reconnect timer is pending (the
socket dropped and `onclose` scheduled `connectRef.current()`; `wsRef` isn't
re-assigned until `connect()` runs). If the user switches projects during that
window:

- the pending reconnect timer is **not** cleared and `backoffRef` is not reset;
- `lastSeqRef` / `hasConnectedRef` are **not** reset — the exact invariant the
  comment on lines 139-143 says must hold ("Project switch = new fan-out
  scope. Previous seqs are no longer meaningful");
- when the timer fires, `connect()` reads the *new* `activeProjectIdRef` but
  sends `?since=<old project's seq>` and, worse, the client-side guard at
  line 72 (`if (data.seq <= lastSeqRef.current) return;`) then discards every
  incoming event of the new project whose seq is below the old watermark —
  the live feed appears connected but is silently dead until the new
  project's seq counter passes the stale one.

**Fix:** distinguish "initial mount" from "disconnected" explicitly. Replace
the `if (!wsRef.current) return;` guard with a `firstRunRef` (skip only the
very first execution), and let the rest of the effect run unconditionally —
it already clears the timer, resets backoff/seq/hasConnected, and reconnects.

### 2. `useDeployEvents`: no reconnect on stream drop → live deploy log freezes mid-deploy

`dashboard/src/hooks/useDeployEvents.ts:98-145`

`useSSE.ts:4-6` documents the failure mode precisely ("proxies and server
restarts drop it; without a reconnect loop, notifications silently stop until
a full page reload") and implements backoff reconnect. `useDeployEvents` —
which watches multi-minute Azure deploys through nginx, the worst case for
proxy idle timeouts — has none: when `reader.read()` returns `done` or
throws, the effect sets `connected = false` and stops. The deploy-detail page
freezes mid-log with no recovery and no user-visible error.

Compounding it, `catch {}` at line 138 swallows *all* errors (not just the
expected `AbortError`), and the non-OK branch at line 88 returns without
recording anything — a black hole in an app whose stated rule is instrumented
observability.

**Fix:** port the reconnect loop from `useApprovalSSE` (initial 1s, cap 30s,
`cancelled` flag, bail on 401/403, reset backoff on successful connect), and
on replay-reconnect rely on the existing `lastSeqRef` dedup (already correct).
Re-throw / log non-`AbortError` failures. Bonus: guard the `finally`
`setConnected(false)` with the `cancelled` flag so an aborted StrictMode
first-pass can't race the second mount's `setConnected(true)`.

---

## P2 — Leaks, data loss, hygiene violations

### 3. `useTesterSubscription`: infinite reconnect with a dead token + ineffective `onclose = null` + inconsistent backoff caps

`dashboard/src/hooks/useTesterSubscription.ts:138-153, 160-168`

Three related defects:

- **No auth bail-out.** Unlike `useApprovalSSE` (stops on 401/403), a socket
  the server closes because the JWT expired is retried forever, every ≤15s,
  per mounted hook instance ("each hook owns its own WebSocket" per the
  header comment — so N testers pages = N hammering loops). Inspect the
  `CloseEvent.code` in the close listener (the server's auth-reject close
  code, e.g. 4401/1008) and stop reconnecting.
- **`socket.onclose = null` (line 162) does nothing** — the handler was
  registered with `addEventListener('close', …)`, which the `onclose`
  property does not detach. Teardown currently works only because the
  `cancelled` flag happens to guard the listener; the comment claims a
  protection that doesn't exist and will bite the next refactor. Use
  `removeEventListener` or keep a named handler.
- **Backoff cap mismatch** (lines 150-151): delay is capped at 15 000 ms but
  `backoff` grows to 30 000 — the second cap is unreachable dead logic. Pick
  one constant.

### 4. `usePerfLogFlush`: failed flush permanently drops perf entries; response status never checked; `keepalive` 64 KB limit unhandled

`dashboard/src/hooks/usePerfLogFlush.ts:62-86`

The cursors (`lastFlushedApi/-Render`) are advanced *before* the POST with a
comment about "retry" — but there is no retry: on any failure (`catch {}`
line 84, silent) the entries between old and new cursor are gone forever.
Also `res.ok` is never inspected, so a 401/413/500 counts as success. And
`keepalive: true` fetches are capped at ~64 KB of body by the browser — the
pagehide flush "carrying the page's whole session" (the comment's own words)
is exactly the one most likely to exceed it and be rejected synchronously.

This is the observability spine of the app — losing it silently contradicts
the project's instrumented-timing requirement.

**Fix:** advance cursors only after `res.ok`; on failure leave them so the
next interval retries (dedup is server-side by `session_id` + timestamp if
double-send matters); chunk the payload to stay under 64 KB when
`keepalive` is set.

### 5. Raw-`fetch` api endpoints bypass `friendlyHttpError` and can render raw server bodies

`dashboard/src/api/client.ts:301-309` (`resetPassword`), `411-415`
(`resolveInvite`), `417-428` (`acceptInvite`), `654-658` (`resolveShareLink`),
`292-299` (`forgotPassword`)

`client.ts:307`: `if (!r.ok) throw new Error(await r.text());` — the thrown
message is the raw response body, which pages render via `errorMessage()`.
This is precisely what `friendlyHttpError` (lines 31-81) exists to prevent
("never surface a raw body verbatim", design audit F3/F16) — and these five
endpoints also skip the api-log/perf-log instrumentation that `request()`
provides, despite the module-level rule at lines 128-131 ("All REST modules
must go through this").

**Fix:** route them through `request()` (all five are plain JSON POST/GET; the
anonymous ones just need their paths added to `AUTH_401_EXEMPT` like
`/auth/login`), or at minimum wrap failures with
`friendlyHttpError(r.status, r.statusText, body)`.

### 6. `LeaderboardPage` TimelineTab: detail-fetch failure is a silent dead end

`dashboard/src/pages/LeaderboardPage.tsx:363-371`

`toggleRun` expands the row, then fetches detail; on failure `catch { // ignore }`
leaves the row expanded with no data, no error state, no toast, no log — the
user sees a permanently empty expansion and no way to know why. (Elsewhere the
codebase does this right — `ProjectMembersPage` toasts on every failure.)

**Fix:** `catch { addToast('error', 'Failed to load run detail'); setExpandedId(null); }`
(or store an error marker in `details[runId]` and render it inline).

---

## P3 — Lint debt (the 3 known items, confirmed) + stubs + hazards

### 7. `ValueReportPage.test.tsx:94` — eslint error `@typescript-eslint/no-unused-vars` on `_projectId`

Confirmed. The flat config (`dashboard/eslint.config.js`) uses
`tseslint.configs.recommended` with **no `argsIgnorePattern`**, so the `_`
convention isn't honored — which is also why `client.ts:320` needs an inline
disable for the same pattern.

**Fix (preferred, kills both):** add to `eslint.config.js` rules:

```js
'@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
```

then delete the now-redundant disable at `client.ts:320`.
**Fix (minimal):** `const getPerfPerCostReport = vi.fn(() => Promise.resolve(report));`
— the arg can be dropped; the mock wrapper at line 98 still forwards args for
call-assertion purposes.

### 8. `lib/ansi.ts:13` — unused `eslint-disable-next-line no-control-regex`

Confirmed. `no-control-regex` only fires on regex *literals* containing raw
control characters; this pattern is built with `new RegExp(...)` from `\uXXXX`
string escapes, so the rule never triggers.
**Fix:** delete line 13. (Autofixable: `npx eslint src --fix` removes it.)

### 9. `components/wizard/TestbedMatrix.tsx:60` — `react-hooks/set-state-in-effect` error

Confirmed. `setTestersLoading(true)` runs synchronously in the effect body
(needed only for `projectId` *changes*, since initial state is already `true`).
**Fix:** derive loading instead of setting it:

```tsx
const [loadedProjectId, setLoadedProjectId] = useState<string | null>(null);
const testersLoading = loadedProjectId !== projectId;
// in the effect:
testersApi.listTesters(projectId)
  .then(rows => { if (!cancelled) setTesters(rows); })
  .catch(() => { if (!cancelled) setTesters([]); })
  .finally(() => { if (!cancelled) setLoadedProjectId(projectId); });
```

Removes the sync setState entirely and stays correct across project switches.

### 10. `api.checkEmail(_email)` stub + dead SSO branch in LoginPage

`dashboard/src/api/client.ts:315-322`, `dashboard/src/pages/LoginPage.tsx:80-88`

Confirmed dead: no `check-email` route exists anywhere in
`src/Networker.ControlPlane` (grep over all `Map*` registrations). The stub is
well-documented and locally resolves `{ provider: null }`, so there's no wasted
round-trip — but `LoginPage.handleEmailContinue` still carries an unreachable
SSO-redirect branch (line 83) and a `catch` fallback (line 87-88) for a promise
that cannot reject. If item 7's `argsIgnorePattern` fix lands, the inline
disable at line 320 goes away too.
**Fix:** either keep the stub (harmless, documented) or delete `checkEmail`
and collapse `handleEmailContinue` to validate + `setShowPassword(true)`,
moving the "restore when per-email SSO routing ships" note to LoginPage.

### 11. `usePolling`: request-source tag reverts to `'user'` after the first await inside a poll callback

`dashboard/src/hooks/usePolling.ts:16-21`

`setRequestSource('poll')` is reset on the next microtask, so only fetches
issued *synchronously* by `fn` are tagged `poll`. Any consumer that awaits
before a subsequent api call gets that call mis-tagged `user`, corrupting the
perf-log `source` dimension. Today's ~25 consumers are safe (checked the
biggest, `InfrastructurePage.tsx:189-197` — a single synchronous
`Promise.all`), so this is a latent hazard, not a live bug — but nothing
enforces it.
**Fix:** document the constraint in the hook's JSDoc ("all api calls must be
issued synchronously"), or make it structural: have `request()` accept an
explicit `source` override and pass a tagged wrapper into `fn`.

---

## What's good (worth keeping as patterns)

- `api/client.ts request()`: the single choke point with perf instrumentation,
  `friendlyHttpError`, 401/403 session handling, and empty-body tolerance is
  exactly right — the P2 #5 items are the stragglers, not the design.
- Defensive dual-shape decoding (`getAgents`, `getTestRunAttempts`) prevents
  the class of contract-drift blanking that bit v0.28.x.
- `useWebSocket`'s seq-watermark replay design and StrictMode-safe teardown
  are careful work; P1 #1 is one guard condition away from correct.
- Error copy hygiene (`STATUS_COPY`, HTML-body dropping, 160-char truncation)
  and toast-on-failure discipline in `ProjectMembersPage` are the house style
  the P2/P3 catch-holes should be brought up to.
- Data-dense tables already use stable domain keys and skeleton rows — no
  render-perf work needed.
