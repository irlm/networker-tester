# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---



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
