# Re-score — Code Quality, Naming & Structure

Date: 2026-07-17 (afternoon) · Baseline: `docs/analysis/scorecard-code-quality.md` (2026-07-17 morning) · Method: read-only verification of PRs #444, #445, #448, #449, #450, #451 against the current tree, plus re-check of every baseline finding.

Letter mapping used for the baseline: A=95, A-=90, B+=87, B=83, B-=80, C+=77, C=73, C-=70, D=60.

## Score table

| # | Dimension | Was | Now | Delta |
|---|-----------|-----|-----|-------|
| 1 | Naming consistency | C+ / 77 | **87** | **+10** |
| 2 | Duplication & drift risk | C+ / 77 | **85** | **+8** |
| 3 | Module organization | B / 83 | **90** | **+7** |
| 4 | Comment / doc quality | B- / 80 | **82** | **+2** |
| 5 | Error handling | B+ / 87 | **87** | **0** |
| — | **Average (equal weights)** | **80.8** | **86.2** | **+5.4** |

All five claimed fixes (#444, #445, #448, #449/#450/#451) are **real and verified in the tree**. The gains are concentrated where the PRs landed; the baseline's comment-quality and error-handling findings were not part of this batch and are unchanged.

---

## 1. Naming consistency — 77 → 87 (+10)

**Verified fixed (#448):**

- `docs/branding.md` exists and is exactly the "naming map" the baseline asked for: one brand (**Networker**), a per-surface name table, an explicit *historical / deployment identifiers deliberately NOT renamed* section (`alethedash.com` infra stem, `alethabench` binary + cloud ids, release-asset chain rationale), and a **documented env-dialect list** (`DASHBOARD_*`, `AGENT_*`, `NETWORKER_*`, `BENCH_*`, `ENDPOINT_*`) with rationalization explicitly tracked as separate future work.
- UI coherent: `dashboard/index.html` `<title>Networker</title>`; `dashboard/package.json` renamed to `networker-dashboard` (was literally `dashboard`).
- `benchmarks/orchestrator/Cargo.toml` — description now "Networker Bench (alethabench)" with an inline comment explaining the historical name; the `aletha`/`alethe` prose-spelling drift is resolved by retiring the prose form entirely (branding.md documents this).
- `AletheDash` is gone from C# display strings — zero hits in `src/Networker.ControlPlane` (the baseline's `CliComputeProvisioner.cs:849` AWS SG description is fixed). Remaining `alethedash` hits in `dashboard/src` (8) are infra/deployment references, consistent with the stated policy.
- Root `README.md` now cross-references the deployment/binary names (the baseline's "no cross-reference exists" complaint).
- `Directory.Build.props` stamps `<Product>Networker</Product>` with a pointer to branding.md.

**Not done (deliberately deferred, but still real debt):** the C# ControlPlane still reads 13 `DASHBOARD_*` vars; the dual-name aliases (`DASHBOARD_DB_URL_NPGSQL`, `DASHBOARD_AZURE_SUBSCRIPTION`, `AGENT_DASHBOARDURL`) still exist. Documented ≠ rationalized — that caps this at high-B+ rather than A-range.

## 2. Duplication & drift risk — 77 → 85 (+8)

**Verified fixed (#445 — versions):**

- `Directory.Build.props` (repo root) is the single C# version source (`<Version>0.28.34</Version>` == `Cargo.toml` 0.28.34), scoped to `src/*` + `tests/*`, with `IncludeSourceRevisionInInformationalVersion=false` so the `+sha` suffix can't leak into `AgentVersionGate` parsing.
- The previously drifted constants are **gone, not just corrected**: `Networker.Agent.csproj` has no `<Version>` override; `ServerInfo.cs` and `VersionEndpoints.cs` derive from `AssemblyInformationalVersionAttribute` at runtime.
- CI guard extended: `ci.yml` `version-check` now asserts `Directory.Build.props == Cargo.toml` (lines 143–167) alongside the existing 4-way check.
- **Runtime guard added** (beyond what the baseline asked): `tests/Networker.Tests/VersionSingleSourceTests.cs` — 3 tests: agent/control-plane/assembly identity, dotted-triple normalization, and `AgentVersionGate.IsCompatible` on the derived version (protects the version-gated-dispatch failure mode the baseline flagged as borderline P0).
- `CLAUDE.md` "Version Sync" updated 3 → **5 locations** with the derived-values rule spelled out.

**Verified fixed (#445 — contract golden):** `tests/Networker.Tests/fixtures/tester-golden.json` is real captured tester output (`client_version 0.28.31`, replacing the hand-typed 0.28.13 string), regenerated via `scripts/regenerate-contract-golden.sh`, and `ContractRoundTripTests.cs:88` asserts the golden still carries unmodelled fields ("regenerate it from the real tester" if not). Partial credit only: it is a checked-in snapshot refreshed by script, not CI-generated per run from the live binary — a Rust-side rename between regenerations would still pass until the script is re-run.

**Verified fixed (#444 — schema):** `src/Networker.Data/Migrations/` now owns the DDL: `schema.sql` + the full `V002..V039` chain as embedded SQL resources, applied by `SchemaMigrator` using the **same `_migrations (version, applied_at)` bookkeeping** as the Rust runner. `tests/Networker.Tests/SchemaMigrationTests.cs` proves it with Testcontainers: fresh-DB build + every EF entity queries cleanly + zero-pending on a Rust-migrated database. The baseline's "worst finding" is closed. Residual: `SchemaMigrator.MigrateAsync` is invoked only from tests — production DDL application still runs through the Rust dashboard until cutover wires the migrator into ControlPlane startup or deploy.

**Unchanged:**
- **Mode/protocol lists — still ~6 unguarded manual copies** (`metrics.rs` enum, `mode-family.ts:17`, `testbed-constants.ts:268-312`, `PlatformEndpoints.cs:65-95`, `docs/deploy-config.md`), no shared manifest, no CI lint. This is the #377–379 bug class and is now the dimension's worst remaining item.
- TS API types still hand-written, no OpenAPI codegen.
- Dual Rust/C# endpoint impls (acceptable — Rust side dying).

## 3. Module organization — 83 → 90 (+7)

All five god-files split, verified move-only in the tree:

| Baseline god-file | Was | Now |
|---|---|---|
| `crates/networker-tester/src/output/html.rs` | 6,826 | `output/html/` — mod 255, charts 671, tables 288, protocol_sections 1,160, run_sections 875, render_multi 640, `tests/` 2,987 across 4 files; **plus** `tests/html_snapshot.rs` (deterministic single- and multi-run snapshot with version-normalized footer, added *before* the split as the safety net) |
| `crates/networker-tester/src/main.rs` | 2,460 | main.rs **361** + `dispatch.rs` 429 + `summary.rs` 425 + `target_runner.rs` 871 + `main_tests.rs` 981 |
| `benchmarks/orchestrator/src/reporter.rs` | 3,243 | `reporter/` — html 883, charts 485, comparison 398, assembly 347, **stats 253** (the baseline's requested extraction), text 213, tests 650, mod 72 |
| `benchmarks/orchestrator/src/executor.rs` | 2,361 | `executor/` — benchmark 864, cycle 739, vm 468, ssh_exec 280, status 158, mod 12 |
| `TesterWriteEndpoints.cs` | 1,734 | 5 partials — Create 706, Lifecycle 397, Schedule 264, base 251, Provisioning 195 |

Baseline's three top fixes: all three done. `CLAUDE.md`'s protocol-variant checklist was updated to the new layout (`dispatch.rs` / `summary.rs`) — no doc staleness introduced by the churn.

**Top-10 survey (current):** pageload.rs 3,817 · testers.rs 2,903 (legacy) · postgres.rs 2,662 · metrics.rs 2,613 · cli.rs 2,484 · migrations.rs 2,475 (legacy) · cloud_provider.rs 2,352 (legacy) · routes.rs 2,306 (legacy) · json.rs 2,079 · mssql.rs 1,949. Every survivor >1,500 lines is either a dying legacy-Rust crate or was rated *cohesive* by the baseline (embedded SQL, data model, flag count, per-protocol impls). No coordinator/concern-mixing god-file remains in maintained code. Frontend unchanged (`JobDetailPage.tsx` 1,116, `DiagnosticsPage.tsx` 1,042 — still >1,000, still clean-ish pages).

Not 95 because: `pageload.rs` (3,817, now the largest maintained file) got no treatment, the two >1,000-line React pages remain, and partial classes are the shallowest form of split (shared static class state, no enforced boundaries).

## 4. Comment / doc quality — 80 → 82 (+2)

**No stale-comment sweep happened.** Every specific bad example from the baseline is still present, verbatim:

- `AccountEndpoints.cs:112-113` — `// TODO(email): the Rust dashboard emails {public_url}/reset-password?token={token} via crate::email.` — Rust module-path syntax, still in a C# file.
- `AccountEndpoints.cs:49` (now ~line 49 unchanged) — `// Rust db::users::change_password: only active/pending accounts.` — provenance, not policy.
- `PerfLogEndpoints.cs` header — still defers to "the Rust `ensure_schema` note" (though the surrounding paragraph does state the C# behavior, softening it).
- `Rust `/`crate::`-style provenance comments remain pervasive across `Background/` services (`AutoShutdownService`, `WatchdogService`, `VersionRefreshService`, …). Many are the *good* kind (policy + numeric constants + "mirrors X" as citation), but the sweep the baseline prescribed did not run.
- `dashboard/src` still near comment-free; `NotificationBell.tsx` bare `catch` unchanged (file moved to `dashboard/src/components/`).

**Why +2 anyway:** the churn's *new* artifacts are exemplary — `Directory.Build.props` (complete bump-list + derived-values contract in a 20-line comment), `VersionSingleSourceTests.cs` (documents the exact historical drift bug it guards), `SchemaMigrationTests.cs` (states its proof strategy and its limits), `docs/branding.md`, `tests/html_snapshot.rs` (explains footer normalization). New modules got real module docs. The average moved; the flagged debt didn't.

## 5. Error handling — 87 → 87 (0)

All three flagged gaps verified **unchanged**:

- **No C# global 500 handler** — zero hits for `UseExceptionHandler`/`AddProblemDetails` in `src/Networker.ControlPlane/Program.cs`; unhandled exceptions still fall through to Kestrel's default 500 while the 4xx `{ error }` envelope stays uniform.
- **`ProvisioningOrchestrator.cs` silent swallow still there** (now at ~line 518): `catch (Exception) { return null; }` with no log, in the pending-endpoint JSON parse path. (The neighboring `FirstEndpointHost` catches the narrower `JsonException` — the pattern the swallow should copy, plus a log.)
- **String-matched classification unchanged**: `runner/http.rs:602` `e.to_string().contains("timed out")`; `runner/http3.rs` still string-matches `TransportError` messages for crypto/TLS (the `quinn::ConnectionError::TimedOut` structural arm predates the baseline). `metrics.rs:1439` `partial_cmp().unwrap()` NaN panic still present (line 1491 shows the safe idiom one screen away).

No regressions found from the churn — the splits were verifiably move-only (snapshot test + move-only commits), and no new bare catches or unwraps were introduced in the split modules. Held at 87.

---

## Remaining-fix list (rolled forward, re-prioritized)

### P1
1. **Global 500 handler** in `ControlPlane/Program.cs` emitting the existing `{ error }` envelope — only baseline P1 untouched by this batch.
2. **Shared modes manifest + guard** for the 6-way mode-list copy (TS × 2, C#, Rust enum, docs) — the only *unguarded already-materialized* drift class left (#377–379).
3. **Wire `SchemaMigrator` into production** (ControlPlane startup or deploy step) — ownership moved, application path hasn't; until then the deleted-crate risk merely became a "nobody runs it" risk at cutover.

### P2
4. Stale-comment sweep in `src/Networker.ControlPlane/Endpoints/` (`crate::`, `db::`, "the Rust dashboard emails…") — rewrite provenance as policy; keep the `ProbeRunResult.cs`-style references.
5. Log-then-return-null at `ProvisioningOrchestrator.cs:518`; typed classification at `http.rs:602` (reqwest `is_timeout()`) and `http3.rs` transport-error match; `total_cmp` at `metrics.rs:1439`.
6. Env-var rationalization (now documented in branding.md, still unexecuted): `CONTROLPLANE_*`/`NETWORKER_*` with deprecated `DASHBOARD_*` fallback; collapse the three alias pairs.

### P3
7. Automate golden regeneration (CI job runs `scripts/regenerate-contract-golden.sh` and diffs) so the contract snapshot can't silently age past a Rust-side shape change.
8. `pageload.rs` (3,817) split; JSDoc + `catch`-intent annotations in `dashboard/src`; TS API types from OpenAPI codegen; drop retired Rust crates from the clippy gate.
