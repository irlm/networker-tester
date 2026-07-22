# Dead-Code Survey — Frontend (dashboard/) + SDKs (sdk/) — 2026-07

Read-only survey. No source was modified. Method: `tsc --noEmit` baseline
(clean), then grep-based reachability analysis. **knip did not run** — `npx
knip` requires a network package install, which is blocked in this
environment; every candidate below was therefore verified manually with
import-site / call-site greps (commands quoted inline as evidence).

Verification protocol used throughout: a symbol is only listed as dead if a
**bare-name** grep (not `api.<name>` — which misses multi-line chained calls
like `testersApi\n .getCostEstimate`) across all of `dashboard/src`
(including tests) finds zero references outside the defining file.

---

## Executive summary

| Area | Result |
|---|---|
| dashboard/ orphan files | **0** — all 44 pages are routed in `App.tsx` (all via `lazyPage()`), every component/hook/lib/store module has ≥1 static import site |
| dashboard/ dead exports | **~40 dead API-client methods**, 1 dead hook file, 2 dead testersApi methods, 3 dead wizard constants, ~4 dead type clusters |
| dashboard/ dead assets | 3 files, **57.7 KB** (`hero.png`, `react.svg`, `vite.svg`) |
| dashboard/ unused npm deps | **0 removable** (`react-is` looks unused but is a recharts peer dep — keep) |
| Mode-list duplication (historic 6-way problem) | **Resolved** — guarded by `src/lib/modes-manifest.test.ts`; 3 leftover unguarded constants are dead (below) |
| sdk/go, sdk/js, sdk/python, sdk/rust, sdk/csharp | **Clean** — no internal dead code found |

Estimated total removable: **~550–600 lines of TS + 57.7 KB of assets + one
stray committed artifact.**

---

## SCOPE A — dashboard/

Baseline: `cd dashboard && npx tsc --noEmit` → exit 0, no errors.

### A1. `src/hooks/usePhaseSubscription.ts` — entire file dead in production (152 lines) — HIGH

- File: `dashboard/src/hooks/usePhaseSubscription.ts` (152 lines; exports
  `Phase`, `Outcome`, `PhaseState`, `usePhaseSubscription`).
- Evidence: `grep -rn "usePhaseSubscription" src` → the only importer is its
  own test block:
  - `src/hooks/testerSubscription.test.ts:4` — `import { usePhaseSubscription } from './usePhaseSubscription';`
  - No page or component imports it. Pages use `useTesterSubscription`
    instead (`InfrastructurePage.tsx:17`, `TesterDetailDrawer.tsx:7`,
    `TesterRegionGroup.tsx:3`).
- Removing it also frees the `describe('usePhaseSubscription', ...)` block in
  `testerSubscription.test.ts` (starts line 144).
- Confidence: HIGH (static ESM imports only; not reachable via lazy()/string).
- Estimated removable: ~152 + ~100 test lines.

### A2. `src/api/client.ts` — ~40 dead methods (~180–200 lines) — mostly HIGH

`client.ts` is 1210 lines defining ~172 methods on the `api` object. Cross
reference of every method name (bare-name grep across all of `src`,
tests included) shows these have **zero call sites** outside `client.ts`.
Most are pre-v0.28 backend-unification leftovers (Job/BenchmarkConfig →
TestConfig; legacy Schedule → TestSchedule).

Legacy benchmark-config family (superseded by TestConfig; 10 methods):

| Method | Line |
|---|---|
| `listBenchmarkConfigs` | 935 |
| `getBenchmarkConfig` | 938 |
| `createBenchmarkConfig` | 941 |
| `launchBenchmarkConfig` | 947 |
| `cancelBenchmarkConfig` | 950 |
| `getBenchmarkProgress` | 956 |
| `getBenchmarkConfigRegressions` | 978 |
| `getBenchmarks` | 541 |
| `getBenchmark` | 550 |
| `compareBenchmarks` | 553 |

(Live siblings for contrast: `getBenchmarkConfigResults` — used at
`BenchmarkConfigResultsPage.tsx:225`; `listBenchmarkCatalog`,
`registerBenchmarkVm`, `deleteBenchmarkVm`, `detectBenchmarkVmLanguages` all
used.)

