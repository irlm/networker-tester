# Scorecard — Code Quality, Naming & Structural Consistency

Date: 2026-07-13 · Scope: full repo (Rust crates, C# `src/Networker.*`, `dashboard/` React, `benchmarks/`) · Method: read-only audit, four parallel deep dives (naming, duplication/drift, module organization + comments, error handling).

## Grade table

| # | Dimension | Grade | One-line justification |
|---|-----------|-------|------------------------|
| 1 | Naming consistency | **C+** | Engineering-level naming (files, DTOs, routes, per-stack conventions) is A-grade, but the product identity is fractured three ways (Networker / AletheDash / AletheBench) and the config namespace (`DASHBOARD_*` driving a "ControlPlane", `BENCH_*` driving an "Endpoint") sells it down hard. |
| 2 | Code duplication & drift risk | **C+** | The two highest-value seams (probe JSON contract test, 4-way version CI check) are genuinely guarded — but C# version constants have **already drifted** (0.28.26/0.28.28 vs 0.28.30), the DB schema's single source of truth sits in a crate scheduled for deletion in ~2 weeks, and mode lists are a 6-way manual copy protected only by a checklist. |
| 3 | Module organization | **B** | Directory-level separation is genuinely good across all four stacks; held down by ~5 coordinator god-files (tester `main.rs`, orchestrator `reporter.rs`/`executor.rs`, `TesterWriteEndpoints.cs`, legacy `testers.rs`) and one 6.8k-line HTML renderer. |
| 4 | Comment / doc-comment quality | **B-** | Rust measurement code is A-grade (WHY-comments with trust-audit citations), but C# endpoints carry systemic stale Rust-era copy-paste comments (`crate::email` in C# files) and the TS dashboard is nearly comment-free. |
| 5 | Error-handling consistency | **B+** | One coherent taxonomy per layer, consistently applied (Rust `ErrorCategory` structural classification, C# `{ error }` envelope, frontend `ApiError` client); docked for the missing C# global 500 handler, a few string-matched Rust classifications, and silent-swallow catches. |
| — | **Overall** | **B-** | Solid per-stack craftsmanship; the fast port left behind a fractured brand, unguarded cross-stack copies (some already drifted), and inherited god-files. |

---

## 1. Naming consistency — C+

**Crate/project naming (clean per stack).** Rust: 6 crates uniformly `networker-{tester,endpoint,common,dashboard,agent,log}` (`Cargo.toml`). C#: `Networker.{Contracts,Data,Agent,ControlPlane,Security,Endpoint}` + mirrored `*.Tests` (`Networker.sln`). Intra-stack file naming is disciplined everywhere: Rust snake_case, C# PascalCase, React PascalCase.tsx / camelCase.ts — no outliers found.

**Brand identity — confirmed three-way split.** ~4,700 "networker" hits vs 275 "alethedash" vs 124 "alethabench":

- Installers and README say **"Networker Tester"** (`install.sh`, `install.ps1`, root `README.md`).
- Users see **"AletheDash"**: `dashboard/index.html` `<title>`, `usePageTitle.ts`, Login/ChangePassword/ShareView pages; prod infra is `alethedash-rg`, `/opt/alethedash/`, `alethedash.com` (`azure-pipelines.yml:19-72`); it even leaks into C# provisioning (AWS SG description "AletheDash tester", `src/Networker.ControlPlane/Provisioning/CliComputeProvisioner.cs:849`).
- **AletheBench** owns `benchmarks/orchestrator` (package `alethabench-orchestrator`, binary `alethabench`) — with an internal spelling drift: the same `Cargo.toml` description spells it "AletheBench" (`aletha` vs `alethe`).
- `dashboard/package.json` is named literally **`dashboard`** — matches none of the three brands.

A new engineer installs "Networker Tester", logs into "AletheDash", and benchmarks with "alethabench" — no cross-reference exists in the root README.

**Env var families.** Clean per component (`DASHBOARD_*` ×18, `AGENT_*`, `NETWORKER_*`, `ENDPOINT_*`, `BENCH_*`; no `CONTROLPLANE_*`/`ALETHE_*` exist), but with legacy seams:

- The C# **ControlPlane** reads 13 `DASHBOARD_*` vars (`AuthExtensions.cs:29`, `CredentialCipherExtensions.cs:36`, `InvitesEndpoints.cs:453-475`) — cutover-compatible, but a permanent name mismatch once Rust dies.
- Dual-name aliases for the same setting: `DASHBOARD_DB_URL` / `DASHBOARD_DB_URL_NPGSQL` (`ControlPlane/Program.cs:30-32`); `AZURE_SUBSCRIPTION_ID` / `DASHBOARD_AZURE_SUBSCRIPTION` (`TesterWriteEndpoints.cs:711-715`); C# Agent accepts both `AGENT_DASHBOARDURL` and `AGENT_DASHBOARD_URL` via `ApplyRustEnvFallbacks()` (`Agent/Program.cs:9,25-27`).
- The Rust endpoint consumes `BENCH_*` (`crates/networker-endpoint/src/routes.rs:450,1136`) while the C# endpoint uses `ENDPOINT_*` — same logical component, different family per stack.

**Wire naming — strongest area.** C# Contracts pin snake_case via `[JsonPropertyName]` on every field, matching Rust serde exactly; routes match across stacks (`/api` + `/api/v2`, plural kebab-case resources, action sub-paths). No mismatches found.

**Top fixes:** (1) pick a brand, add a naming map to README, rename `dashboard/package.json`, fix `alethabench`/`AletheBench` spelling; (2) introduce `CONTROLPLANE_*` (or `NETWORKER_*`) with `DASHBOARD_*` as deprecated fallback, centralized in one config extension; (3) collapse the three alias pairs behind single documented names with startup deprecation warnings.

## 2. Code duplication & drift risk — C+

| Duplication | Copies | Guard status | Evidence |
|---|---|---|---|
| Rust tester JSON ↔ C# Contracts | 2 | **GUARDED (partially)** — `tests/Networker.Tests/ContractRoundTripTests.cs` (5 tests: round-trip, unknown fields, null phases, missing `schema_version`) — but the fixture is a hand-typed string pinned at `client_version 0.28.13`, not generated from the real binary | `src/Networker.Contracts/ProbeRunResult.cs`, `crates/networker-tester/src/metrics.rs:162-273`, `main.rs:1416` |
| Version constants | **8** (CLAUDE.md still says 3) | **SPLIT** — Cargo.toml / CHANGELOG / install.sh:327 / install.ps1:62 guarded by `ci.yml` `version-check` (lines 79-145); the 4 C# copies UNGUARDED and **2 already drifted**: `Networker.Endpoint/ServerInfo.cs:15` = 0.28.28, `Networker.Agent.csproj:11` = 0.28.26 vs workspace 0.28.30 | `ControlPlane/Program.cs:90`, `VersionEndpoints.cs:119` |
| C# Endpoint ↔ Rust endpoint | 2 full impls | **SEMI-GUARDED** — two independent test suites (`Networker.Endpoint.Tests`, `tests/endpoint.bats`), no shared golden; acceptable only because Rust side dies ~Jul 30 | `crates/networker-endpoint/src/*`, `src/Networker.Endpoint/*` |
| Mode/protocol lists | **6** | **UNGUARDED** (manual checklist in CLAUDE.md only) — `metrics.rs:442` enum, `dashboard/src/components/common/mode-family.ts:15-23`, `wizard/testbed-constants.ts:268-312`, `PlatformEndpoints.cs:69-73`, `docs/deploy-config.md:130,219` — the `endpoint.kind` bugs (#377-379) are this drift class already materialized | — |
| SQL schema | 2+ | **UNGUARDED — worst finding**: DDL source of truth is `crates/networker-dashboard/src/db/migrations.rs` (a crate scheduled for deletion); `Networker.Data/NetworkerDbContext.cs` is a DB-first scaffold with no EF migrations, no EnsureCreated. Post-decommission nobody owns DDL | + `bootstrap/reset-pre-prod.sql` |
| TS API types ↔ C# DTOs | 2 | **UNGUARDED** — hand-written `dashboard/src/api/*.ts` interfaces, zero OpenAPI/codegen in repo | — |
| Retired Rust crates ↔ C# | 2 | Retired crates still workspace members, clippy-clean-required in CI (`ci.yml:174`) but untested (`ci.yml:222-228`); partial mitigation: `RawWebSocketIntegrationTests.cs` pins the WS wire contract | `Cargo.toml:2-9` |

Drift consequence for the contract seam (per the project's own docs): a Rust-side rename passes both Rust tests and the stale C# sample — every timing deserializes to 0/null and the system "works while reporting garbage." Version drift feeds version-gated dispatch (`RunDispatcher.cs:644` `MinAssignRunVersionString`) with wrong self-reported versions — borderline correctness impact.

**Top fixes:** (1) extend `ci.yml` version-check to the C# copies (or single-source via `Directory.Build.props`); (2) CI-generated golden: run `networker-tester --json-stdout` in the dotnet workflow and deserialize through `ProbeContractJsonContext`; (3) move DDL ownership into `Networker.Data` before Jul 30; add a shared modes manifest consumed by TS + C# + docs-lint.

## 3. Module organization — B

Top-10 largest source files (excluding generated/vendored):

| File | Lines | Verdict |
|---|---|---|
| `crates/networker-tester/src/output/html.rs` | 6,826 | Cohesive but oversized — one concern, bundles SVG charts + comparison + observations + inline tests |
| `crates/networker-tester/src/runner/pageload.rs` | 3,817 | Cohesive (per-HTTP-version impls internally modular) |
| `benchmarks/orchestrator/src/reporter.rs` | 3,243 | God-file — 4 export formats mixed with statistics (bootstrap resampling, Tukey fencing, RNG) |
| `crates/networker-dashboard/src/api/testers.rs` | 2,903 | God-file (legacy) — CRUD + provisioning + audit + cloud + cost |
| `crates/networker-tester/src/output/db/postgres.rs` | 2,662 | Cohesive (embedded SQL) |
| `crates/networker-tester/src/metrics.rs` | 2,613 | Cohesive — the data model, well documented |
| `crates/networker-tester/src/cli.rs` | 2,484 | Cohesive — big by flag count |
| `crates/networker-dashboard/src/db/migrations.rs` | 2,475 | Cohesive (inline SQL migrations) |
| `crates/networker-tester/src/main.rs` | 2,460 | God-file — dispatch + output routing + benchmark phases + mode expansion |
| `benchmarks/orchestrator/src/executor.rs` | 2,361 | Multi-concern coordinator (provisioning + state machine + collection + DB) |

Per stack: **networker-tester** is best organized (`runner/` by protocol, `output/` by format, `output/db/` by backend). **ControlPlane** has good vertical slices (18 `Endpoints/` files + `Provisioning/`, `Background/`, `Dispatch/`, `Realtime/`, `Sso/`; no Program.cs monster) — but `TesterWriteEndpoints.cs` (1,734) reproduces the Rust `testers.rs` god-file blend: **the god-file was ported, not fixed**. **dashboard/src** is clean; only `JobDetailPage.tsx` (1,116) and `DiagnosticsPage.tsx` (1,042) exceed 1,000 lines. **benchmarks/orchestrator** has a reasonable flat layout but owns the repo's two clearest concern-mixers (`reporter.rs`, `executor.rs`) plus a coordinator-heavy `main.rs` (1,787).

**Top fixes:** (1) split `reporter.rs` statistics into a `stats` module; (2) break `TesterWriteEndpoints.cs` into create/lifecycle/audit slices; (3) convert `output/html.rs` into an `html/` submodule (charts, comparison, observations, tests).

## 4. Comment / doc-comment quality — B-

Ten files sampled across stacks. Pattern: Rust probe code is disciplined; C# Contracts frame Rust parity correctly; C# endpoint files carry unadapted Rust-era copy-paste; TS dashboard is nearly comment-free.

**Good (WHY-comments, verified):**
- `runner/dns.rs:17-22` — why the system resolver is used ("measures the path the user's applications actually take") + fallback policy + "Trust audit V1: resolver was previously hardcoded to 8.8.8.8". Rationale + history.
- `runner/http.rs:304-306` — "Disable Nagle's algorithm to prevent 40 ms delayed-ACK stalls during the HTTP/2 SETTINGS handshake."
- `runner/http.rs:404-406` — documents that client config is built BEFORE the handshake timer starts (what is deliberately excluded from measurement, trust audit V5).
- `Contracts/ProbeRunResult.cs:5-16` — explains selective field modelling, unknown-member tolerance, `schema_version` as growth signal — the right way to reference Rust origin.

**Bad / stale (verified):**
- `AccountEndpoints.cs:112-113` — `// TODO(email): the Rust dashboard emails {public_url}/reset-password?token={token} via crate::email.` — Rust module-path syntax in a C# file; target no longer exists in this stack.
- `AccountEndpoints.cs:49` — `// Rust db::users::change_password: only active/pending accounts.` — provenance, not policy; the WHY is still missing.
- `PerfLogEndpoints.cs:28` — doc-comment defers to "the Rust `ensure_schema` note" — that authority now lives only on the legacy branch.
- `App.tsx:17-24` — unexplained `lazyPage` adapter; `NotificationBell.tsx:17-19` — bare `catch { // ignore }` with no rationale.

**Top fixes:** (1) sweep `src/Networker.ControlPlane/Endpoints/` rewriting "Rust X does Y" comments into self-contained policy statements (grep `crate::`, `Rust `, `db::`); (2) JSDoc on exported dashboard components/hooks; annotate every empty `catch` with intent; (3) keep Rust-provenance references only where load-bearing (the `ProbeRunResult.cs` header style).

## 5. Error-handling consistency — B+

**Rust.** `ErrorCategory` (`metrics.rs:1367`, 8 variants) used at ~280 sites across every runner; classification is predominantly structural (Timeout assigned in `tokio::time::timeout` Err arms — `runner/tls.rs:133,235,511,571,606`; `pageload.rs:832,923,1030`). Exceptions: string-matching at `runner/http.rs:602` (`e.to_string().contains("timed out")`) and `runner/http3.rs:153-174` (`msg.contains("crypto")…`); `url_diagnostic.rs:1238` dumps to `Other`. `.context()` used ~150× in I/O/config paths (postgres.rs 54, mssql.rs 35, main.rs 19); runners intentionally return structured `ErrorRecord {category, message, detail}` instead — two idioms, each in its lane. unwrap/expect: 866 raw hits but overwhelmingly in `#[cfg(test)]`; non-test counts are low (pageload.rs 12, main.rs 4, http.rs/tls.rs 0). One real latent panic: `metrics.rs:1439` `partial_cmp().unwrap()` on RTT floats — NaN panics in a library path. `networker-endpoint/src/routes.rs:740-1073` has builder `.unwrap()`s in server handlers.

**C#.** No global exception middleware — zero hits for `UseExceptionHandler`/`AddProblemDetails`/`Results.Problem`/`TypedResults`; unhandled endpoint exceptions fall through to Kestrel's default 500 with no defined body, while 4xx is highly specified: a uniform homegrown `Results.*(new { error = "..." })` envelope (91 occurrences; sampled `TesterWriteEndpoints.cs:119,135,146,787,887`). 91 `catch (Exception)` — mostly disciplined (background services catch-log-continue with explanatory comments, e.g. `SchedulerService.cs:95-97`), but ~37 bare/silent catches, including a true swallow: `ProvisioningOrchestrator.cs:518` `catch (Exception) { return null; }` with no log. Agent uses `ILogger` uniformly (`AgentWorker.cs:73-76,206-209`); `Console.Error.WriteLine` only in pre-DI startup paths.

**Frontend.** Strong: shared `api/client.ts` with typed `ApiError {status, body}` (:17-27), centralized 401 → `clearSession()` + redirect (:47-59), per-request perf logging; 54 files use the shared clients, only 5 raw `fetch()` calls outside them.

**Top fixes:** (1) global exception handler emitting the existing `{ error }` envelope for 500s in `ControlPlane/Program.cs`; (2) typed error inspection instead of string matching (`e.is_timeout()` for reqwest at `http.rs:602`; quinn `ConnectionError` variants at `http3.rs:153-174`); (3) log before `return null` at `ProvisioningOrchestrator.cs:518`; `total_cmp` at `metrics.rs:1439`.

---

## Prioritized fix list

### P1 — drift with plausible correctness impact, or expiring windows

1. **Extend the CI version-check to the C# copies** (or single-source via `Directory.Build.props` + generated const). Drift is live today: `ServerInfo.cs` = 0.28.28, `Networker.Agent.csproj` = 0.28.26 vs 0.28.30 — and version-gated dispatch (`RunDispatcher.cs:644`) reasons over these self-reported values. Borderline P0. Also update CLAUDE.md's "3 locations" to the real 8.
2. **Move DDL ownership out of `crates/networker-dashboard/src/db/migrations.rs` into `Networker.Data`** (EF migrations or checked-in SQL) before the ~Jul 30 decommission — otherwise the schema's single source of truth gets deleted.
3. **Replace the hand-typed contract fixture (pinned at 0.28.13) with a CI-generated golden**: run `networker-tester --json-stdout` in the dotnet workflow and deserialize through `ProbeContractJsonContext`. This closes the "works while reporting garbage" failure mode the project's own docs describe.
4. **Add a global 500 handler in the ControlPlane** emitting the existing `{ error }` envelope — the 4xx contract is uniform, the 500 contract is undefined.

### P2 — structural debt a new engineer trips on

5. **Resolve the brand split**: naming map in README (Networker = engine, AletheDash = dashboard, AletheBench = benchmarks), rename `dashboard/package.json`, fix `alethabench` vs "AletheBench" spelling in `benchmarks/orchestrator/Cargo.toml`.
6. **Env var rationalization**: `CONTROLPLANE_*` (or `NETWORKER_*`) with deprecated `DASHBOARD_*` fallback, centralized in one config extension; collapse the three alias pairs (`DASHBOARD_DB_URL_NPGSQL`, `DASHBOARD_AZURE_SUBSCRIPTION`, `AGENT_DASHBOARDURL`); align Rust endpoint `BENCH_*` with C# `ENDPOINT_*` (or let it die with the crate).
7. **Shared modes manifest** consumed by TS + C# + a docs-lint step — the 6-way copy already produced the #377-379 bug class.
8. **Split the ported god-file** `TesterWriteEndpoints.cs` (1,734 lines) into create/lifecycle/audit slices; same treatment for `benchmarks/orchestrator/src/reporter.rs` (extract `stats`).
9. **Stale-comment sweep** in `src/Networker.ControlPlane/Endpoints/` (grep `crate::`, `Rust `, `db::`) — rewrite provenance comments as self-contained policy.

### P3 — polish

10. Split `output/html.rs` (6.8k lines) into an `html/` submodule.
11. Typed error classification for reqwest/quinn (`http.rs:602`, `http3.rs:153-174`); `total_cmp` at `metrics.rs:1439`; log the swallow at `ProvisioningOrchestrator.cs:518`.
12. JSDoc on exported dashboard components/hooks; annotate empty `catch` blocks.
13. TS API types from OpenAPI/NSwag codegen instead of hand-written `dashboard/src/api/*.ts` interfaces.
14. Drop retired Rust crates from the clippy-clean requirement (or from the workspace) so untested legacy code stops taxing CI.
