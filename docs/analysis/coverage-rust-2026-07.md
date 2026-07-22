# Rust Test-Coverage Survey — 2026-07 (READ-ONLY)

Scope: current (non-retired) crates only — `networker-tester`, `networker-endpoint`,
`networker-log`. Retired crates (`networker-dashboard`, `networker-agent`,
`networker-common`) excluded per CLAUDE.md.

This is a **risk-weighted** map, not a line-% vanity report. The question for
each module is not "is it touched" but "would these tests catch a bug that
corrupts a measurement, mis-classifies an error, or breaks the JSON contract".

## Real coverage numbers — obtained

`cargo llvm-cov --lib -p networker-tester --summary-only` ran successfully
(719 lib tests pass, 18 ignored, ~5s). **TOTAL: 47.6% lines, 48.2% regions,
50.9% functions.**

Important scoping caveats on these numbers:
- `--lib` only. It does NOT run `tests/integration.rs` (32 tests),
  `tests/json_contract.rs`, or `tests/html_snapshot.rs`. Several modules that
  read 0%/low here (`native.rs`, `curl.rs`, `http.rs` error paths, `udp*`,
  `pageload`) ARE exercised by the integration suite — the number understates
  their true coverage.
- Default features are `["http3", "db-mssql", "browser"]`. `native` and
  `db-postgres` are **not** default, so `native.rs` (0%) and
  `output/db/postgres.rs` (0%) never compiled into this run — their 0% is a
  measurement artifact, not necessarily an absence of tests (native has 6
  unit + 1 integration test behind `--features native`; postgres has 32
  `#[ignore]` tests needing Docker).

Per-file (from llvm-cov, lines %):

| Module | Lines % | Verdict |
|---|---|---|
| modes_manifest_guard.rs | 99.3% | strong (drift guard) |
| output/db/test_fixtures.rs | 100% | test support |
| output/db/mod.rs | 98.9% | strong |
| output/html/render_multi.rs | 98.1% | strong |
| output/excel.rs | 96.9% | strong |
| output/html/tables.rs | 96.0% | strong |
| output/json.rs | 94.4% | strong (contract seam) |
| output/html/mod.rs | 89.9% | strong |
| output/html/protocol_sections.rs | 89.7% | strong |
| url_diagnostic.rs | 76.1% | good (SSRF guards tested) |
| runner/socket_info.rs | 75.0% | good |
| output/html/run_sections.rs | 75.3% | ok |
| runner/udp.rs | 70.6% | ok |
| metrics.rs | 69.8% | **partial — see gaps** |
| capture.rs | 68.6% | ok |
| cli.rs | 67.5% | ok (huge surface) |
| runner/dns.rs | 64.1% | ok |
| runner/http3.rs | 59.1% | partial (classifiers tested; probe path thin) |
| runner/browser.rs | 57.2% | partial (stub-heavy) |
| output/html/charts.rs | 54.8% | weak |
| runner/throughput.rs | 52.1% | **misleading — math is exemplary; see below** |
| runner/udp_throughput.rs | 51.6% | partial |
| runner/pageload.rs | 51.3% | partial (largest file, 3.8k LOC) |
| tls_profile.rs | 43.9% | weak |
| runner/http.rs | **30.0%** | **weak — core probe path** |
| runner/curl.rs | 29.0% | integration-covered under feature |
| runner/tls.rs | **21.0%** | **weak — core probe path** |
| output/db/mssql.rs | 2.2% | ignored (needs Docker) |
| **baseline.rs** | **0%** | ← lib-untested, but 15 integration tests exist |
| **benchmark.rs** | **0%** | **← genuine gap** |
| **dispatch.rs** | **0%** | **← genuine gap (the routing hub)** |
| **summary.rs** | **0%** | **← genuine gap (pass/fail + aggregates)** |
| output/html.rs (top orchestrator) | 0% | integration/snapshot-covered |
| runner/native.rs | 0% | feature-gated out of this run |
| output/db/postgres.rs | 0% | feature-gated + `#[ignore]` |
| output/sql.rs, progress.rs | 0% | trivial |

