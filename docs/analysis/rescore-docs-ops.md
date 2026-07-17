# Re-score: Documentation Accuracy + Operational Readiness

Date: 2026-07-17 (evening) · Version audited: 0.28.34 (prod) · READ-ONLY re-score
against the same-day morning baseline [`scorecard-docs-ops.md`](scorecard-docs-ops.md)
(audited at 0.28.30). Intervening merges verified in-tree: #443 (required-check-safe
workflows, cargo-audit gate, EF log level), #445 (single-sourced versions, real
/api/health version), #447 (docs truth pass), plus #446/#448/#449–#451.

Grade→score mapping: A=95, A-=90, B+=87, B=83, B-=80, C+=77, C=73, C-=70.

## Score Table

| # | Dimension | Baseline | Now | Delta |
|---|-----------|----------|-----|-------|
| 1 | README + docs/ accuracy | C (73) | **A- (90)** | **+17** |
| 2 | CLAUDE.md accuracy | C+ (77) | **A (95)** | **+18** |
| 3 | CI/CD coherence | B- (80) | **B+ (87)** | **+7** |
| 4 | Observability / ops | B- (80) | **B+ (87)** | **+7** |
| 5 | Release/versioning docs | C- (70) | **A (95)** | **+25** |
| — | **Overall (equal weights)** | **C+ (76)** | **A- (90.8)** | **+14.8** |

---

## 1. README + docs/ accuracy — C (73) → A- (90)

Every baseline finding for this dimension is fixed, verified in the tree:

