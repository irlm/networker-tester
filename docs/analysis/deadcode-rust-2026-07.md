# Dead-code survey — current Rust crates (2026-07)

**Scope:** `crates/networker-tester`, `crates/networker-endpoint`, `crates/networker-log` only.
The retired crates (dashboard/agent/common) are excluded; they are already deleted in the
held decommission draft (`7dea2b67`, PR #518). References *from* the retired crates were
checked at the pre-delete commit `19f7aded`.

**Baseline:** `cargo build --workspace 2>&1 | grep -i warn` → zero warnings. CI runs
`cargo clippy --all-targets -- -D warnings`, so everything below is code the compiler
*cannot* flag: unused `pub` API, never-enabled features, and test-only survivors.

**Method:** every `pub fn/struct/enum/const/static/type/trait` in the three crates was
extracted, then `grep -rw <name> --include='*.rs' crates/` was run repo-wide (target/
excluded) to count references outside the defining file, plus same-file references
(excluding the definition line) to separate "truly dead" from "over-visible", plus
`git grep -w <name> 19f7aded -- crates/networker-{dashboard,agent,common}` for the
decommission bucket. Confidence: HIGH = provably unreferenced; MEDIUM = referenced only
by tests or only by retired crates; LOW = possible serde/contract/reflection use.

Intentional stubs (`http3`/`pageload3` no-default-features paths), serde wire types that
are actually serialized, trait impls, and test helpers used by tests are **not** reported.

---

## 1. Feature-gated code never shipped

### 1.1 `native` feature — ~900 lines that no build ever enables — MEDIUM (policy)

- Definition: `crates/networker-tester/Cargo.toml` `native = ["dep:native-tls", "dep:tokio-native-tls"]`;
  implementation `crates/networker-tester/src/runner/native.rs` (989 lines total; the
  `#[cfg(feature = "native")]` real implementation spans ~l.85–989, ~900 lines).
- Not in `default = ["http3", "db-mssql", "browser"]`. Release builds
  (`.github/workflows/release.yml:91,197`) use `--features browser` on top of defaults —
  `native` is never enabled. CI enables it only via `--all-features` in the clippy/coverage
  and sdk-conformance jobs (`ci.yml:459`, `sdk-conformance.yml:122-126`), i.e. it is
  lint-checked and coverage-built but never tested by the integration matrix
  (`ci.yml:360,366` use `http3,db-mssql` / `http3`) and never shipped.
- Consequence: every shipped binary dispatches `--mode native`
  (`dispatch.rs:132 → run_native_probe`) into the stub, which always returns
  `ErrorCategory::Config: "native-tls probe requires '--features native' (recompile to enable)"`
  (`runner/native.rs:45-77`). Yet the mode is advertised in `shared/modes.json:15`
  (catalog: true) and `docs/deploy-config.md:251` as a supported probe mode.
- Evidence: `grep -rn '"native"' .github/workflows/` → no job enables it;
  `grep -n "native" release.yml` → only `build-native` (the OS matrix job name).
- Verdict: either promote to default features or delete the impl + manifest entry.
  As-is it is ~900 lines of shipped-never code plus a user-facing mode that always errors.

### 1.2 `db-postgres` tester sink — 2,662 lines, only CI tests build it — MEDIUM (policy, dead-after-decommission)

- Definition: `crates/networker-tester/src/output/db/postgres.rs` (2,662 lines), behind
  `db-postgres = ["dep:tokio-postgres"]` — not in default features.
- Before the decommission, the retired dashboard was the only in-repo consumer that turned
  it on (`19f7aded:crates/networker-dashboard/Cargo.toml`:
  `networker-tester = { ..., features = ["db-postgres"] }`). After #518, the feature is
  enabled solely by the CI SQL-test job (`ci.yml:565`,
  `--no-default-features --features db-postgres db_postgres_ -- --ignored`).
- Shipped binaries (defaults + browser) accept `--db-url postgres://…` in help text
  (`cli.rs:387`) but cannot use it.
- Verdict: post-#518 this is a self-licking ice-cream cone: CI tests code no product build
  contains. Decide: ship it (add to defaults like db-mssql) or remove sink + CI job.

`http3`/`pageload3` stubs: intentional per CLAUDE.md — not reported.
`browser` feature: enabled in release builds — alive. Endpoint `http3`: default — alive.

---

## 2. Truly dead `pub` items (HIGH — provably unreferenced)

| # | Item | Definition | Evidence (grep pattern, repo-wide `--include='*.rs'`, target/ excluded) | Lines |
|---|------|-----------|-------------------------------------------------------------------------|-------|
| 1 | `query::stats()` + `LogStats` + `ServiceStats` | `crates/networker-log/src/query.rs:217,73,63` | `grep -rw stats/LogStats/ServiceStats` → refs only inside `query.rs` itself; only external caller was retired dashboard `api/logs.rs` (`git grep 19f7aded` → `networker_log::query::stats`, `LogStats`); not even networker-log's own integration test calls it (`tests/integration.rs` uses only `query::list`) | ~80 |
| 2 | `UrlDiagnosticOrchestrator::mark_partial` | `crates/networker-tester/src/url_diagnostic.rs:232` | `grep -rw mark_partial` → zero references anywhere, including its own file | 6 |
| 3 | `default_methodology_json()` | `crates/networker-tester/src/cli.rs:1366-1386` | `grep -rw default_methodology_json` → zero references anywhere (not even a `#[serde(default = …)]` attribute) | ~21 |
| 4 | `BenchmarkComparison` (empty struct) | `crates/networker-tester/src/output/json.rs:236` (`pub struct BenchmarkComparison {}`) | only ref is the field decl `comparisons: Vec<BenchmarkComparison>` (json.rs:47); the vec is only ever set to `Vec::new()` (json.rs:550) — never constructed, always serializes as `[]`. Contract caveat: keep the `comparisons` field emitting `[]` if C# consumers parse it; the empty struct itself is removable | 2 (+ placeholder field) |
| 5 | `DbLayer::close()` | `crates/networker-log/src/db_layer.rs:60` | `grep -rn "\.close()" crates/networker-log` → zero callers; `LogGuard::shutdown` (builder.rs:81) re-implements the sender-take inline instead | ~7 |
| 6 | `SharedMetrics` type alias | `crates/networker-log/src/metrics.rs:98` | `grep -rw SharedMetrics` → only its own unit test (metrics.rs:230-234). Never used as an annotation anywhere | 2 |

Subtotal HIGH: **~118 lines**.

---

## 3. Test-only survivors (MEDIUM — referenced only from `#[cfg(test)]` / test crates)

| # | Item | Definition | Evidence | Lines |
|---|------|-----------|----------|-------|
| 1 | `ModeInfo` + `Protocol::all_modes()` + `fn m()` catalog | `crates/networker-tester/src/metrics.rs:544-778` (~235 lines) | doc comment says "used by the dashboard UI … single source of truth for the UI mode pickers"; dashboard consumed it (`19f7aded`: `networker_tester::metrics::Protocol::all_modes` in dashboard); today the only reference is the `#[cfg(test)]` drift guard `modes_manifest_guard.rs:130`. Caveat: guard check #3 ("all_modes equals manifest catalog") intentionally pins this against `shared/modes.json` — removing the catalog also removes that guard leg; the C#/frontend now read modes.json directly | ~235 |
| 2 | `execute_protocol_validation_probes()` + `make_protocol_probe()` | `crates/networker-tester/src/url_diagnostic.rs:294-350, 271-293` | only callers are the `#[cfg(test)]` module (l.1184, 1213; tests start l.1021). Live path `url_test_cli.rs:84` calls only `execute_primary_page_diagnostic`. **Side-effect note:** `UrlTestRun.validated_http_versions` / `protocol_runs` are therefore never populated in production runs (`add_protocol_run` live caller is only l.341, inside the test-only fn) — either a functional gap or confirmation the C# control plane now owns protocol validation | ~80 |
| 3 | `query::list()` + `LogQuery` + `LogRow` + `LogQueryResponse` | `crates/networker-log/src/query.rs:16-215` | only caller: `crates/networker-log/tests/integration.rs:136,147`; only production caller was retired dashboard `api/logs.rs` (git grep at 19f7aded). C#/TS `LogRow`/`ServiceStats` hits in `src/`/`dashboard/` are the independent C# port, not references | ~190 (query.rs is 272 lines total with §2.1) |
| 4 | `MetricsSnapshot::status()` + 5 threshold consts | `crates/networker-log/src/metrics.rs:5-10,66-96` | `grep -rn "\.status()" crates/networker-log` → only unit tests (l.138-196); production caller was the retired dashboard health endpoint | ~40 |
| 5 | `TestRun::protocols_tested()` | `crates/networker-tester/src/metrics.rs:232-244` | same-file refs only at l.2588/2633, inside `#[cfg(test)]` (starts l.1661) | ~14 |
| 6 | `Level::from_db()` / `Level::to_tracing()` | `crates/networker-log/src/types.rs:50,78` | refs only in own unit tests (l.171-181, 240-245); `query.rs` reads `level` as raw i16 (`row.get(2)`), never via `from_db` | ~20 |
| 7 | `LogGuard::metrics()` | `crates/networker-log/src/builder.rs:68` | only caller: `tests/integration.rs:111`; production caller was dashboard `main.rs:490` (`_log_guard.metrics().clone()` at 19f7aded) | 4 |
| 8 | `LogGuard::shutdown()` / `BatchHandle::shutdown()` | `crates/networker-log/src/builder.rs:81`, `batch.rs:56` | only caller: `tests/integration.rs:162`. Caveat: this is the graceful-flush API for the `--log-db-url` path the endpoint still exposes (`networker-endpoint/src/main.rs:113-119`) — arguably the *binaries* should call it, not delete it. Keep, but decide | (~30) not counted |

Subtotal MEDIUM (removable if the caveats are accepted, excluding 3.8): **~583 lines**.

---

## 4. Dead-after-decommission bucket (#518)

The working tree already reflects the deletion, so "referenced only by retired crates"
manifests here as "referenced by nothing / tests only". Verified at `19f7aded`, the retired
crates' entire surface into the three current crates was:

- `networker-log`: `LogBuilder::new`/`Stream::Stderr` (still alive — tester+endpoint use them),
  **`query::list`, `query::stats`, `LogQuery`, `LogQueryResponse`, `LogStats`,
  `LogPipelineMetrics` via `LogGuard::metrics()`, `Level`** → items 2.1, 3.3, 3.4, 3.6, 3.7 above.
- `networker-tester` (lib): `TestRun`, `RequestAttempt`, `Protocol`, `TlsEndpointProfile`
  (all still alive in tester itself), **`Protocol::all_modes`** (→ 3.1),
  `cli::PacketCaptureMode` (still alive in capture.rs), and the
  **`db-postgres` feature activation** (→ 1.2).
- `networker-endpoint`: no lib references from retired crates.

**Bucket size: ~3,290 lines** whose last production consumer died with the dashboard:
`output/db/postgres.rs` (2,662) + `networker-log/query.rs` (272) + ModeInfo catalog (~235)
+ `MetricsSnapshot::status` (~40) + `LogGuard::metrics` + `Level` helpers (~25) +
`query::stats` structs (counted in query.rs). All of it currently survives on CI-test /
unit-test life support.

---

## 5. Unused dependencies

Method: `grep -rE "(use |extern crate )<snake_name>|<snake_name>::" crates/<crate>/{src,tests}`.

**Result: none with zero hits.** Every `[dependencies]` entry in all three Cargo.tomls has
at least one real usage. Low-count entries verified by hand:

| Dep | Hits | Verified usage |
|-----|------|----------------|
| tester `serde_yaml` | 1 | `cli.rs:1269` config-file parsing — alive |
| tester `rust_xlsxwriter` | 1 | `output/excel.rs` — alive |
| tester `rustls-pemfile` | 1 | TLS cert loading — alive |
| tester `reqwest` | 4 | `baseline.rs:304`, `progress.rs:5`, `target_runner.rs:678` — alive (note: duplicates the hand-rolled hyper stack for non-timing paths; consolidation candidate, not dead) |
| endpoint `crc32fast`, `regex`, `http-body-util`, `futures` | 1 each | benchmark API routes — alive |

Confidence HIGH on the "none unused" verdict for direct `use`/path usage; macro-only usage
was covered by the `::` pattern.

---

## 6. Unreferenced files / assets / env vars / CLI flags

- **Files/assets:** the three crates contain only `.rs` + `Cargo.toml` (verified via
  `find crates/networker-{tester,endpoint,log} -type f -not -path '*/target/*'`). All
  `include_str!` targets (`shared/modes.json`, `docs/*.sql` schema files in
  mssql.rs:955-979 / postgres.rs:1658-1662) exist and are referenced. No orphans.
- **Env vars:** every `env::var` read in the three crates is acted upon
  (`BENCH_API_TOKEN`, `BENCH_DATA_PATH`, `NETWORKER_CHROME_PATH`, `NETWORKER_NO_SANDBOX`,
  proxy vars, `LOGS_DB_URL` [tests]). No dead reads found.
- **CLI flags:** no parsed-but-ignored flags found in the structures audited
  (`PacketCaptureConfig`/`ImpairmentConfig` are live at cli.rs:585/587). A field-by-field
  audit of all ~2,500 lines of cli.rs was not exhaustive.

---

## 7. Over-visible `pub` (not dead — should be `pub(crate)`/private; zero lines removable)

Referenced only from within their own file, live code (definition site → in-file use):
`benchmark.rs` consts + `BenchmarkPilotCriteria`/`BenchmarkAdaptiveStatus`/`MedianErrorBounds`
(8-47), `baseline.rs measure_rtt:61`, `runner/http3.rs build_quic_endpoint:120`,
`runner/browser.rs find_chrome:24` + the three `build_*_url` helpers,
`metrics.rs MIN_SAMPLES_P95/P99:1501-1503`, `cli.rs MAX_IMPAIRMENT_DELAY_MS:509`,
`HttpStack::from_name:693`, `capture.rs check_capture_prereqs:192`,
`summary.rs print_comparison:295`. Cosmetic; a `pub(crate)` sweep would let future
dead-code detection work via rustc instead of surveys like this one.

---

## 8. Ranked top candidates (by removable lines)

| Rank | Candidate | Lines | Confidence |
|------|-----------|-------|-----------|
| 1 | `output/db/postgres.rs` sink (db-postgres feature) — ship it or delete it | 2,662 | MEDIUM (policy) |
| 2 | `runner/native.rs` real impl (`native` feature) + modes.json/docs entry | ~900 | MEDIUM (policy) |
| 3 | `ModeInfo` catalog (`metrics.rs:544-778`) + guard leg | ~235 | MEDIUM |
| 4 | `networker-log/src/query.rs` (whole file + its integration test) | 272 | stats-half HIGH, list-half MEDIUM |
| 5 | `execute_protocol_validation_probes` + `make_protocol_probe` (url_diagnostic.rs) | ~80 | MEDIUM (flags a live functional gap) |
| 6 | `query::stats` + `LogStats` + `ServiceStats` (subset of #4) | ~80 | HIGH |
| 7 | `MetricsSnapshot::status()` + threshold consts | ~40 | MEDIUM |
| 8 | `default_methodology_json()` (cli.rs:1366) | ~21 | HIGH |
| 9 | `Level::from_db`/`to_tracing` | ~20 | MEDIUM |
| 10 | `mark_partial` + `DbLayer::close` + `SharedMetrics` + `BenchmarkComparison` | ~17 | HIGH |

**Totals:** HIGH-confidence removable now: **~118 lines**. MEDIUM (tests-only, accept
caveats): **~580 lines**. Policy decisions (features never shipped): **~3,560 lines**.
Dead-after-decommission bucket: **~3,290 lines** (overlaps the policy bucket).

## Recommended sequencing

1. After #518 merges: delete `networker-log/query.rs` + its test, `stats` structs,
   `MetricsSnapshot::status`, `LogGuard::metrics`, `Level::from_db/to_tracing`,
   `SharedMetrics`, `DbLayer::close` (≈ 400 lines, one PR, zero user impact).
2. Decide `native` and `db-postgres` feature fate (ship or delete) — these change the
   documented CLI surface, so they need a CHANGELOG entry and modes.json/docs sync.
3. Decide whether `execute_protocol_validation_probes` should be *wired in* (populating
   `validated_http_versions` in live url-test runs) rather than deleted.
4. Optional `pub(crate)` sweep (§7) so rustc's dead_code lint regains visibility.