`networker-endpoint`: `routes.rs` 30 tests, `lib.rs` 20 tests,
`tests/canonical_checksums.rs` (2). `udp_throughput.rs`, `http3_server.rs`,
`main.rs` have **no test module**. `networker-log`: `types.rs` (11),
`metrics.rs` (13), `tests/integration.rs` (2); `batch/builder/db_layer/query/
schema` untested. (Endpoint/log coverage not separately quantified — structural.)

## What is genuinely GOOD (so we don't retest it)

- **`runner/throughput.rs` — the gold standard.** 57 tests nail the Mbps math
  (`mbps()` 1 MiB/s, 10 MiB/s, 1 GiB/1024ms, zero/negative/single-byte),
  window selection (excludes TTFB, excludes HTTP handshake, uses body-receive
  time), integrity verification (`verify_upload`/`verify_download` byte-match
  incl. truncated body, case-insensitive header, non-numeric/float/empty
  header). These would catch a wrong-throughput bug. The 52% line number is
  misleading — the async probe glue is uncovered, but the value-bearing math is
  thoroughly covered.
- **`tests/json_contract.rs`** — meaningful: freezes `schema_version == "1.0"`,
  proves additive fields (`dns.resolver`, `http.http_handshake_ms`) round-trip
  and back-compat-deserialize when absent. This is the C# seam guard.
- **`tests/integration.rs` error taxonomy** — real assertions on ErrorCategory:
  `connection_refused_classified_as_tcp`, `request_timeout_classified_as_timeout`,
  `tls_to_plain_http_port_classified_as_tls`,
  `tls_cert_verification_failure_classified_as_tls`,
  `http_500_is_failure_with_http_category`, `http_4xx_is_failure_across_modes`,
  `http_3xx_counts_as_success`. These exercise the http.rs classification branches
  the `--lib` number misses.
- **`url_diagnostic.rs`** SSRF guards (rejects loopback/metadata/bad-scheme,
  redacts credentials) — meaningful security tests.
- **`baseline.rs` classify_ip/classify_target** — 15 tests in `main_tests.rs`
  (loopback v4/v6, RFC1918, CGNAT, link-local, ULA, public). 0% lib line is
  because they live in `main_tests.rs` which links the binary, not the lib.

## TOP RISK-RANKED UNDER-TESTED PATHS

Ranked by blast radius: a bug here corrupts a measurement, mis-classifies an
error, silently breaks the contract, or crashes a probe.

### 1. `summary.rs` — 0% coverage, no test module. (507 LOC, 67 fns)
`summary.rs::print_summary` and its aggregate helpers compute the pass/fail
verdict, percentiles, and per-protocol rollups the user actually reads.
**Why it matters:** a wrong p50/p95/mean or a mis-counted success rate makes
the whole product lie — silently. There is nothing asserting `min/mean/p95`
ordering or that a run with N failures reports N failures.
**Test idea:** feed a fixed `TestRun` with known attempt latencies and assert
exact computed percentiles + success/failure counts + per-protocol grouping.

### 2. `dispatch.rs::dispatch_once` / `log_attempt` — 0% coverage. (481 LOC)
The central `(Protocol, payload) → runner` router (dispatch.rs:65-152). A wrong
arm sends the wrong probe or drops a variant.
**Why it matters:** adding/renaming a Protocol variant can mis-route with zero
compile error and zero test failure; `modes.json` guard catches the manifest,
not the dispatch arm.
**Test idea:** table test asserting each `Protocol` variant routes to a runner
that returns an attempt with the matching `protocol` field (mock/stub runners),
and that download/upload variants without a payload size are rejected, not
silently mapped.

### 3. `runner/tls.rs` — 21% lines, and TLS-resume is untested. (1239 LOC)
Handshake-timing capture, `resumed`/`handshake_kind` labeling, and
`previous_handshake_*` fields are the whole point of the `TlsResume` protocol,
yet `json_contract.rs` hardcodes `resumed: Some(false)` and no test proves a
resumed handshake sets `resumed:true`/`handshake_kind:"resumed"` or that
`tls13_tickets_received` is populated. Cert-parsing helpers ARE tested; the
timing/resume path is not.
**Why it matters:** TLS-resume is a headline metric; a bug would report every
handshake as "full" and the resume feature would be invisibly dead.
**Test idea:** integration test doing two sequential TLS probes with ticket
reuse against the endpoint; assert second attempt has `resumed==Some(true)`,
`handshake_kind=="resumed"`, and `previous_handshake_duration_ms.is_some()`.