- **`architecture.md` rewritten** (`docs/architecture.md`): Mermaid + component
  table now show the hybrid reality — Rust probe core, `Networker.ControlPlane`
  (C#) as prod at alethedash.com behind nginx :5030, `Networker.Agent` on tester
  VMs, retired crates in a dedicated "Retired Components" section with the
  do-not-restart-in-prod warning. Covers all six crates incl. `networker-log`,
  `benchmarks/orchestrator`, and the Rust↔C# seam.
- **`docs/README.md` index complete**: all 13 current docs listed (the 5
  previously missing files are in), plus `archive/`, `analysis/`, `examples/`,
  `prs/`/`superpowers/` explained, and the frozen `benchmarks/shared/API-SPEC.md`
  pointer. Every listed file exists (checked against `git ls-files`). Task-based
  "By Task" section added.
- **Migration docs archived**: `docs/archive/hybrid-migration-plan.md` and
  `docs/archive/phase2-scope.md` — and no doc outside `archive/` still links the
  old paths (grep across README.md, docs/*.md, CLAUDE.md: zero hits).
- **`installation.md`** now carries the retired-Rust banner (lines 14–15) and
  the `dotnet build` / `dotnet run --project src/Networker.ControlPlane` paths.
- **`deploy-config.md`** `dashboard` object explicitly marked **Legacy — do not
  use for new deployments** (lines 56, 263–266).
- **`setup-guide.md`** stale `v0.14.5` pins: gone (grep returns nothing).
- **Cutover runbook** kept at `docs/phase2-cutover-runbook.md`; index entry
  correctly states cutover is complete and §4/§5/§7 remain operative.
- **No new staleness from the day's churn**: the #449–#451 move-only splits
  (`TesterWriteEndpoints.cs`, orchestrator `reporter.rs`/`executor.rs`, tester
  `main.rs`/`output/html.rs`) are referenced by **no** current doc (grep clean);
  `dispatch.rs`/`summary.rs`/`main.rs` cited by CLAUDE.md all still exist.
- **`branding.md` does not conflict**: it explicitly reconciles the Networker
  brand with the intentionally-unrenamed deployment identifiers
  (`alethedash.com`, `alethedash-cs`, `alethabench` et al.), which is exactly
  what the other docs and workflows use them as.

Remaining (why not A): `docs/branding.md` (added in #448, after the #447 index)
is **missing from the `docs/README.md` index** — the one-day-old index is
already one file behind, which is the exact failure mode the index is meant to
prevent. Minor: `architecture.md` labels `src/Networker.Endpoint` "port"
without saying whether it's on any deployment path (it isn't — correct but
terse).

## 2. CLAUDE.md accuracy — C+ (77) → A (95)

All four baseline-stale sections rewritten and **verified against reality**:

- **Crate table**: six crates (incl. `networker-log`) with status column;
  matches `crates/*/Cargo.toml` exactly. Retired crates marked; "Migration
  status: complete" with do-not-add-features rule; C# solution layout,
  `benchmarks/orchestrator`/`alethabench`, and reference-API solution all named.
- **Version Sync (5 locations)**: Cargo.toml, CHANGELOG.md, install.sh,
  install.ps1, `Directory.Build.props` — matches `Directory.Build.props`
  (Version 0.28.34, with the same 5-file list in its header comment) and the
  ci.yml `version-check` script, which enforces **all five** including
  props==Cargo.toml. The "everything else is derived" claim is true:
  `VersionEndpoints.DashboardVersion` reads `AssemblyInformationalVersion`
  (VersionEndpoints.cs:122–129) and Program.cs:115 wires it into `/api/health`.
- **Required checks**: CLAUDE.md lists 6; branch protection API returns exactly
  those 6: `Test (ubuntu-latest)`, `Test (windows-latest)`, `Detect changed
  areas`, `Build & audit (C#)`, `bats (installer unit tests)`, `shellcheck`.
  Both job names exist in ci.yml (`Detect changed areas` line 30).
- **Local dev**: "Control Plane Local Dev (C#)" — `dotnet run` for control
  plane (:5030) and agent, fail-closed env vars, explicit "Do NOT run the
  retired Rust dashboard/agent for dev". `dotnet build`/`dotnet test` added to
  build/test sections.
- **Protocol checklist**: updated to `dispatch.rs`/`summary.rs` (both exist);
  apibench runner-level-mode note added — closing the baseline's P2 item too.

Remaining: nothing material found. (Nit: the checklist's "Throughput/payload
size mapping in `main.rs`" survived the #451 split — still true, `main.rs`
exists.)

## 3. CI/CD coherence — B- (80) → B+ (87)

Fixed since baseline:

- **Required set corrected and expanded**: 2 → 6 required checks (API-verified),
  now including the prod-critical `Build & audit (C#)` — the baseline's P0 #1.
- **Required-check-safe pattern**: `dotnet.yml` and `test-installer.yml` PR
  triggers deliberately drop `paths:` filters; a cheap `changes` job gates the
  real work while skipped jobs still report passing check runs (documented
  in-file, mirrors ci.yml). A Rust-only PR can no longer merge past a broken C#
  build, and the checks can't wedge on "Expected — waiting for status".
- **`azure-pipelines.yml` deleted** (baseline P1 #7) — the dead artifact that
  would have deployed the retired Rust stack is gone from the tree.
- **New `rust-audit.yml`**: cargo-audit over the workspace **and** the
  orchestrator lockfile (`--file benchmarks/orchestrator/Cargo.lock`), with the
  same required-check-safe change detection; last 3 runs green. Closes the
  version-check C# gap too (props enforcement, §5).

Remaining (why not higher):

1. **Orchestrator still never linted/tested in CI** (baseline gap #3, P1 #8
   unaddressed): no workflow runs `cargo clippy/fmt/test` against
   `benchmarks/orchestrator/Cargo.toml` — it is only *audited* (rust-audit.yml)
   and *built* (release.yml). The clippy debt cleared in #441 (53→0) has no
   regression guard.
2. **`auto-tag` still `needs: [lint, test-ubuntu, frontend]`** (ci.yml:515) —
   no C# dependency on the tag/deploy path. Largely mitigated now that
   `Build & audit (C#)` gates every merge, but a red C# build on the main push
   itself would not stop the tag.

## 4. Observability / ops — B- (80) → B+ (87)

Fixed:

- **EF Core SQL spew silenced**: `src/Networker.ControlPlane/appsettings.json`
  line 6 — `"Microsoft.EntityFrameworkCore": "Warning"` (#443).
- **`/api/health` real version**: `hybrid-phase2-poc` appears nowhere in `src/`;
  Program.cs:115 sets `version = VersionEndpoints.DashboardVersion`, which is
  assembly-derived from `Directory.Build.props` (#445) — so it can never skew
  again by construction.
- **Soak-check healthy**: latest scheduled run (2026-07-17, cron `47 6 * * *`)
  **success**; the 2026-07-16 22:09 dispatch failure was pre-fix, followed by a
  green re-dispatch the same night. (Run history for the workflow is short —
  the required-check refactor reset it; streak counting restarts.)

Unchanged (why not higher):

- **Alerting is still only a red Actions run** — no issue creation, webhook, or
  notification in soak-check.yml or anywhere else (baseline P1 #9 open). The
  "14 consecutive green soak runs" decommission gate is still eyeball-counted,
  and now from a shorter history.
- No consolidated "prod at a glance" page yet (baseline P2 #12) — ops truth
  still spans runbook + release.yml comments + deploy-dashboard.sh, though
  release-flow.md now covers the deploy slice well.

## 5. Release/versioning docs — C- (70) → A (95)

- **`docs/release-flow.md` exists** (#447) and is the prose doc the baseline
  said was missing — one-line flow, the 5 CI-enforced bump locations, the
  SHIPPING-filter exemption for docs/C#-only PRs, auto-tag mechanics (incl. the
  Actions-token explicit-dispatch subtlety), the deploy-first graph, full asset
  inventory, two-level rollback, post-release checks.
- **Verified against release.yml**: prevbuild swap (lines 415–416), 30 s
  readiness poll on `/api/health/ready` (452–456), auto-rollback restore
  (467–469), `DASHBOARD_PUBLIC_URL` assertion into `/etc/alethedash-cs.env`
  (406–412), public nginx-path verification (478), orchestrator/`alethabench`
  build+deploy steps, C# asset names. The `gh release delete` nuance for manual
  re-dispatch matches `gh release create` semantics.
- **version-check enforces props==Cargo.toml**: ci.yml extracts
  `Directory.Build.props` `<Version>` and hard-fails on mismatch, with the
  5-file list in the error text; `Directory.Build.props` pins
  `InformationalVersion` to the dotted triple (no `+sha` suffix) so derived
  runtime versions stay parseable. Docs (CLAUDE.md, props header, ci.yml error,
  release-flow.md) all state the same 5-location list — no contradiction found.
- Linked from both `docs/README.md` (index + two task entries) and CLAUDE.md.

Remaining nit: release-flow.md §1's "5 locations, CI-enforced" is enforced only
when the SHIPPING filter trips — stated correctly two paragraphs later, but a
skim of the heading alone could over-promise. Cosmetic.

---

## Remaining fixes, prioritized

1. **Add orchestrator lint/test to CI** — `cargo clippy --manifest-path
   benchmarks/orchestrator/Cargo.toml --all-targets -- -D warnings` (+ fmt/test)
   in ci.yml or rust-audit.yml. Only gap from the baseline P0/P1 list that
   touches a shipping binary (`alethabench`) with zero regression guard.
2. **Minimal soak alerting** — on soak-check failure, open/comment a pinned
   GitHub issue so failures outlive the Actions list and the 14-green-run
   decommission streak becomes machine-countable (matters more now that the
   workflow's run history was reset).
3. `docs/README.md`: add `branding.md` to the index.
4. ci.yml: add the C# build to `auto-tag`'s `needs` (or accept the PR-gate
   mitigation and note it in release-flow.md).
5. (Carry-over P2) one-page "prod at a glance" — services/ports/env-files/URLs —
   at the top of the cutover runbook, cross-linking `deploy-dashboard.sh`.
