# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

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

<<<<<<< HEAD
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
=======
- **Start a test is now an incident-ready triage console.** Tests are grouped
  by the signal an operator needs, ranked without moving under the user during
  background refreshes, and checked against the same runner, endpoint, and
  cloud readiness rules used by the launch flows. Blocked tests lead directly
  to the required repair, recent non-provisioning configurations can be safely
  repeated, and all scenarios are searchable from the command palette. The
  complete flow supports persisted Vim-style keyboard controls with an explicit
  off switch and responsive, WCAG-checked layouts.

---