### 4. `runner/http.rs` per-phase error classification — 30% lines. (1963 LOC)
The connect/TLS/timeout → ErrorCategory mapping (http.rs:292,306,428,447,474,488,
638) is the product's core probe. Helper-level tests (proxy, cert, request
build, server-timing parse) are excellent, but the classification branches are
only reached via the integration suite (4-5 cases). Branches like
Config-vs-Tcp-vs-Tls-vs-Http on partial failures are thin.
**Why it matters:** mis-labeling a TLS failure as Tcp (or Http-500 as success)
corrupts the error taxonomy the dashboard sorts/filters on.
**Test idea:** unit-test the classification helper directly (extract if needed)
with synthetic errors for each phase; assert exact ErrorCategory + that
partial phases (dns ok, tcp fail) preserve the dns timing.

### 5. `benchmark.rs` — 0% coverage, no test module. (408 LOC)
Warmup/pilot/overhead/cooldown attempt accounting and the execution-plan /
noise-threshold fields feed the benchmark verdict.
**Why it matters:** an off-by-one in warmup exclusion or a mis-set phase label
poisons benchmark numbers (which drive the "perf-per-cost" product direction).
**Test idea:** given a fixed plan, assert the correct counts land in
`benchmark_warmup/pilot/overhead/cooldown_attempt_count` and that warmup
attempts are excluded from the reported aggregate.

### 6. `metrics.rs::primary_metric_value` / `primary_metric_label` — partial. (2658 LOC, 69.8%)
48 tests exist, but they cover `ErrorCategory::Display` and enum round-trips.
The `primary_metric_value(a)` selector (metrics.rs:1617) — which picks WHICH
number represents each protocol on the dashboard — has thin coverage of the
per-protocol arms.
**Why it matters:** if `primary_metric_value` returns ttfb where it should
return throughput (or vice-versa) for a variant, the headline number for that
protocol is wrong everywhere downstream.
**Test idea:** table test: for each `Protocol`, build an attempt and assert
`primary_metric_label` + `primary_metric_value` return the intended field.

### 7. `runner/udp_throughput.rs` — 51% lines. (834 LOC)
Loss accounting and the transfer-window (excludes report-wait) have 2 good
integration tests (`udpupload_missing_report_is_an_error_not_zero_loss`,
`udpupload_transfer_window_excludes_report_wait`), but reorder/dup-packet and
partial-loss Mbps math on the download side are thin vs the TCP throughput
module's rigor.
**Why it matters:** UDP loss% and goodput are the metric; treating a missing
report as 0% loss (the exact bug one test guards) elsewhere would understate loss.
**Test idea:** mirror the throughput.rs mbps edge-case suite for UDP: reordered
echo, duplicate packet, out-of-window bytes, zero-received.

### 8. `runner/pageload.rs` — 51% lines, largest module. (3818 LOC)
Waterfall/asset-timing aggregation across h1/h2/h3. Has connection-refused
tests per stack, but the multi-asset timing rollup (which asset counts toward
which phase) is under-asserted for a 3.8k-LOC file.
**Why it matters:** page-load is a headline test type; a wrong asset-to-phase
attribution mis-reports where the time went.
**Test idea:** synthetic multi-asset fixture; assert per-origin/connection
grouping and that total = sum of critical-path phases.

### 9. `tls_profile.rs` — 44% lines. (1926 LOC)
30 tests but large uncovered surface in profile construction / ALPN / cipher
selection paths.
**Why it matters:** wrong profile → probing a different TLS config than the
user asked for; measurements attributed to the wrong handshake shape.
**Test idea:** assert each named profile yields the expected min/max version +
ALPN + cipher list.

### 10. `networker-endpoint` UDP-throughput + HTTP/3 server — no test module.
`udp_throughput.rs`, `http3_server.rs`, `main.rs` have no in-file tests. These
are the measurement TARGET; if the server's byte accounting or checksum is
wrong, every client-side throughput number is measured against a broken oracle.
`canonical_checksums.rs` guards the download payload checksum (good) but not the
UDP throughput server's report accounting.
**Why it matters:** a wrong server-side received-bytes report makes the client's
goodput calc wrong even though the client math is correct.
**Test idea:** in-process UDP throughput server test asserting the server's
received-byte report matches bytes actually sent under induced loss.

