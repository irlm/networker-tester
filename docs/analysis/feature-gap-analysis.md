# Feature-Gap Analysis — Networker Platform

**Date:** 2026-07-18
**Scope:** Product capability gaps only. Code health was audited separately (90/100) and is
explicitly out of scope. Method: full-surface inventory (dashboard pages/wizards, ControlPlane
endpoints, tester CLI, docs promises) compared against what network/IT engineers and DevOps/SRE
teams need from this category (Grafana/Datadog synthetics, Catchpoint, ThousandEyes, k6 Cloud).

---

## 1. Current-Capability Inventory

Evidence-based: every row was confirmed by reading the surface, not assumed.

| Capability | Status | Evidence |
|---|---|---|
| Per-phase network probes (TCP/HTTP1-3/UDP/DNS/TLS/tlsresume/pageload/browser) | **Full** | `crates/networker-tester/src/cli.rs` — ~25 modes, payload sizes, presets, proxy/IPv4/IPv6, impairment profiles |
| Benchmark methodology (warmup/measured phases, adaptive stopping, CI targets, env/stability checks) | **Full** | `--benchmark-*` flag family, `baseline.rs`, `benchmark.rs`, frozen `benchmarks/shared/API-SPEC.md` |
| Cross-language benchmarking (17 reference APIs, orchestrator, leaderboard) | **Full (leaderboard API stubbed)** | `benchmarks/orchestrator/`, `LeaderboardPage.tsx`; C# `LeaderboardEndpoints.cs` returns empty arrays |
| Cloud tester provisioning (Azure/AWS/GCP VMs, auto-shutdown, lifecycle history, orphan reaping) | **Full** | `TestersEndpoints`, `CreateTesterModal`, `VmHistoryPage`, 8 background services incl. Reaper/Watchdog/AutoShutdown |
| Managed runs: wizard → dispatch → live results | **Full** | `NetworkTestPage` 4-step wizard, agent WebSocket dispatch, run detail w/ TTFB histogram |
| Schedules (cron, timezone, pause/resume, manual trigger) | **Full** | `SchedulesEndpoints.cs`, `SchedulerService.cs` (30s loop), `SchedulesPage.tsx` |
| Run comparison (side-by-side diff of 2+ runs) | **Full** | `RunComparePage.tsx`, comparison report API |
| Comparison groups (multi-cell matrix) | **Partial** | CRUD exists; per-cell run fan-out on launch is a deferred TODO (`ComparisonGroupsEndpoints.cs`, "deferred to M3") |
| Share links (public read-only run, expiry, revoke, access count) | **Full** | `ShareLinksEndpoints.cs`, `GET /api/share/{token}`, `ShareDialog` |
| Projects/workspaces + RBAC (viewer/operator/admin; platform admin) | **Full** | `useProject.ts`, policy set in ControlPlane, RBAC test suites |
| SSO (Microsoft/Google, provider CRUD, encrypted secrets) | **Full** | `SsoAdminEndpoints.cs`, `/sso-complete` route |
| Member management (invites, CSV bulk import, pending approval) | **Full** | `MembersEndpoints`, `InvitesEndpoints`, `ProjectMembersPage` |
| Command approval workflow (approve/deny infra ops, SSE feed) | **Full** | `ApprovalsEndpoints.cs`, `CommandApprovalsPage.tsx` |
| TLS profiles + URL diagnostics (HAR/PCAP capture, protocol force) | **Full** | `--url-test-*` / `--tls-profile-*` flags, `TlsProfilesPage`, `DiagnosticsPage` |
| Agent/endpoint version visibility + one-click update w/ live logs | **Full** | `SettingsPage.tsx`, `UpdateEndpoints.cs`, `VersionRefreshService` |
| Admin observability (system metrics, service logs w/ FTS, perf log, bench tokens) | **Full** | `AdminEndpoints`, `LogsEndpoints`, `PerfLogEndpoints`, `SystemDashboardPage` |
| Onboarding/empty states | **Full** | `EmptyState.tsx`, sparse-data detection on Dashboard, CTA flows |
| Responsive/mobile | **Full** | Tailwind `md:`/`lg:` throughout, hamburger sidebar, stacked cards |
| CLI machine output for automation | **Full** | `--json-stdout`, `--config` JSON/YAML, non-zero exit on error |
| Cost estimate (per-tester) | **Partial** | `GET .../testers/{id}/cost_estimate` + `CostRate` table exist; UI aggregation marked "follow-up" in `VmHistoryPage.tsx`; no spend rollup |
| Regression detection | **Stub** | `RegressionAnalyzer.cs` is data shapes only ("will be implemented"), not wired to any endpoint; `BenchmarkRegressionsPage` route exists but backend has no comparison logic |
| Email delivery | **Stub** | `EmailSenderExtensions.cs` marked TODO — forgot-password/invites have no real outbound mail |
| Cloud credential validation | **Stub** | `POST .../cloud-accounts/{id}/validate` does not contact providers |