Benchmark compare presets (3): `getBenchmarkComparePresets` (559),
`saveBenchmarkComparePreset` (562), `deleteBenchmarkComparePreset` (568).

Legacy schedules — superseded by `*TestSchedule*` (6): `createSchedule`
(651), `getSchedules` (640), `getSchedule` (648), `updateSchedule` (666),
`deleteSchedule` (680), `toggleSchedule` (683), `triggerSchedule` (686).
Evidence: `SchedulesPage.tsx` uses only `api.listTestSchedules` (92),
`api.updateTestSchedule` (102), `api.triggerTestSchedule` (114),
`api.deleteTestSchedule` (130). The `updateSchedule` hits at
`TesterDetailDrawer.tsx:470,519` and `api/testers.ts:185` are
`testersApi.updateSchedule` — a different namespace.

Legacy agents/jobs/runs (5): `createAgent` (458), `deleteAgent` (472),
`deployTesterVm` (475), `getJobs` (481), `getRuns` (503). (`getJob`,
`createJob`, `cancelJob`, `getRun`, `getRunAttempts` are live —
`JobDetailPage.tsx:264` etc.)

Command-visibility rules (3): `getVisibilityRules` (~808),
`addVisibilityRule` (~810), `removeVisibilityRule` (812) — no UI consumes
them (CommandApprovalsPage uses only `getPendingApprovals`,
`getPendingApprovalCount`, `decideApproval`).

Singletons (13):

| Method | Line | Note |
|---|---|---|
| `exchangeCode` | 324 | legacy SSO path; `ssoExchange` (285) is the live one |
| `updateProject` | 357 | no settings UI calls it |
| `deleteProject` | 363 | ditto |
| `getCloudStatus` | 607 | |
| `updateLocalTester` | 615 | `updateDashboard` (618) IS used |
| `startDeployment` | 594 | stop/delete/check are used |
| `updateCloudConnection` | 730 | create/delete/validate are used |
| `uploadLeaderboardResults` | 912 | CI/token path posts directly, not via SPA |
| `ingestPerfLogs` | 994 | `usePerfLogFlush.ts:75` uses raw `fetch('/api/perf-log', ...)` instead |
| `updateTestConfig` | 1031 | create/list/get/delete/launch all used |
| `getComparisonGroup` | 1127 | create/launch used; `listComparisonGroups` (1124) also dead |
| `getSdkEndpoint` | 1190 | list/create/delete used |

Confidence: HIGH for the legacy families (superseded endpoints, zero refs);
MEDIUM for `updateTestConfig`, `updateProject`/`deleteProject`,
`listComparisonGroups`/`getComparisonGroup` (plausibly next-sprint wiring —
but currently unreferenced).

Note: methods here are unreachable UI-side but the server endpoints they
wrap may still be live for other clients; this survey only claims the
*frontend* code is dead.

### A3. Dead types dragged along in `src/api/types.ts` (~80–120 lines) — MEDIUM

Only referenced by the dead methods above (or by nothing at all):

- `Schedule` (types.ts:710) — only refs are client.ts:645,649 inside dead
  schedule methods. ~18 lines.
- `RunSummary` (223) — only ref client.ts:510 inside dead `getRuns`.
- `CloudStatus` (640) + `ProviderStatus` (647) — only ref client.ts:607.
- `BenchmarkProgressResponse` (1135) + `BenchmarkModeProgress` (1119) +
  `BenchmarkLanguageProgress` (1129) — `BenchmarkProgressResponse` has zero
  references outside its own definition (grep of types.ts internal refs:
  only line 1135 itself).
- `BenchmarkComparePreset` / `BenchmarkComparePresetInput` /
  `BenchmarkComparePresetFilters` (261–286) — consumed only by the dead
  preset methods.

Not dead (verified transitively referenced, do not remove): the big
`Benchmark*` metadata cluster (`BenchmarkHostInfo`, `BenchmarkEnvironment*`,
`BenchmarkExecutionPlan`, `BenchmarkNoiseThresholds` — fields of
`BenchmarkArtifact`), `AzureConfig`/`AwsConfig`/`GcpConfig` (fields of
`CloudConnection`, line 743), `ImportDetail`/`SendInviteDetail`,
`TlsEndpointProfile`, `GroupedLeaderboardEntry`, `PerfLogRow`.
Confidence MEDIUM overall because types.ts mirrors the API contract and the
team may prefer keeping contract types complete.