### 11. `runner/http3.rs` real probe path — 59% lines.
Error classifiers (`classify_quic_connection_error`,
`classify_endpoint_build_error`) ARE unit-tested (good). The real QUIC probe
timing path is only smoke-covered (connection-refused). HTTP/3 stub-vs-real:
the `--no-default-features` stub is CI-build-verified but its returned-attempt
shape isn't asserted equal to the real module's.
**Test idea:** assert the stub `run_h3_probe` returns an attempt with
`protocol==Http3` and a graceful Config/Other error (contract parity with real).

### 12. `output/db/postgres.rs` — 0% in this run (feature-gated + `#[ignore]`).
32 tests exist but require Docker and `--features db-postgres`. In a default CI
lib run, the Postgres persistence path — which the C# control plane now owns the
schema for — is entirely unexercised. Same for `mssql.rs` (2.2%).
**Why it matters:** a serialization/column-mapping regression in DB persistence
ships green. This is a CI-configuration gap as much as a test gap.
**Test idea:** ensure the SQL integration tests run in a gated CI lane (they
exist; confirm they actually execute), and add a non-Docker unit test for the
row-mapping / SQL-param-binding pure functions.

## Weak / misleading existing tests (would pass even if the metric were wrong)

- `json_contract.rs::json_output_carries_all_phase_timings` asserts only
  `> 0.0` / `total >= ttfb` on **hardcoded** sample values — it proves the
  fields serialize, NOT that any real probe computes them correctly. It cannot
  catch a wrong duration.
- TLS-resume: `sample_run()` hardcodes `resumed: Some(false)` — no test ever
  sees `resumed: true`, so the resume-detection code is contractually untested.
- `runner/browser.rs` `stub_or_real_returns_browser_protocol` and similar
  stub tests assert protocol tag only, not any captured timing/asset value.
- `native.rs` has 6 unit tests (backend name, make_failed field-setting) but
  they never call `run_native_probe`; combined with the feature-gate, the
  actual native-TLS timing path is only touched by one integration test.
- `dns.rs` at 64% — resolver-identity and multi-A-record handling are lighter
  than the SSRF-adjacent paths.

## ADD THESE TESTS FIRST (prioritized)

1. **`summary.rs` aggregate correctness** — fixed TestRun → exact p50/p95/mean +
   success/failure counts + per-protocol grouping. (highest value; 0% today)
2. **`dispatch.rs` routing table test** — every Protocol variant routes to a
   runner returning the matching `protocol`; payload-required variants reject
   missing size. (0% today, silent mis-route risk)
3. **TLS-resume behavior test (integration)** — two sequential probes, assert
   `resumed==Some(true)` + `handshake_kind=="resumed"` + `previous_*` set.
   (headline metric, contractually untested)
4. **`http.rs` phase-classification unit test** — synthetic per-phase errors →
   exact ErrorCategory, partial phases preserve earlier timings. (core taxonomy)
5. **`metrics.rs::primary_metric_value/label` table test** — per-protocol
   headline-number selector. (wrong-number-everywhere risk)
6. **`benchmark.rs` phase-accounting test** — warmup excluded from aggregate;
   correct per-phase attempt counts.
7. **UDP throughput loss/goodput edge cases** — reorder, dup, partial loss,
   out-of-window bytes (mirror the throughput.rs suite).
8. **endpoint UDP-throughput server accounting** — server received-byte report
   correct under induced loss (protects the measurement oracle).
9. **Confirm Postgres/MSSQL SQL integration lane actually runs in CI** and add
   pure row-mapping unit tests that don't need Docker.

## Bottom line

The **JSON contract**, **TCP throughput math**, **SSRF/url-diagnostic guards**,
and **HTML/Excel reporters** are genuinely well-tested. The gaps cluster in the
**aggregation + routing layer** (`summary.rs`, `dispatch.rs`, `benchmark.rs` all
0%) and the **live TLS/HTTP probe timing/classification path** (tls.rs 21%,
http.rs 30% — helpers tested, phase logic reached only by the integration
suite). TLS-resume and per-protocol headline-metric selection are the two spots
where a real bug would silently ship wrong numbers with all tests green.