**Absent entirely** (verified, no code found): alerting/threshold rules, notification channels
(Slack/Teams/webhook), historical time-series trend views, SLA/uptime reporting, public status
pages, user API tokens/PATs, result export (UI or API), user-configurable data retention,
project-scoped audit log, CLI assertion gates (`--max-latency` → exit 1), i18n, theme toggle.

---

## 2. Gap List — Importance (0–100, weighted for the target user) × Effort (S/M/L)

| # | Gap | Importance | Effort | Notes |
|---|---|---|---|---|
| 1 | **Alerting: threshold rules + notification channels (webhook/Slack/email)** | 95 | M–L | The category-defining feature of synthetic monitoring. Schedules run tests forever but nobody is told when latency regresses. Email sender is already stubbed; no rules engine, no channels, no alert history. |
| 2 | **Historical trend views (latency/throughput over days/weeks per endpoint)** | 90 | M | All the data is persisted per run; there is no time-series chart anywhere. Scheduled recurring tests are near-pointless without trends. Only single-run and run-vs-run views exist. |
| 3 | **API tokens + CI gate (PAT auth; CLI/API "fail if p95 > X" with exit code)** | 85 | M | No programmatic auth besides user JWT (24h) and agent keys — a pipeline cannot trigger a test and assert on results. CLI has `--json-stdout` but no blocking threshold flags (confirmed: benchmark thresholds are post-hoc reporting only). k6/Catchpoint users expect this. |
| 4 | **Baseline/regression detection wired user-visible** | 78 | M | `RegressionAnalyzer.cs` is an empty shell; the frontend regressions page has nothing real behind it. "Did last night's deploy make the network path slower?" is unanswerable without manual run-compare. |
| 5 | **SLA/uptime reporting (per endpoint: availability %, error budget, monthly report)** | 72 | M | `uptime_secs` field exists but is unused. Network engineers need to *prove* uptime to stakeholders — this is a report they currently must build by hand. |
| 6 | **Result export (CSV/JSON download from runs UI + export API)** | 70 | S | Zero download buttons, zero export endpoints. Data-dense product whose data cannot leave the product. Cheapest high-value fix on this list. |
| 7 | **Comparison-group launch fan-out (finish the deferred M3 TODO)** | 68 | M | Multi-region comparison is a promised capability whose launch endpoint returns 202 and dispatches nothing. Users can create matrices they cannot run. |
| 8 | **Cost visibility rollup (per-project spend dashboard, monthly estimate, budget warning)** | 65 | S–M | Endpoint + `CostRate` table exist; surface it: uptime × rate per tester, project rollup, column in VM history (already noted as follow-up in code). |
| 9 | **Real email delivery (unstub `EmailSenderExtensions`)** | 62 | S | Forgot-password and invites silently don't send. Blocks alerting (#1) too. `setup-guide.md` §6 already documents Azure Communication Services config. |
| 10 | **Public status page (opt-in, per project: endpoint health from scheduled runs)** | 55 | M | Natural extension of share links + schedules; expected in the category (UptimeRobot/Checkly), but external-facing and deferrable. |
| 11 | **Project-scoped audit log (who ran/deleted/changed what, admin-visible)** | 52 | M | Only platform service logs + command-approval history exist; no mutation trail. Matters for the enterprise/RBAC posture the product already invests in. |
| 12 | **User-configurable data retention (TTL per project, prune old runs/artifacts)** | 48 | S–M | Reaper cleans *stale* runs, not *old* ones; DB grows unboundedly under schedules. Becomes urgent the moment trends (#2) drive heavier scheduling. |
| 13 | **Leaderboard C# endpoints return real data (parity with Rust)** | 45 | S | Page exists; C# `LeaderboardEndpoints.cs` returns empty arrays — a silent post-migration regression. |
| 14 | **Cloud credential validation unstubbed** | 40 | S | `validate` returns success without contacting providers; users learn credentials are bad only when a VM create fails minutes later. |
| 15 | **Alert/notification history + quiet hours/dedup** | 38 | S | Sub-feature of #1; listed separately so #1's MVP can ship without it. |
| 16 | **Multi-region trend overlay (one chart, N testbeds)** | 35 | S | Sub-feature of #2 once time-series exists; the testbed matrix already gives the dimensions. |
| 17 | **Webhook on run completion (fire-and-forget, before full alerting)** | 33 | S | Minimal integration hook for teams with their own alert stack; subsumed by #1 but shippable alone. |
| 18 | **Theme toggle (light mode)** | 18 | S | Deliberate brand choice (terminal dark); some enterprise users will ask. Low priority. |
| 19 | **i18n** | 10 | L | Target users (network/SRE engineers) work in English tooling; not worth the surface-wide cost now. |

**Total gaps found: 19** (13 fully absent, 6 partial/stubbed).

Not gaps (verified present and adequate): run diffing, onboarding/empty states, responsive
layout, agent update UX, share links, SSO, RBAC role-differentiated UI, admin system
observability, CLI JSON output/config files.

---

## 3. Top-5 Recommended Roadmap

### 1. Alerting & notification channels — Importance 95, Effort M–L

This is the single wall every target user hits. The product already has the hard parts of a
synthetic-monitoring platform — scheduled recurring probes, per-phase metrics, multi-region
testers — but the loop never closes: when a scheduled run shows p95 latency doubling at 3 AM,
nothing happens. Grafana Synthetics, Catchpoint, and ThousandEyes are, at their core, exactly
this loop; without it Networker is a measurement tool, not a monitoring product, and schedules
are a feature users set up once and stop trusting. MVP: per-config threshold rules (metric,
comparator, N-consecutive-breaches), a generic webhook channel + Slack-compatible payload, and
the already-stubbed email sender made real. The `EmitRegressionEvent()` seam in
`RegressionAnalyzer.cs` and the SSE infrastructure in `ApprovalsEndpoints` show the plumbing
patterns already exist in-house.

### 2. Historical trend views — Importance 90, Effort M

Every run is persisted, yet no chart anywhere spans more than one run (RunCompare is N-run
side-by-side, leaderboard box-whiskers are per-language distributions). A network engineer
diagnosing "the app got slow last Tuesday" needs latency-over-time per endpoint per testbed —
that's the first screen in Datadog synthetics and the reason people schedule tests at all. The
data model needs nothing new: a time-bucketed aggregation endpoint over `test_run` +
per-endpoint trend page (p50/p95 bands, phase breakdown, region overlay) turns the existing
schedule feature from "cron that fills a table" into the product's retention driver. This also
supplies the baseline data that #1 (alerting) and regression detection consume.

### 3. API tokens + CI assertion gates — Importance 85, Effort M

DevOps/SRE teams — half the stated audience — live in pipelines, and today there is no way for
a pipeline to authenticate (only 24h user JWTs and agent keys) and no way for the CLI to fail
a build (confirmed: benchmark thresholds affect adaptive stopping and reporting, never exit
codes). Two thin slices close it: (a) personal/project access tokens with scopes, manageable
from the dashboard, honored by the existing JWT middleware; (b) CLI flags like
`--assert 'http2.p95<250ms'` returning exit 1 on breach, plus a `POST launch → poll → result`
recipe in docs. This is precisely k6 Cloud's adoption wedge: the tool that gates the deploy is
the tool that becomes infrastructure.

### 4. Wire regression detection end-to-end — Importance 78, Effort M

The most misleading gap in the product: a `BenchmarkRegressionsPage` route exists in the SPA
and `RegressionAnalyzer.cs` exists in the ControlPlane, but the analyzer contains only data
shapes ("real comparison will be implemented") and is wired to no endpoint — a user who finds
the page concludes the feature is broken, which is worse than absent. The benchmarking side has
rigorous methodology (CIs, IQR outlier rejection, stability gates), so the statistical
credibility to say "this is a real regression, not noise" already exists in `benchmark.rs`.
Implement median/CI comparison against a designated baseline run (or trailing window from #2),
persist regression rows, surface them on the existing page, and emit through the alerting
channel from #1. This converts three half-built assets into the platform's differentiator:
methodology-grade regression verdicts, not just threshold pings.

### 5. Result export (CSV/JSON) — Importance 70, Effort S

The cheapest wall to remove: a data-dense product for engineers whose data cannot leave the
UI — no download buttons, no export endpoints, so users screenshot tables into incident docs
and postmortems. The CLI already serializes full `TestRun` JSON (`--json-stdout`) and builds
Excel reports, so the shapes exist; add `GET /api/v2/test-runs/{id}/export?format=csv|json`
(and a filtered bulk variant on the runs list), plus download buttons on RunDetail, RunCompare,
and VM history. Ship it first — it's an S-effort trust win that also serves as the interim
answer for CI ingestion (#3) and offline analysis until trends (#2) land.

---

*Sequencing note: 5 (S, unblocks nothing but wins trust) → 2 (data foundation) → 1 (closes the
monitoring loop; do #9 email unstub inside it) → 3 (pipeline wedge) → 4 (differentiator on top
of 1+2). Finish the comparison-group fan-out (#7) opportunistically — it is a promised feature
whose launch button currently does nothing.*