### A4. `src/api/testers.ts` — 2 dead methods (~10 lines) — HIGH

- `testersApi.getRegions` (testers.ts:143) — zero call sites
  (`grep -rn "getRegions" src` → only the definition).
- `testersApi.getQueue` (testers.ts:148) — zero call sites; queue state
  arrives via `useTesterSubscription` WebSocket instead.
- `getCostEstimate` (151) IS live — chained call at
  `TesterDetailDrawer.tsx:100` (`testersApi\n .getCostEstimate(...)`) —
  which is exactly why single-line `api.x` greps are insufficient here.

### A5. `src/components/wizard/testbed-constants.ts` — 3 dead constants — HIGH

Zero references anywhere (including `testbed-constants.test.ts`):

- `CLOUDS` (line 3) — `['Azure', 'AWS', 'GCP']`.
- `DEPLOY_REGIONS` (358) — lowercase-keyed region map **duplicating the live
  `REGIONS` (line 5)**; leftover of the historic constant-duplication
  problem.
- `HTTP_STACKS` (364) — `['nginx', 'iis']`.

Kept-but-only-internal (not dead, no action): `WINDOWS_PROXIES_AZURE/AWS/GCP`
feed `windowsProxiesFor()` (75–77) and the `WINDOWS_PROXIES` alias;
`WINDOWS_ONLY_LANGS` feeds a helper at 215.

### A6. Dead assets — 57.7 KB — HIGH

Zero references in any `.ts/.tsx/.html/.css` (grep for `hero`, `react.svg`,
`vite.svg`, `assets/` across `src` + `index.html`; `index.css` contains only
`@import "tailwindcss"`):

| File | Size |
|---|---|
| `dashboard/src/assets/hero.png` | 44,919 B |
| `dashboard/src/assets/vite.svg` | 8,709 B (Vite scaffold leftover) |
| `dashboard/src/assets/react.svg` | 4,126 B (Vite scaffold leftover) |

(Vite would not bundle these anyway since nothing imports them — the win is
repo hygiene, not bundle size. `public/favicon.svg` + `public/icons.svg` are
live via `index.html`.)

### A7. npm dependencies — nothing removable

- `react-is`: zero direct imports in `src`, **but** it is a declared
  peerDependency of `recharts@3.8.0` (package-lock.json:4098). Keep. This is
  the classic knip false positive; documented here so nobody "cleans" it.
- All other deps have direct import hits (`@tanstack/react-virtual` →
  `JobDetailPage.tsx:2`; `recharts` → RunDetail/ValueReport/JobDetail pages;
  `tailwindcss`/`@tailwindcss/vite` are config-referenced in
  `vite.config.ts`/`index.css`). Devdeps are all wired to eslint/vitest/tsc.

### A8. Stray committed artifact — `dashboard/networker-cloud.json` — MEDIUM

3-line probe config pointing at
`networker-endpoint-vm.eastus.cloudapp.azure.com` — this is the file the
*installer writes at runtime* (see `.github/wiki-pages/Cloud-Deployment.md`
lines 16/46/108), accidentally committed into `dashboard/`. Nothing in the
frontend build references it. MEDIUM only because someone may treat it as a
sample config.

### A9. Mode-list duplication status (historic 6-way problem) — resolved

- `src/components/common/mode-family.ts` (`FAMILY_BY_MODE`, `MODE_LABELS`)
  and `testbed-constants.ts` `RUNTIME_TEMPLATES` are both drift-guarded
  against the canonical `shared/modes.json` by
  `src/lib/modes-manifest.test.ts` (lines 15–16, 20, 53, 87).
- Remaining hardcoded mode arrays (`NetworkTestPage.tsx:34–78`,
  `EndpointRunsPage.tsx:43–46`, `DiagnosticsPage.tsx:34–36`,
  `FullStackPage.tsx:44`, `AppBenchmarkPage.tsx:55`) are *curated preset
  subsets*, not duplicated authoritative lists — no dead code, though they
  are not manifest-guarded (a typo there would ship silently; out of scope
  for this survey).

