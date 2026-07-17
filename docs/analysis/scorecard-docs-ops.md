# Scorecard: Documentation Accuracy + Operational Readiness

Date: 2026-07-17 · Version audited: 0.28.30 · Scope: READ-ONLY review post C#-cutover
(prod = C# control plane at alethedash.com; Rust dashboard/agent soaking until ~July 30).

Verified live: branch-protection required checks fetched via
`gh api repos/irlm/networker-tester/branches/main/protection` → `["Test (ubuntu-latest)","Test (windows-latest)"]`.

---

## Grade Table

| # | Dimension | Grade | One-line justification |
|---|-----------|-------|------------------------|
| 1 | README + docs/ accuracy | **C** | README is genuinely current (all links resolve, C# shown as prod), but the doc it leads with — `architecture.md` — still diagrams the retired Rust control plane as current, `installation.md`/`deploy-config.md` teach the Rust stack unmarked, and `docs/README.md` omits 5 real files. |
| 2 | CLAUDE.md accuracy | **C+** | 5 of 10 sections verified accurate (build/test cmds, protocol checklist, auto-tag, Gist), but the highest-leverage sections are wrong: "Version Sync (3 locations)" misses both C# constants, "five crates" misses `networker-log`, local-dev teaches the retired Rust dashboard, and the required-checks list doesn't match actual branch protection. |
| 3 | CI/CD coherence | **B-** | The machinery is strong (version-check gate, auto-tag, deploy-first release graph, nightly soak, weekly 15-language bench validation) — but governance lags: only 2 checks are actually required, the prod-critical C# build is skippable on Rust-only PRs, the orchestrator is never linted in CI, and `azure-pipelines.yml` is a dead artifact that would deploy the wrong stack. |
| 4 | Observability / ops | **B-** | Excellent health surface (4 endpoints incl. per-service `/api/health/background`) and an automated nightly soak, plus a genuinely good cutover runbook — undercut by zero alerting beyond red Actions runs, EF Core SQL spew at Info level in prod, and `/api/health` reporting `version: "hybrid-phase2-poc"`. |
| 5 | Release/versioning docs | **C-** | The release flow itself is well-built and richly commented *inside* release.yml, but no prose doc exists anywhere a new maintainer could follow; the version lives in 6 places, only 3 are documented, and only the Rust/installer subset is CI-enforced. |
| — | **Overall** | **C+** | Automation outran documentation: the pipelines are near-A, the written record of them is D-territory, and the two meet in the middle. |

---

## 1. README + docs/ — Per-Doc Verdict Table

README.md: **CURRENT**. Correctly presents `Networker.ControlPlane` (C#) as prod, marks Rust
dashboard/agent/common as pending decommission, differentiates `cargo build` vs
`dotnet build Networker.sln`. All 9 relative links checked — zero broken. No stale version pins.

docs/README.md (index): **INCOMPLETE** — omits 5 real files: `dotnet-migration.md`,
`hybrid-migration-plan.md`, `phase2-cutover-runbook.md`, `phase2-scope.md`,
`tls-endpoint-profile-phase1-checklist.md`. Also never mentions `docs/analysis/` or the frozen
`benchmarks/shared/API-SPEC.md` (v1, frozen 2026-07-16 — referenced only from deploy-config.md §apibench).

| File | Verdict | Evidence |
|------|---------|----------|
| `architecture.md` | **STALE — worst offender** | Mermaid diagram (lines 20–34) + crate table (39–45) show Rust `networker-dashboard`/`networker-agent`/`networker-common` as the current control plane; `Networker.ControlPlane` appears nowhere. This is the doc README's "Architecture" link points to. |
| `installation.md` | **STALE** | Lines 53–54 list `target/release/networker-dashboard`/`networker-agent` as build outputs; lines 137–175 walk through starting the retired Rust dashboard/agent via cargo with no legacy warning and no C# path. |
| `deploy-config.md` | **STALE (dashboard section)** | Lines 290–295: `dashboard` schema object still described as deploying the Rust `networker-dashboard` (axum) + `networker-dashboard.service`. The apibench/modes content is current. |
| `setup-guide.md` | **CURRENT (minor)** | C#-focused "AletheDash Setup Guide"; only blemish: stale `v0.14.5` tag pins at lines 608, 734. |
| `phase2-cutover-runbook.md` | **CURRENT** | Dated 2026-07-16; accurate post-cutover ops reference (soak checklist, rollback, decommission criteria = 14 green soak runs). Best ops doc in the tree. |
| `phase2-scope.md` | **SUPERSEDED — should archive** | M0–M6 milestone plan written forward-looking; cutover is done. Mark `[HISTORICAL — completed 2026-07]`. |
| `hybrid-migration-plan.md` | **SUPERSEDED — should archive** | Still says "Phase 2 proof-of-concept working"; migration finished. Keep for decision rationale, banner it as historical. CLAUDE.md still cites it as the live plan. |
| `dotnet-migration.md` | **CURRENT** | Accurate seam/contract reference (schema_version'd JSON), dated 2026-07-06. Missing from index. |
| `cloud-auth.md` | **CURRENT** | Zero-credential federation for the C# control plane; accurate. |
| `config-examples.md` | **CURRENT** | All 8 referenced `examples/configs/` files exist. |
| `probes.md` | **CURRENT** | Rust tester probe modes — the permanent core; accurate. |
| `testing.md` | **CURRENT** | tester/endpoint protocol comparison; no control-plane dependency. |
| `tls-endpoint-profile-design.md` | **CURRENT** | Feature design doc, no stale refs. |
| `tls-endpoint-profile-phase1-checklist.md` | **CURRENT** | Companion checklist; missing from index. |

---

## 2. CLAUDE.md — Section Verdicts

| Section | Verdict | Evidence |
|---------|---------|----------|
| Crate table ("five crates") | **STALE** | Workspace has **six** members — `networker-log` missing (Cargo.toml lines 2–9). Table also presents Rust dashboard/agent without legacy marking; C# migration note below it says "in progress" (it's complete, soaking). `benchmarks/orchestrator` (excluded Rust project, ships as `alethabench`) unmentioned. |
| Build commands | ACCURATE (Rust) | All verified; but no `dotnet build Networker.sln` / `dotnet test` despite C# being prod. |
| Test commands | ACCURATE | integration.rs, installer.bats, endpoint tests, npm commands all verified to exist. |
| Dashboard Local Dev | **STALE** | Steps 3–4 launch the retired Rust `networker-dashboard`/`networker-agent`. Real dev flow is `dotnet run` in `src/Networker.ControlPlane` / `src/Networker.Agent`. Frontend step still correct. |
| Version Sync "(3 locations)" | **STALE — highest risk** | Actual locations for 0.28.30: Cargo.toml, CHANGELOG.md, install.sh, install.ps1, **`src/Networker.ControlPlane/Program.cs:90`** (`AddVersionRefresh("0.28.30")`), **`.../Endpoints/VersionEndpoints.cs:119`** (`DashboardVersion`). 6 places; CLAUDE.md documents 3; ci.yml version-check enforces only the Rust/installer subset. An AI session following CLAUDE.md will ship version skew in prod's own `/api/version`. |
| Protocol Variant checklist | ACCURATE | Protocol enum (metrics.rs:442), dispatch_once/log_attempt (dispatch.rs), print_summary (summary.rs) all exist. Note: doesn't cover the newer apibench runner-level-mode pattern (benchmarks/configs/apibench.json + API-SPEC §4). |
| Git Workflow / required checks | **STALE** | Claims 4 required checks; **actual branch protection requires only 2**: `Test (ubuntu-latest)`, `Test (windows-latest)` (verified via API). bats/shellcheck are not actually required, and no C# check is. Auto-tag and Gist-sync claims verified accurate. |
| Installer constraints + Gist | ACCURATE | sync-gist.yml exists; Gist ID matches; manual fallback command matches. |

Score: 4 of ~9 sections stale — and they are the sections future AI sessions rely on most.

---

## 3. CI/CD — Workflow Map

| File | Name | Trigger | Purpose |
|------|------|---------|---------|
| `ci.yml` | CI | push main/develop, PR→main | detect-changes, version-check gate, lint, Test (ubuntu/windows), frontend, coverage, SQL/PG integration, **auto-tag on main push** (needs lint+test-ubuntu+frontend — no C#) |
| `dotnet.yml` | .NET | push/PR, **path-filtered** to `src/**`, `tests/Networker.*/**`, sln | Build & audit (C#): restore/build Release, vuln audit, xUnit |
| `release.yml` | Release | tag `v*`, dispatch | Deploy-first graph: build-linux + build-csharp → release → **deploy to alethedash.com** (az vm run-command) → native builds attach async |
| `soak-check.yml` | Prod soak check | daily 06:47 UTC, dispatch | 5-point prod checklist: /api/health 200, `all_healthy` on /api/health/background, stuck-queue=0, Rust services inactive, advisory locks ≤2, orphan-VM guard |
| `validate-bench-apis.yml` | Validate Benchmark APIs | weekly Mon 06:00, PR/push on `benchmarks/**`, dispatch | template-drift, Rust baseline, language matrix (8 on PR / 15 weekly) vs frozen API-SPEC |
| `benchmark.yml` | Benchmarks | weekly Sun, dispatch | Runs the 17-language suite |
| `test-endpoint.yml` | Endpoint Tests | push/PR, path-filtered | endpoint cargo test + bats + JSON API validation (minor overlap with ci.yml — acceptable) |
| `test-installer.yml` | Installer Tests | push/PR, path-filtered | shellcheck, bats, PSScriptAnalyzer |
| `sync-gist.yml` | Sync install scripts to Gist | push main (installer paths) | Updates public bootstrap Gist |
| `wiki-setup.yml` | Initialize Wiki | dispatch | One-time wiki bootstrap |

(10 workflows, not 9 — plus `azure-pipelines.yml` at repo root: **dead artifact**, `trigger: none`,
downloads retired Rust `networker-dashboard` tarballs and restarts the wrong systemd service; would
conflict with release.yml's deploy if its webhook ever fired. Delete it.)

### Gaps / overlaps

1. **C# build is not a required check and is path-filter-skippable** — prod-critical code can break
   on a Rust-only PR and merge green; `auto-tag` doesn't depend on it either.
2. **Actual required set is thinner than documented**: only `Test (ubuntu-latest)` + `Test (windows-latest)`.
   bats/shellcheck (claimed required in CLAUDE.md) are not enforced by branch protection.
3. **benchmarks/orchestrator (alethabench) is never linted/tested in CI** — excluded from the
   workspace, so `ci.yml:lint` skips it; it's only *built* in release.yml. The just-cleared clippy
   debt (commit 723f5a25) has no regression guard.
4. **version-check doesn't cover the C# version constants** (Program.cs / VersionEndpoints.cs).
5. soak-check ↔ validate-bench-apis: independent by design (post-deploy monitoring vs pre-release
   contract validation); neither gates deploy — acceptable, but see alerting gap below.

---

## 4. Observability / Ops

**Health surface — strong.** `/api/health` (public liveness+DB), `/api/health/ready` (readiness),
`/api/health/background` (per-service tick telemetry for 8 background services, `all_healthy`
aggregate, 3× interval tolerance — the soak signal), `/api/system/health` (admin).
One wart: **`Program.cs:113` hardcodes `version = "hybrid-phase2-poc"`** in the public health
response while the real version is 0.28.30.

**Alerting — minimal.** Nightly soak failure = a red Actions run + `::error::` annotation. No issue
creation, no Slack/webhook/email/PagerDuty anywhere in src/, scripts/, or workflows. The soak's
"14 consecutive green runs" decommission gate is counted by eyeballing Actions history.

**Log hygiene — regression still present.** `src/Networker.ControlPlane/appsettings.json` sets only
`Default: Information` and `Microsoft.AspNetCore: Warning`; **`Microsoft.EntityFrameworkCore` is
unset → inherits Information → SQL command spew in prod logs**. No appsettings.Production.json, no
code-level filter. The previously flagged noise was never fixed.

**Runbook — good but split across three places.** `phase2-cutover-runbook.md` is a genuinely
current 367-line ops runbook (env, leader election, cutover steps, soak checklist, rollback,
decommission criteria). But "how prod works" also lives in `scripts/deploy-dashboard.sh` (infra
from scratch — not referenced from any doc) and `release.yml` deploy job comments (service names,
env file `/etc/alethedash-cs.env`, `DASHBOARD_PUBLIC_URL` assertion). No single services/ports/env
/deploy/rollback page ties them together.

---

## 5. Release / Versioning

**Reconstructed actual flow** (from ci.yml + release.yml — nowhere in prose):
PR → version-check gate (Cargo.toml + CHANGELOG + installers only) → merge → **auto-tag** from
Cargo.toml on main push → release.yml: `build-linux` (tester, endpoint, alethabench, frontend) ∥
`build-csharp` (`networker-controlplane-linux-x64.tar.gz`, `networker-agent-cs-linux-x64.tar.gz`,
`networker-agent-cs-win-x64.zip`) → GitHub release (notes from CHANGELOG) → **deploy job** to
alethedash.com (stop `alethedash-cs`, backup `.prevbuild`, extract, assert env, restart, verify
`all_healthy`) → macOS/Windows native builds attach asynchronously. Rust dashboard/agent are off
the train (#424).

**Documentation of this flow: effectively none.** CLAUDE.md's Git Workflow (5 lines) + the stale
"3 locations" section is the entire prose record. No CONTRIBUTING.md, no docs/release-process.md.
A new maintainer must reverse-engineer two YAML files — well-commented YAML, but YAML.

---

## Prioritized Fixes

### P0 — this week
1. **Make the C# build a required check.** Add an always-run thin job (or drop the path filters on
   dotnet.yml's PR trigger), add `Build & audit (C#)` to branch protection, and add it to ci.yml's
   `auto-tag` needs. Also re-add bats/shellcheck to actual branch protection or stop claiming they
   are required. Today a Rust-only PR can break prod's control plane and auto-tag a release.
2. **Fix the version-sync record and enforcement.** CLAUDE.md "3 locations" → 6 locations
   (add `Program.cs:90` + `VersionEndpoints.cs:119`; ideally collapse the two C# constants into one
   single-sourced const — already noted as deferred in the trust-audit train). Extend ci.yml
   version-check to grep the C# constants. While there: fix `/api/health`'s
   `version = "hybrid-phase2-poc"`.
3. **Silence EF Core SQL spew**: add `"Microsoft.EntityFrameworkCore": "Warning"` to
   `src/Networker.ControlPlane/appsettings.json` (or an appsettings.Production.json).

### P1 — before Rust decommission (~July 30)
4. **Rewrite `architecture.md`** around the C# control plane; mark Rust components `[LEGACY]`.
   It's the flagship architecture doc and it describes the retired system.
5. **Legacy-banner `installation.md` + `deploy-config.md` dashboard sections**; add the C# start
   path; fix setup-guide's `v0.14.5` pins.
6. **Write `docs/release-process.md`** — the reconstructed flow above, plus asset names and the
   deploy/rollback (`.prevbuild`) procedure — and link it from docs/README.md and CLAUDE.md.
7. **Delete `azure-pipelines.yml`** (dead, and dangerous if its webhook ever fires).
8. **Add orchestrator lint/test to ci.yml** (`cargo clippy/fmt/test --manifest-path
   benchmarks/orchestrator/Cargo.toml`) to guard the just-cleared clippy debt.
9. **Minimal alerting**: soak-check failure opens/comments a pinned GitHub issue (and the soak
   streak becomes machine-countable for the decommission gate).

### P2 — housekeeping
10. Update CLAUDE.md: six-crate table (+`networker-log`), C# local-dev flow (`dotnet run`),
    mention benchmarks/orchestrator + frozen API-SPEC, add apibench note to the protocol checklist.
11. docs/README.md index: add the 5 missing files + docs/analysis/ + API-SPEC pointer; banner
    `hybrid-migration-plan.md` and `phase2-scope.md` as `[HISTORICAL]`.
12. Consolidate a one-page "prod at a glance" (services, ports, env files, URLs) at the top of the
    cutover runbook or a new ops.md, cross-linking deploy-dashboard.sh and release.yml.
