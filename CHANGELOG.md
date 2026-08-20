# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---



## [0.28.258] - 2026-08-20

### Fixed

- **GCP provisioning works from a host that never ran \`gcloud auth\`** (#827).
  The provisioner passed GOOGLE_APPLICATION_CREDENTIALS to the gcloud CLI, which
  does not read it — every GCP create failed with "no active account selected"
  while the account validated as active (the validator authenticates per-call).
  All gcloud invocations (create AND the start/stop/delete/describe lifecycle,
  which ran with no credentials at all) now pass the stored service-account key
  via CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE, stateless per invocation.

## [0.28.257] - 2026-08-20

### Fixed

- **Failed probe attempts show their reason** (#824). Each failed attempt row on
  the run detail page renders its recorded error message (ANSI-stripped,
  category-prefixed) next to the FAIL chip, and a protocol block whose failures
  share a dominant reason says so in the collapsed header ("5 FAIL — QUIC
  handshake timeout") instead of a bare count.

## [0.28.256] - 2026-08-20

### Fixed

- **Install diagnostics persist for post-mortem, and Windows caddy cells stop
  failing TLS-less** (#816). The deployment-events SSE route existed only in the
  retired Rust dashboard — the C# control plane now serves it (persisted-log
  replay for finished deployments, live ring+tail for active ones). The
  deployment row records exit_code and failed_step (V054), the persisted log is
  ANSI-scrubbed and tail-bounded, and failure messages carry the last fatal line
  and dying step. On Windows, the caddy stack declared a hostless TLS site under
  auto_https off and started certificate-less — every handshake failed while TCP
  checks stayed green; the Caddyfile now issues a real internal cert
  (default_sni localhost) and the in-guest verify asserts an actual TLS
  handshake.

## [0.28.255] - 2026-08-20

### Fixed

- **Install budgets age from install start, and infrastructure kills retry** (#817).
  The deploy timeout re-arms with the full scaled budget when install.sh reaches
  its install step, so quota-contended cloud provisioning no longer burns the
  window an AOT publish needs; the watchdog shares the same anchor. Kills during
  a control-plane shutdown and reaps of never-started deployments are marked
  retryable and go through the existing retry machinery instead of failing
  terminally; credential failures stay terminal.

## [0.28.254] - 2026-08-20

### Fixed

- **Multi-URL "Diag set" runs now update every member URL's watched row** (#820).
  Set configs join the probe-page watchlist, per-URL health verdicts come from
  the run's target_url-filtered attempts, and the `?host=` query param no longer
  double-encodes.

### Added

- **URL Probe burst sampling, hourly monitoring, and set comparison** (#782 P2).
  A Samples selector (1/3/5) runs each URL in a burst, watched rows can be
  monitored hourly via the existing scheduler, and set runs get a side-by-side
  per-phase median comparison table on the run detail page.

## [0.28.253] - 2026-08-19

### Fixed

- **The Benchmark Regressions page finally says why it is empty** (#810). An
  empty state now distinguishes "these runs were never compared" (with a
  pointer to how comparisons get produced) from "compared, no regressions
  found", and a run can be pinned as the comparison baseline directly from
  its detail page instead of the baseline being implicit.

## [0.28.252] - 2026-08-19

### Fixed

- **Launch-flow hardening, #793 slices (b)+(d) plus the last #791 item.** The
  wizard's review jump is gated until autoprovision actually completes, cost
  and runner notices state what they will really do (no more silent runner
  reuse surprises), "runner online" now means `power_state == running` AND
  `agent_status == online` everywhere the wizard checks readiness, and the
  cloud-account combobox looks like the control it is. Closes #793 and #791.

## [0.28.251] - 2026-08-19

### Fixed

- **URL Probe no longer 409s on previously-probed hosts** (#812). The probe
  page's find-or-create matched config names against the 200-newest list, so
  a config older than 200 rows (easy after heavy matrix testing) was missed
  and the create hit the UNIQUE(project_id, name) constraint. Config create
  now supports opt-in idempotency (`find_or_create: true` returns the
  existing row, race-free via the unique-violation catch), the config list
  gains an exact `?name=` filter that bypasses the cap, and the probe page
  uses both. Same failure class as the historical canary wedge — now dead
  product-wide.

## [0.28.250] - 2026-08-19

### Added

- **Local C# and Rust SDK demo deployments.** Runnable SDK examples
  (sdk/csharp/Example, sdk/rust/example) with Dockerfiles and local/Azure
  run scripts under examples/, plus an SDK examples panel and refreshed
  create-endpoint dialog on the SDK Endpoints page.

## [0.28.249] - 2026-08-19

### Fixed

- **The deploy wedge-watchdog enforces the SAME scaled budget as the deploy
  runner** (#804). It killed deploys at a flat 30 minutes while #740's scaled
  budget legitimately allowed more (a 1-language cpp deploy had 38m; the
  watchdog always won the race). The watchdog now computes the shared budget
  (+5m slack) per deployment, the reap re-checks the aging basis so a
  concurrent #785 recovery re-claim wins, and the timeout message states the
  enforced budget and language count — with the control-plane-restart hint
  only when a recovery actually occurred.

## [0.28.248] - 2026-08-19

### Added

- **Group-first benchmark results** (#803). Comparison groups render as ONE
  expandable row on the Runs list (live progress `X/N · F failed`, aggregate
  status, fastest-so-far chip) linking to the group's comparison page;
  benchmark wizards land on the comparison page after launch; run detail
  pages of group members gain a group breadcrumb and cell X-of-N prev/next
  navigation; the comparison page gets a completion banner listing failed
  cells with their reasons, and a Delete-group action. Standalone runs and
  individual run URLs are unchanged.

## [0.28.247] - 2026-08-19

### Fixed

- **.NET language installs unbroken across the board** (#801). net9-aot and
  net10-aot were missing from the Linux allowed-language list (instant
  install failure); plain net9 failed whenever a system dotnet existed (the
  SDK-channel check was skipped and the publish error silenced); launches
  now pin DOTNET_ROOT so dotnet-install runtimes resolve. AOT publishes get
  clang/zlib prerequisites on Linux. AOT on Windows is genuinely
  unsupportable (needs VS C++ Build Tools) and is now EXCLUDED at the wizard
  (languageAllowedOnOs both directions) and rejected server-side — no more
  doomed cells.
- **Caddy-on-Windows failures are no longer invisible** (#801 pattern B —
  4/4 systematic in the field). The Windows proxy setup surfaced no remote
  output, never retried Conflicts, and only warned on verify failure; now:
  output captured, Conflict retry, success marker required, ~90s serve
  verify (HTTP + HTTPS) inside install.ps1, UDP 8454 NSG rule for h3, and a
  fatal error with diagnostics instead of a silent 6-minute reachability
  death. One live Windows Caddy cell after deploy pinpoints any remaining
  in-guest cause.

## [0.28.246] - 2026-08-19

### Added

- **Production UI smoke harness** (`scripts/prod-smoke.sh` +
  `dashboard/e2e/prod/`): 11 read-only Playwright specs against the live
  dashboard — runs list + purpose tabs, URL probe, Start-a-test cards, both
  benchmark wizards' gates, comparison pivots, run detail (incl. the failed-
  run error banner), the canary panel, and system versions — each with a
  full-page screenshot and page-error collection. Auth by session-token
  injection (never a password); the runner auto-files a GitHub issue with
  the failing screenshot (secret gist) per failed spec, de-duped against
  open issues. Never mutates: no launches, no dispatches. Setup in
  docs/prod-smoke.md.

### Fixed

- RunsPage's time-filter unit test used hardcoded 2026-08-18 fixtures and
  became a date time-bomb (red on main once the fixture aged past the 24h
  window it asserted); timestamps are now relative to the test clock.

---

## [0.28.245] - 2026-08-19

### Fixed

- **CI: matrix-launch tests seed the active cloud account the #795 launch
  gate requires.** The tests referenced a cloud-account id no fixture ever
  created; once the gate (correctly) started failing cells whose account is
  missing or not active, all four tests failed with zero launched runs —
  blocking every C#-touching PR. The tests now seed an active account,
  matching the real-project contract the gate enforces.

## [0.28.244] - 2026-08-19

### Added

- **Start a test is now an incident-ready triage console.** Tests are grouped
  by the signal an operator needs, ranked without moving under the user during
  background refreshes, and checked against the same runner, endpoint, and
  cloud readiness rules used by the launch flows. Blocked tests lead directly
  to the required repair, recent non-provisioning configurations can be safely
  repeated, and all scenarios are searchable from the command palette. The
  complete flow supports persisted Vim-style keyboard controls with an explicit
  off switch and responsive, WCAG-checked layouts.

---