### A10. Minor notes (not counted in totals)

- `api.checkEmail` (client.ts:321) takes `_email` and ignores it — stub, but
  it is called (2 sites), so not dead.
- `analysis.ts` over-exports `formatThroughput`, `attemptPayloadBytes`,
  `extractPhaseTiming`, `groupByProtocolAndPayload` — zero external
  consumers, but all are used *within* analysis.ts (lines 159, 181–182, 233,
  280). Could be un-exported, not removed.
- `benchmark.ts` `formatBenchmarkNumber` — same pattern (internal use at
  lines 13, 19).

---

## SCOPE B — sdk/ (customer-shipped libraries; public API excluded by design)

Only *internal* (unexported/private) symbols were audited. Result: **no dead
internal code found in any of the five SDKs.**

- **rust** (`sdk/rust`): `cargo check --all-features` → zero warnings (the
  compiler's `dead_code` lint would flag unused private items). Clean.
- **go** (`sdk/go`): all 11 unexported helpers (`bare404`, `clientIP`,
  `withBucket`, `bucketFrom`, `appendMarksTo`, `validMarkName`, `durMS`,
  `newBucket`, `newLockedBucket`, `newIPLimiter`, `newBudget`) have call
  sites beyond their definitions (per-file grep counts ≥2 each). Clean.
- **js** (`sdk/js/src/index.ts`, 810 lines): internal helpers `sha256` (4
  refs), `clampCap` (3), `resolveOptions` (2), `msSince` (8), `fmtDur` (5),
  `isFastifyInstance` (2) all used. `test/memprobe.ts` is imported by
  `test/safety.test.ts`. Clean.
- **python** (`sdk/python/src/laghound`): all module-private helpers
  (`_fill_chunks` 6 refs, `_norm_rate` 3, `_parse_bytes_param` 2,
  `_parse_content_length` 3, `_standalone_lifespan` 2, `bare_404` 22,
  `build_server_timing` 5, `reason_phrase` 3, `validate_mark` 3,
  `merge_marks_into_server_timing` 5, marks API 3 each, etc.) used within
  the package or its conformance tests. `tests/harness.py` is imported by
  both test modules. Clean.
- **csharp** (`sdk/csharp/LagHound.Endpoint`): every `Internal/` class is
  referenced (ByteBudget 19 refs, ConcurrencyGate 14, JsonBodies 5,
  KillSwitch 7, LagHoundRuntime 18, SdkVersion 3, ServerTimingHeader 7,
  TokenBucket 6). Clean.

---

## Ranked removal candidates

| # | Candidate | Est. lines/bytes | Confidence |
|---|---|---|---|
| 1 | ~40 dead `api` client methods (client.ts, families listed in A2) | ~180–200 lines | HIGH (legacy families) / MEDIUM (6 singletons) |
| 2 | `usePhaseSubscription.ts` + its test block | ~250 lines | HIGH |
| 3 | Dead type clusters in types.ts (Schedule, RunSummary, CloudStatus, BenchmarkProgress*, ComparePreset*) | ~80–120 lines | MEDIUM |
| 4 | `src/assets/hero.png` + `vite.svg` + `react.svg` | 57.7 KB | HIGH |
| 5 | Legacy schedule methods specifically (subset of #1; whole superseded API family) | ~45 lines | HIGH |
| 6 | Benchmark-config method family (subset of #1) | ~50 lines | HIGH |
| 7 | `testersApi.getRegions` + `getQueue` | ~10 lines | HIGH |
| 8 | `CLOUDS`, `DEPLOY_REGIONS`, `HTTP_STACKS` in testbed-constants.ts | ~10 lines | HIGH |
| 9 | `dashboard/networker-cloud.json` stray artifact | 3 lines | MEDIUM |
| 10 | Visibility-rule methods (subset of #1) | ~10 lines | HIGH |

Suggested sequencing if acted on: one PR for #2+#4+#7+#8+#9 (zero-risk,
purely additive deletions), a second PR for #1/#3/#5/#6/#10 together (client
methods + the types they reference must move in lockstep or `tsc` breaks),
re-running `tsc --noEmit` + `npm run build` + `npm test` after each.
