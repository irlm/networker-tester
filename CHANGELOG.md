# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.12.9] – 2026-03-02 — Coverage phase 4: http.rs + udp.rs unit tests + integration flakiness fix

### Added
- **30 new unit tests in `runner/http.rs`**: `is_no_proxy` edge cases (empty string,
  case-insensitive, whitespace trimming, empty entries, non-suffix match); `parse_server_timing_header`
  edge cases (unknown name ignored, invalid `dur=` ignored, `dur=` among multiple attrs);
  `parse_cert_fields` (invalid DER returns None, empty bytes, valid cert subject/issuer/expiry via
  rcgen, expiry is in future); `pick_ip` fallback cases (no IPv4 available → first, `prefer_v4=false`
  → first regardless); `failed_attempt` constructor (all fields verified); `parse_server_timing`
  with HeaderMap (no headers → None, server-timing only, `x-networker-server-version`, request-id);
  `build_request` method/header selection (GET for empty payload, POST with content-length and
  upload-bytes headers, host + request-id headers set correctly).
- **3 new unit tests in `runner/udp.rs`**: `udp_failed` constructor correctness; loopback echo
  server with `probe_rtts_ms` all-Some verification; existing test reuse without duplication.

### Fixed
- **Integration test flakiness** (`free_udp_port()`): UDP server binds `0.0.0.0:{port}` but the
  previous `free_port()` used a TCP listener (different port namespace). New `free_udp_port()` now
  binds `0.0.0.0:0` — the same address family as the server — so the port is guaranteed free for a
  subsequent `0.0.0.0:{port}` bind, eliminating "UDP throughput server did not bind within 10s" panics.

### Coverage
- `runner/http.rs`: 68% → 78.22% lines, 75% → 86.67% functions
- `runner/udp.rs`: 60% → 84.68% lines, 78% → 88.89% functions
- Total: 67.46% → 70.28% lines, 74.92% → 74.48% functions (line improvement of +2.8 pp)

---

## [0.12.8] – 2026-03-02 — Upload size verification via response header

### Added
- **`X-Networker-Upload-Bytes: N`** request header: the client now declares the
  intended upload size on every POST so the server knows what to expect.
- **`X-Networker-Received-Bytes: N`** response header: the endpoint echoes the
  actual number of bytes drained from the request body, enabling end-to-end
  verification without parsing the JSON body.
- `verify_upload()` in `throughput.rs`: after every `upload` / `webupload` probe,
  the received byte count is compared against the declared payload size. A mismatch
  marks the attempt `success = false` with a clear `ErrorCategory::Http` message
  ("sent N bytes but server received M bytes"). Absent header (third-party or older
  endpoint) is silently skipped — no false failures.
- 6 new unit tests covering the verification logic: match passes, mismatch fails with
  descriptive message, absent header is skipped, already-failed attempt is not
  overwritten, header name matching is case-insensitive.
- 1 new endpoint unit test: `upload_returns_received_bytes_header`.

---

## [0.12.7] – 2026-03-02 — Streaming upload body; large-payload timeout scaling

### Fixed
- **Upload probes** (`upload`, `webupload`) no longer allocate the entire upload payload in
  RAM at once. The body is now streamed in 256 KiB chunks from a static zero buffer, so a
  5 GiB upload uses ~256 KiB of memory instead of ~5 GB.
- **Timeout auto-scaling**: both upload probes extend the request timeout if the payload
  cannot complete at an assumed minimum speed of ~100 MB/s within the user-specified timeout.
  A 5 GiB upload now gets ~60 s instead of the default 30 s, preventing spurious timeouts
  on large but healthy uploads. Users on slower links can still set `--timeout` explicitly.
- `content-length` header is now set on streaming upload requests so HTTP/1.1 uses
  fixed-length framing rather than chunked transfer encoding.

### Added
- 9 new unit tests: `upload_body_exact_byte_count`, `upload_body_all_zeros`,
  `upload_body_small_payload`, `upload_body_zero_bytes_yields_nothing` (streaming body
  correctness); `timeout_unchanged_for_small_payloads`, `timeout_unchanged_for_one_gib`,
  `timeout_extended_for_five_gib`, `timeout_never_below_base`,
  `timeout_zero_payload_returns_overhead_only` (timeout scaling logic).

---

## [0.12.6] – 2026-03-02 — Integration tests for pageload H1/H2/H3

### Added
- Integration tests for `run_pageload_probe` (HTTP/1.1), `run_pageload2_probe` (HTTP/2),
  and `run_pageload3_probe` (HTTP/3 over QUIC) — all using the in-process endpoint on random
  ports, no external dependencies required
- `Endpoint::wait_for_quic()` helper in the integration test fixture: waits 300 ms after
  TCP-HTTPS readiness to let the QUIC server bind its UDP port
- `pageload.rs` line coverage: 11% → 74%; function coverage: 28% → 84%
- Overall (lib + integration): lines 47.9% → 57.4%, functions 54% → 65.8%

---

## [0.12.5] – 2026-03-02 — Test coverage phase 2

### Added
- **`runner/curl.rs`** — unit tests for `parse_write_out`, `secs_to_ms`, and
  `error_category_for_exit` (all pure-logic, no live curl needed): 11 new tests
- **`metrics.rs`** — tests for `primary_metric_label` (Dns / Tls / Browser),
  `primary_metric_value` (with and without sub-results), `attempt_payload_bytes`
  (http, udp_throughput, zero-payload filter), `TestRun::protocols_tested`
  (deduplication), and `RequestAttempt::total_duration_ms`: 18 new tests
- **`runner/pageload.rs`** — tests for all 6 named presets (tiny / small / medium /
  large / default / mixed), mixed-asset composition, case-insensitive matching,
  and unknown-preset error messaging: 8 new tests
- **`cli.rs`** — tests for empty-modes validation, verbose flag → log-level debug,
  log-level overriding verbose, `parsed_modes` filtering invalid strings, `--page-preset`
  tiny resolution, invalid preset fallback, `parse_size` gigabyte suffix, and
  `load_config` error paths: 8 new tests
- **`output/html.rs`** — tests for CSS `<link>` injection, error section, Throughput
  Results section (Download attempt), TLS Details section, Page Load section, Browser
  Results section, `escape_html` quotes/ampersand: 8 new tests
- **`output/excel.rs`** — test exercising the Throughput sheet (Download + Upload
  attempts) that was previously uncovered: 1 new test
- **`runner/browser.rs`** — tests for `find_chrome` with non-existent env-var path
  (skip), existing tempfile path (return), and `build_page_url` scheme/host preservation:
  3 new tests

---

## [0.12.4] – 2026-03-02 — README: fix Windows installer examples

### Fixed
- **README.md** — Windows PowerShell section now shows self-contained one-liners for both
  `tester` and `endpoint`: the endpoint example previously assumed the script was already
  downloaded locally (`.\install.ps1 -Component endpoint`); replaced with an
  `Invoke-WebRequest` + `&` pattern that downloads from the Gist URL before running.
  Added PS 5.1 / PS 7+ compatibility note.

---

## [0.12.3] – 2026-03-02 — Fix install.ps1 NativeCommandError on SSH probe

### Fixed
- **`install.ps1`** — `ssh -T git@github.com` always exits with code 1 (GitHub design).
  With `$ErrorActionPreference = "Stop"` set globally, PowerShell 5.1 throws
  `NativeCommandError` before the authentication string can be checked.
  Fixed by temporarily lowering preference to `"Continue"` around the SSH call and
  restoring it immediately after.

---

## [0.12.2] – 2026-03-02 — Fix Gist sync to include install.ps1

### Fixed
- **`sync-gist.yml`** — workflow only watched `install.sh` changes and only uploaded `install.sh`
  to the Gist; `install.ps1` was never synced even when it changed. Updated path trigger and
  payload to include both `install.sh` and `install.ps1`.

---

## [0.12.1] – 2026-03-01 — Fix install.ps1 compatibility with Windows PowerShell 5.1

### Fixed
- **`install.ps1`** — replaced `?.Source` (null-conditional member access, requires PS 7.1+)
  with a PS 5.1-compatible `if ($cmd) { $cmd.Source }` pattern. The script declared
  `#Requires -Version 5.1` but used syntax only available in PowerShell 7.1+, causing a
  `ParseException: UnexpectedToken` when run via `irm … | iex` on Windows with the default
  Windows PowerShell 5.1.

---

## [0.12.0] – 2026-03-01 — Real-browser probe (`browser` mode via chromiumoxide)

### Added
- **`browser` probe mode** (`--features browser`) — drives a real headless Chromium instance
  via the Chrome DevTools Protocol (chromiumoxide 0.7) to measure actual page-load performance
  that no synthetic probe can replicate.
- **Metrics captured**: load time (navigation start → load event), DOMContentLoaded (ms),
  TTFB (ms), total resource count, total transferred bytes, negotiated protocol for the main
  document, and per-protocol resource counts (e.g. `h2×18 h3×2`).
- URL is rewritten to `/page` endpoint for a fair comparison with `pageload`/`pageload2`/
  `pageload3` probes.
- Self-skips with a `success: false` `RequestAttempt` if no Chrome/Chromium binary is found;
  binary search order: `NETWORKER_CHROME_PATH` env var → common Linux paths → macOS app bundles.
- `--features browser` is opt-in (not part of `default`); the stub build always compiles and
  returns a clear error message.
- HTML report **"Browser Results"** section with a per-attempt table (Protocol, TTFB, DCL,
  Load, Resources, Bytes, per-protocol counts).
- Terminal summary: `Protocol Comparison` table now includes a `browser` row.
- New `BrowserResult` struct in `metrics.rs`; `Protocol::Browser` variant in the `Protocol`
  enum; `RequestAttempt.browser: Option<BrowserResult>` field (backwards-compatible via
  `#[serde(default, skip_serializing_if = "Option::is_none")]`).

---

## [0.11.7] – 2026-03-01 — Fix ServerTimingResult schema (type mismatch in FK)

### Fixed
- **`sql/06_ServerTiming.sql`** — `ServerId` and `AttemptId` columns were declared
  as `UNIQUEIDENTIFIER`, but `RequestAttempt.AttemptId` (the FK target) is
  `NVARCHAR(36)`. SQL Server rejected the `CREATE TABLE` silently (sqlcmd
  doesn't abort on DDL errors), leaving `dbo.ServerTimingResult` absent from the
  database. Both columns are now `NVARCHAR(36)` to match the schema convention
  used by all other tables and the Rust insert code (`uuid.to_string()`).
  The `DEFAULT NEWSEQUENTIALID()` on `ServerId` is also removed — the insert
  always provides an explicit UUID value.

---

## [0.11.6] – 2026-03-01 — Improve sql.rs coverage

### Added
- **`sql_full_round_trip` test** — exercises all 7 sub-result insert helpers
  (`insert_dns_result`, `insert_tcp_result`, `insert_tls_result`,
  `insert_http_result`, `insert_udp_result`, `insert_error`,
  `insert_server_timing_result`) by inserting a fully-populated `RequestAttempt`
  with every sub-result field set; expected to push `sql.rs` line coverage from
  ~39% to ~85%+.

### Changed
- `sql_insert_round_trip` refactored to use shared `make_run` / `bare_attempt` /
  `sql_conn` helpers so the new comprehensive test reuses the same scaffolding.

---

## [0.11.5] – 2026-03-01 — Fix 6 test failures with --include-ignored in coverage CI

### Fixed
- **`validate_save_to_sql_without_conn_string_fails`** — test now self-skips when
  `NETWORKER_SQL_CONN` is set in the environment; clap picks up the env var automatically,
  so validation correctly passes in that environment (breaking the assertion).
- **`sql_insert_round_trip`** — coverage CI schema creation now runs all 7 SQL migration
  files (01–02 base + 04–07 migrations); previously missing migrations left `RequestAttempt`
  without the `RetryCount` column, causing the INSERT to panic.
- **`http1_probe_succeeds` / `http2_probe_negotiates_h2`** — tests now probe
  `127.0.0.1:8080` / `127.0.0.1:8443` with a TCP connect before running; they self-skip
  with an `eprintln!` message when no endpoint is listening, avoiding false failures in CI.
- **`download_probe_returns_throughput` / `upload_probe_returns_throughput`** — same
  self-skip pattern: TCP connect to `127.0.0.1:8080` before running the probe.
- **`sql-integration` CI job** — also updated to run all 7 SQL migration files (same fix
  as coverage job).

---

## [0.11.4] – 2026-03-01 — Coverage phase 1: SQL Docker, DNS tests, Excel tests

### Added
- **SQL Server in coverage CI** — MSSQL 2022 Docker service added to the coverage
  job; schema migrations run automatically; `NETWORKER_SQL_CONN` env var set so the
  existing `#[ignore]` SQL round-trip test executes under `cargo-llvm-cov`.
- **Excel unit tests** — `save_writes_xlsx_file` exercises all 10 worksheet writers
  with a fully-populated `TestRun` (HTTP, TCP, TLS, UDP, throughput, UDP throughput,
  server timing, page-load, errors); `save_empty_run_does_not_panic` covers the
  zero-attempt edge case.
- **`main.rs` excluded from coverage** — `--ignore-filename-regex 'main\.rs'` added
  to all `cargo llvm-cov report` invocations so the binary entry point does not
  drag down the overall percentage.

### Changed
- DNS tests `resolves_localhost` and `ipv4_only_filter` are no longer `#[ignore]` —
  loopback resolution works in all environments including CI.
- Coverage lib-test step now passes `--include-ignored` to pick up `#[ignore]` tests
  that self-skip when their service is unavailable (SQL, curl).

---

## [0.11.3] – 2026-03-01 — Coverage report as GitHub Actions artifact

### Changed
- CI coverage job now uploads an HTML report + `lcov.info` as a downloadable
  GitHub Actions artifact (`coverage-report`, 30-day retention) instead of
  pushing to Codecov. The coverage summary is also printed directly in the
  CI log (`cargo llvm-cov report --summary-only`).

---

## [0.11.2] – 2026-03-01 — Fix --all-features compile error in native probe

### Fixed
- `runner/native.rs`: second `HttpResult` literal was missing `goodput_mbps`,
  `cpu_time_ms`, `csw_voluntary`, and `csw_involuntary` — fields added in v0.11.0.
  Only exposed by `--all-features` builds (e.g. `cargo-llvm-cov`); the default CI
  build does not enable `native` and did not catch this. (#58)

---

## [0.11.1] – 2026-03-01 — HTTP/3 QUIC endpoint; --insecure for http3 probe

### Added
- **HTTP/3 QUIC server in `networker-endpoint`** — Quinn-based QUIC listener on UDP
  8443 (same port as HTTPS), serving `/health`, `/download`, `/upload`, `/page`, `/asset`
  with full `Server-Timing` (proc/recv/csw-v/csw-i) and `X-Networker-*` headers.
  `http3` is now a default feature of both crates; no extra flags needed.
- **`--insecure` and `--ca-bundle` for `http3` probe** — previously the h3 client always
  used webpki roots and ignored these flags. Now uses the same `build_tls_config()` path
  as HTTP/1.1 and HTTP/2, so `--insecure` works with the self-signed endpoint cert.

### Changed
- `networker-endpoint/Cargo.toml`: `[features] default = ["http3"]`
- `/info` endpoint: `"http3": true` and `"protocols": ["HTTP/1.1","HTTP/2","HTTP/3"]`
  when compiled with the http3 feature (now the default).

---

## [0.11.0] – 2026-02-28 — CPU cost, goodput, context switches & TTFB visibility

### Added
- **CPU time on all HTTP probes** (`http1`, `http2`, `http3`) — `HttpResult.cpu_time_ms`
  captures process CPU (user + system) consumed per probe using `cpu-time::ProcessTime`.
  Enables a fair H1 vs H2 vs H3 comparison; QUIC/HTTP3 is expected to show the highest
  CPU cost due to in-process TLS encryption.
- **Goodput metric** — `HttpResult.goodput_mbps` = payload_bytes / full end-to-end delivery
  time (DNS + TCP + TLS + total HTTP ms). Penalises connection-setup overhead, giving a
  more complete picture than throughput alone (which only measures the body-transfer phase).
  Set for all four throughput probe types: `download`, `upload`, `webdownload`, `webupload`.
- **Client-side context switches** — `HttpResult.csw_voluntary` and `csw_involuntary`
  capture the `getrusage(RUSAGE_SELF)` delta (`ru_nvcsw`, `ru_nivcsw`) over the full probe
  duration (Unix only; `None` on Windows).
- **Server-side context switches** — `networker-endpoint` now appends
  `csw-v;dur=N, csw-i;dur=N` to the existing `Server-Timing` header on `/download` and
  `/upload` responses. `ServerTimingResult.srv_csw_voluntary` /
  `srv_csw_involuntary` expose these values in the tester's metrics.
- **TTFB + TLS visibility in throughput terminal output** — `log_attempt()` for
  `download`, `upload`, `webdownload`, `webupload` probes now shows:
  `TLS:Xms` (when applicable), `TTFB:Xms`, `Goodput:X MB/s`, `CPU:Xms`,
  `CSW:Xv/Xi` (client), `sCSW:Xv/Xi` (server).
- **New HTML Throughput Results columns**: Goodput (MB/s), CPU (ms),
  Client CSW (v/i), Server CSW (v/i) alongside the existing TTFB and Total columns.

### Internal
- `parse_server_timing_header()` refactored to return a named `ParsedServerTiming` struct
  (replacing the previous 3-tuple) to accommodate the two new `csw-v`/`csw-i` fields.

---

## [0.10.0] – 2026-02-28 — H1.1 keep-alive fix, TLS cost visibility, named presets, CPU measurement

### Added
- **`pageload` H1.1 keep-alive pool** — corrected a fundamental accuracy bug where each
  asset opened a brand-new TCP+TLS connection. The rewritten probe opens `k = min(6, n)`
  persistent TCP connections (one TLS handshake each for HTTPS) and distributes assets
  across them round-robin, so each connection reuses its TCP/TLS handshake for all its
  assigned assets — exactly how a real browser behaves. This eliminates the previous
  inflation of TLS setup cost and makes the H1.1 vs H2 vs H3 comparison accurate.
- **TLS cost fields on `PageLoadResult`** — four new fields report the cost of TLS
  establishment per page-load variant:
  - `tls_setup_ms`: sum of all TLS handshake durations (H1.1: k handshakes; H2/H3: 1).
  - `tls_overhead_ratio`: fraction of `total_ms` spent in TLS (0.0–1.0).
  - `per_connection_tls_ms`: per-connection handshake durations (length = `connections_opened`).
  - `cpu_time_ms`: process CPU time consumed during the probe (highest for HTTP/3 due to
    QUIC userspace encryption).
- **Named `--page-preset` flag** — selects a predefined asset mix, overriding
  `--page-assets` and `--page-asset-size`:

  | Preset    | Assets | Size per asset | Total    |
  |-----------|--------|---------------|----------|
  | `tiny`    | 100    | 1 KB          | ~100 KB  |
  | `small`   | 50     | 5 KB          | ~250 KB  |
  | `default` | 20     | 10 KB         | ~200 KB  |
  | `medium`  | 10     | 100 KB        | ~1 MB    |
  | `large`   | 5      | 1 MB          | ~5 MB    |
  | `mixed`   | 30     | varied        | ~820 KB  |

  The `mixed` preset (1×200KB + 4×50KB + 10×20KB + 15×5KB) approximates a real-world
  web page with a large hero image, medium assets, and many small scripts/styles.
- **Per-asset sizes in `PageLoadConfig`** — `asset_sizes: Vec<usize>` replaces the old
  uniform `asset_count`/`asset_size` pair. Each element specifies the byte count for
  one asset, enabling varied payloads (used by presets and future per-asset control).
- **Extended Protocol Comparison table** — both the terminal output and the HTML report
  now include `TLS Setup (ms)`, `TLS Overhead %`, and `CPU (ms)` columns, making the
  cost structure of each protocol variant immediately visible.

### Changed
- `PageLoadConfig.asset_count` / `asset_size` → `asset_sizes: Vec<usize>` and
  `preset_name: Option<String>`. Consumers must pass `asset_sizes` (a `Vec`).
- `ResolvedConfig.page_assets` / `page_asset_size` → `page_asset_sizes: Vec<usize>` and
  `page_preset_name: Option<String>`.
- Workspace version bumped to `0.10.0` (MINOR — new fields, new flag, keep-alive fix).

---

## [0.9.0] – 2026-02-28 — HTTP/3 page-load probe

### Added
- **`pageload3` probe mode** — fetches the same N assets as `pageload`/`pageload2` but
  multiplexed over a single QUIC/HTTP/3 connection (`connections_opened = 1`).
  All N asset streams are opened sequentially (fast HEADERS frames) then all responses
  are received concurrently. Requires `--features http3` and an HTTPS target.
  Completes the three-protocol page-load comparison: HTTP/1.1 (≤6 conns) vs
  HTTP/2 (1 TLS conn) vs HTTP/3 (1 QUIC conn), motivated by
  "Does QUIC Make the Web Faster?" (Biswal & Gnawali, IEEE GLOBECOM 2016).
- **`--insecure` support for `pageload3`** — reuses `build_tls_config` from `http.rs`
  (same `NoCertVerifier` + custom CA bundle path), overriding ALPN to `h3`.
- **ALPN warning extended** — startup `[WARN]` now also fires for `pageload3` mode
  against a plain `http://` target.
- **Protocol Comparison table extended** — terminal and HTML report now include a
  `pageload3` row alongside `pageload` and `pageload2`.

### Background
Reference [5] cited in "Does QUIC Make the Web Faster?" for the finding that
bandwidth improvements beyond ~5 Mbps yield diminishing returns on page load time is:
Ilya Grigorik, *"Latency: The New Web Performance Bottleneck"*,
https://www.igvita.com/2012/07/19/latency-the-new-web-performance-bottleneck/, 2012.
This motivates testing all three protocols: the wall-clock difference between `pageload`,
`pageload2`, and `pageload3` reveals which bottleneck (connection setup vs multiplexing
vs QUIC handshake latency) dominates under real network conditions.

---

## [0.8.0] – 2026-02-28 — Page-load simulation, ALPN warning

### Added
- **`pageload` probe mode** — fetches `/page?assets=N&bytes=B` manifest from the endpoint
  then downloads all assets over up to 6 parallel HTTP/1.1 connections (browser-like).
  Measures wall-clock `total_ms`, `ttfb_ms`, `connections_opened`, per-asset timings,
  and total bytes. Configure with `--page-assets N` (default 20) and
  `--page-asset-size <size>` (default 10k, accepts k/m suffixes).
- **`pageload2` probe mode** — same N assets multiplexed over a single HTTP/2 TLS
  connection. Records `connections_opened = 1`. Requires an HTTPS target.
- **`/page` and `/asset` endpoints on `networker-endpoint`** — `GET /page?assets=N&bytes=B`
  returns a JSON manifest listing N asset URLs; `GET /asset?id=X&bytes=B` returns B
  zero bytes (cap 100 MiB).
- **ALPN warning** — startup warns with `[WARN]` when `http2`, `http3`, or `pageload2`
  mode is requested against a plain `http://` target (HTTP/2 requires TLS+ALPN; over
  plain HTTP every connection silently falls back to HTTP/1.1).
- **`PageLoadResult` struct** — `asset_count`, `assets_fetched`, `total_bytes`,
  `total_ms`, `ttfb_ms`, `connections_opened`, `asset_timings_ms`, `started_at`.
  Attached to `RequestAttempt.page_load` (serde-default, skip_serializing_if none).
- **Terminal comparison table** — when both `pageload` and `pageload2` are run in the
  same session, a `Protocol Comparison (Page Load)` table is printed showing N,
  assets, avg connections, p50/min/max total_ms per variant.
- **HTML Protocol Comparison card** — same data rendered as an HTML `<table>` after
  the Statistics Summary section whenever any `pageload`/`pageload2` attempts are present.
- `pageload` and `pageload2` appear in terminal averages + statistics tables, HTML
  Timing Breakdown, and HTML Statistics Summary.

### Changed
- CLI `--modes` help text extended to document `pageload` and `pageload2`.
- `runner/http.rs::build_tls_config` promoted to `pub(crate)` for reuse by `pageload.rs`.
- `cli::parse_size` promoted to `pub(crate)` for reuse in `resolve()`.
- Workspace version bumped to `0.8.0` (MINOR — new features).

---

## [0.7.0] – 2026-02-28 — native-TLS probe, curl probe, tls_backend field

### Added
- **`native` probe mode** — DNS + TCP + platform TLS + HTTP/1.1 using the OS TLS
  stack: SChannel (Windows), SecureTransport (macOS), OpenSSL (Linux). Requires
  recompiling with `--features native` (gates the `native-tls` / `tokio-native-tls`
  deps to avoid mandatory OpenSSL headers on Linux CI). Records leaf certificate
  info via `x509-parser`. TLS version and cipher suite are not exposed by
  `native-tls` and are reported as `"unknown"`.
- **`curl` probe mode** — spawns the system `curl` binary with `--write-out` timing
  fields and maps the output to the same `DnsResult` / `TcpResult` / `TlsResult` /
  `HttpResult` structs as an `http1` probe. Requires `curl` on `$PATH`; returns a
  graceful error at runtime if not found. Supports `--insecure`, `--proxy`,
  `--ca-bundle`, `--ipv4-only`, `--ipv6-only`, and `--timeout`.
- **`TlsResult.tls_backend: Option<String>`** — new serde-default field that records
  which TLS implementation performed the handshake: `"rustls"` for all existing
  rustls-based probes (`http1`, `http2`, `http3`, `tls`), `"native/schannel"` /
  `"native/secure-transport"` / `"native/openssl"` for the `native` probe, and
  `"curl"` for the `curl` probe.
- `native` and `curl` appear in the terminal summary tables, HTML Statistics
  Summary, and HTML Timing Breakdown.

### Changed
- CLI `--modes` help text extended to document `native` and `curl`.
- Workspace version bumped to `0.7.0` (MINOR — new features).

### Fixed
- `runner/tls.rs`: default port for non-HTTPS targets was incorrectly `443`; now `80`.

---

## [0.6.0] – 2026-02-28 — DNS probe, TLS probe, proxy support, CA bundle

### Added
- **`dns` probe mode** — standalone DNS resolution probe (`--modes dns`); records
  resolved IPs, query duration, and success state. No TCP or HTTP activity.
- **`tls` probe mode** — standalone TLS handshake probe (`--modes tls`); performs
  DNS + TCP connect + TLS handshake and records the full certificate chain (all
  certs with Subject, Issuer, SANs, and expiry), negotiated cipher suite, TLS
  version, and ALPN protocol. Advertises both `h2` and `http/1.1` in ALPN to
  discover server preference without sending an HTTP request.
- **`--proxy <url>`** — explicit HTTP proxy URL (e.g. `http://proxy.corp:3128`);
  overrides `HTTP_PROXY`/`HTTPS_PROXY` env vars. For HTTPS targets, a CONNECT
  tunnel is established through the proxy before TLS; for HTTP targets an
  absolute-form URI is used.
- **`--no-proxy`** — disable all proxy detection (both `--proxy` flag and
  `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` env vars). Respects `NO_PROXY` /
  `no_proxy` env var when reading proxy settings from the environment.
- **`--ca-bundle <path>`** — path to a PEM-format CA certificate bundle to add
  to the trust store; useful for corporate CAs not present in the OS store.
  Supported by both HTTP/HTTPS probes and the standalone TLS probe.
- **`CertEntry`** struct in `metrics.rs` — captures `subject`, `issuer`, `expiry`,
  and `sans` (Subject Alternative Names) for each certificate in the chain.
- **`cert_chain: Vec<CertEntry>`** field on `TlsResult` — populated by the
  standalone TLS probe.
- **`proxy` / `ca_bundle`** fields in `ConfigFile` / `ResolvedConfig` / `tester.example.json`.
- Terminal progress logging for `dns` and `tls` protocols.
- HTML and terminal summary tables now include `dns` and `tls` rows.

### Changed
- `RunConfig` gains `ca_bundle: Option<String>`, `proxy: Option<String>`, and
  `no_proxy: bool` fields (all defaulting to `None`/`false`).
- `build_tls_config()` in `runner/http.rs` now returns `anyhow::Result` and
  accepts an optional CA bundle path.
- Workspace version bumped to `0.6.0` (MINOR — new features).

---

## [0.5.0] – 2026-02-28 — Payload-grouped stats + collapsible HTML sections

### Added
- **Payload-grouped statistics** — the terminal Statistics Summary and Averages tables now group
  results by `(protocol, payload_size)` rather than by protocol alone. Running
  `--modes download,upload --payload-sizes 64k,1m,4m` produces separate rows for
  "download 64KiB", "download 1MiB", etc., each with their own N/Min/Mean/p50/p95/p99/Max/StdDev.
- **`attempt_payload_bytes()`** — new public helper in `metrics.rs` that returns the payload
  size for throughput attempts (`http.payload_bytes` or `udp_throughput.payload_bytes`),
  `None` for latency-only probes.
- **`fmt_bytes()` helper in `main.rs`** — formats byte counts as KiB/MiB/GiB for terminal output.
- **Collapsible `<details>` sections in HTML report** (no JS — pure HTML5):
  - **Throughput Results** — one `<details>` per `(proto, payload)` group; summary line shows
    `N runs · avg X MB/s · ±stddev · min Y · max Z`. Expanded by default only when there is
    exactly one group with ≤ 20 rows.
  - **UDP Throughput Results** — same treatment; summary line includes average loss %.
  - **All Attempts** — single collapsible block; summary shows succeeded/failed counts;
    open by default when total attempts ≤ 20.
  - **TCP Stats** — single collapsible block showing connection count; open by default when ≤ 20 rows.
- **Inline CSS** and **`assets/report.css`** updated with `<details>`/`<summary>` styles
  (`▶`/`▼` indicator, `.grp-lbl`, `.grp-meta` classes).

### Changed
- HTML Statistics Summary now emits one row per `(protocol, payload_size)` group, matching
  the terminal output. The "Protocol" column value becomes e.g. "download 64 KiB".
- Terminal averages table header widened from 9 → 16 chars to accommodate grouped labels.
- Workspace version bumped to `0.5.0` (MINOR — new feature).

---

## [0.4.0] – 2026-02-28 — JSON config file support

### Added
- **`--config` / `-c` flag (both binaries)** — accepts a path to a JSON config file. Any key
  from the file can be overridden by a CLI flag (priority: CLI arg > JSON key > built-in default).
- **`--log-level` flag (both binaries)** — set the `tracing` filter directly (e.g.
  `"debug"`, `"info,tower_http=debug"`). Overrides `--verbose` (tester only) and `RUST_LOG`.
- **`ConfigFile` / `ResolvedConfig` structs in `cli.rs`** — all previously hard-defaulted
  tester fields are now `Option<T>` in the raw `Cli` struct; `Cli::resolve(Option<ConfigFile>)`
  merges CLI + file + built-in defaults into a concrete `ResolvedConfig`.
- **`validate()`, `parsed_modes()`, `parsed_payload_sizes()`** moved to `ResolvedConfig`;
  `validate()` gains an explicit `ipv4_only && ipv6_only` conflict check (catches config-file
  sourced conflicts not covered by clap's `conflicts_with`).
- **`tester.example.json`** — repo-root example file showing every tester key with its default
  value.
- **`endpoint.example.json`** — repo-root example file showing every endpoint key with its
  default value.
- New unit tests: `resolved_defaults`, `config_file_overrides_defaults`,
  `cli_overrides_config_file`.

### Changed
- `Cli` struct field types changed from concrete types with `default_value` annotations to
  `Option<T>` (no observable behaviour change — defaults still apply via `resolve()`).
- Existing tests `defaults_parse`, `validate_save_to_sql_without_conn_string_fails`, and
  `payload_sizes_parsed_via_cli` updated to reflect the new raw/resolved split.
- Workspace version bumped to `0.4.0` (MINOR — new feature).

---

## [0.3.3] – 2026-02-28 — Fix RUST_LOG documentation

### Fixed
- **README `RUST_LOG` example** — `RUST_LOG=tower_http=debug` was documented as the way
  to get verbose HTTP logs, but a target-specific directive alone silently suppresses all
  other log targets (including the endpoint's own startup lines). Corrected to
  `RUST_LOG=info,tower_http=debug` with an explanatory note.

---

## [0.3.2] – 2026-02-28 — Endpoint version banner + request logging

### Added
- **Version banner at startup** — `networker-endpoint` now prints its version (e.g.
  `networker-endpoint v0.3.2`) as the first log line before the listening-address lines.
- **HTTP request/response logging** — `TraceLayer` (from `tower-http`) added to the axum
  router; every request is logged at `INFO` with method + URI, and every response with
  status code + latency. Verbosity is controlled by `RUST_LOG`
  (e.g. `RUST_LOG=info,tower_http=debug` for verbose HTTP spans).

---

## [0.3.1] – 2026-02-28 — webdownload/webupload path rewrite

### Fixed
- **`webdownload` and `webupload` path rewrite** — both probes previously left the URL path
  unchanged (e.g. `/health`), so `webdownload` returned whatever the target endpoint happened
  to respond with (e.g. 114 B health JSON) and `webupload` POSTed to a path that ignored the
  request body. Both probes now rewrite the URL path identically to their non-web counterparts:
  `webdownload` → `GET /download?bytes=N`, `webupload` → `POST /upload`. The `--target` flag
  may point at any path; the host and port are preserved and the path is replaced.
- **`--payload-sizes` now required for `webdownload`** — updated CLI help text to document that
  `webdownload` requires `--payload-sizes` (same as `download`), since it now issues a
  `?bytes=N` request and must have a size to request.

---

## [0.3.0] – 2026-02-28 — Web probes, UDP throughput, statistics

> Starting from this release every PR includes a version bump.
> Standard [Semantic Versioning](https://semver.org/) (`MAJOR.MINOR.PATCH`) is used:
> new features → MINOR bump, bug fixes → PATCH bump.

### Fixed
- **`webdownload` ignored `--payload-sizes`** — the mode previously ran once per cycle
  and GETed the target URL as-is, returning whatever the server happened to send (e.g. 114 B
  for a `/health` endpoint). `webdownload` now expands per payload size exactly like `download`,
  and appends `?bytes=N` to the target URL so that any server that supports the parameter (such
  as `networker-endpoint`'s `/download` route) will stream back the requested number of bytes.
  The actual body bytes received are always used for the throughput calculation.
  `--payload-sizes` is now required for `webdownload` (same as `download`).
- **`webupload` absurd throughput when server ignores the request body** — generic targets
  (e.g. a `/health` endpoint) may respond immediately without draining the POST body, making
  `ttfb_ms` near-zero and the computed throughput physically impossible (e.g. 1.3M MB/s).
  `webupload` now uses a dedicated `patch_webupload_throughput` helper that (a) falls back to
  `total_duration_ms` instead of `ttfb_ms` when no `Server-Timing: recv` header is present,
  and (b) caps results at 100,000 MB/s (≈ 800 Gbps — physically impossible on any real link);
  values above the cap are reported as `null`/`—` instead. Four new unit tests cover the
  server-recv, fallback, implausible, and plausible cases.
- **`webdownload`/`webupload` probes always failed** — `run_probe` in the HTTP runner only
  listed `Http1 | Http2 | Tcp | Download | Upload`; both web-probe variants fell through to the
  `other =>` error arm, returning "Protocol not handled by http runner" on every attempt.
  Added `WebDownload | WebUpload` to both match arms (`run_probe` entry point and the
  `send_http1` dispatch inside `run_http_or_tcp`).
- Clippy `redundant_closure` in `html.rs` (`.map(|b| format_bytes(b))`) and `main.rs`
  (`.filter_map(|a| primary_metric_value(a))`); both replaced with the bare function reference.
- Integration test `ServerConfig` initializer missing `udp_throughput_port` field (added in
  the `udpdownload`/`udpupload` PR but not reflected in the test harness).

### Added
- **`udpdownload` probe mode** — bulk UDP download from `networker-endpoint`'s UDP throughput
  server (default port 9998); measures datagrams sent/received, packet loss %, transfer window
  ms, and throughput MB/s. Requires `--payload-sizes`.
- **`udpupload` probe mode** — bulk UDP upload to `networker-endpoint`'s UDP throughput server;
  server reports bytes actually received (CMD_REPORT) so client-side and server-side counts are
  compared. Requires `--payload-sizes`.
- **UDP throughput protocol** — new custom datagram protocol (`b"NWKT"` magic) over a separate
  port. Control packets: CMD_DOWNLOAD, CMD_UPLOAD, CMD_DONE, CMD_ACK, CMD_REPORT. Data packets
  have 8-byte header (seq_num + total_seqs) + up to 1400-byte payload.
- **`UdpThroughputResult`** — new JSON field on `RequestAttempt`; stores remote_addr,
  payload_bytes, datagrams_sent, datagrams_received, bytes_acked, loss_percent, transfer_ms,
  throughput_mbps.
- **HTML UDP Throughput section** — new card in the report showing all UDP throughput attempts
  with loss %, throughput, and bytes-acked.
- **Excel UDP Throughput sheet** — new sheet in the `.xlsx` report.
- **`networker-endpoint --udp-throughput-port`** — new CLI flag (default 9998) for the bulk
  throughput listener.
- **`networker-tester --udp-throughput-port`** — new CLI flag (default 9998) matching the
  endpoint default.
- **`webdownload` probe mode** — GET the target URL as-is (no endpoint path rewriting),
  measures full HTTP phase timing (DNS, TCP, TLS, TTFB, Total) + response body throughput
  + TCP kernel stats. Works with any HTTP server, not just `networker-endpoint`.
- **`webupload` probe mode** — POST to the target URL with a payload body (requires
  `--payload-sizes`), measures full HTTP phase timing + upload throughput + TCP kernel
  stats. Works with any HTTP server.
- Both new modes appear in the HTML Throughput table, TCP Stats card, All Attempts table,
  and Excel Throughput sheet alongside the existing `download`/`upload` modes.
- **TCP Stats card in HTML report** — new section showing all per-connection kernel
  stats: local→remote addresses, MSS, RTT, RTT variance, min RTT, cwnd, ssthresh,
  retransmits, total retransmits, receive window, segments out/in, delivery rate (MB/s),
  and congestion algorithm.
- **Congestion algorithm** — `TCP_CONGESTION` getsockopt added to Linux and macOS;
  stored as `TcpResult.congestion_algorithm` (e.g. "cubic", "bbr").
- **Delivery rate** — `tcpi_delivery_rate` (Linux ≥ 4.9); bytes/sec stored as
  `TcpResult.delivery_rate_bps`; displayed as MB/s in HTML + Excel.
- **Minimum RTT** — `tcpi_min_rtt` (Linux ≥ 4.9); ms stored as `TcpResult.min_rtt_ms`.
- **segs_out / segs_in** — now populated on Linux ≥ 4.2 (were always `None` previously);
  switched from `libc::tcp_info` struct to raw byte-offset reads so all kernel-version-
  gated fields work without a matching libc struct definition.
- `sql/07_MoreTcpStats.sql` — idempotent `ALTER TABLE` adding `CongestionAlgorithm`,
  `DeliveryRateBps`, `MinRttMs` columns to `dbo.TcpResult`.
- Excel TCP Stats sheet gains **Min RTT ms**, **Delivery MB/s**, **Congestion** columns.
- **Statistics Summary** — per-protocol descriptive statistics (N, Min, Mean, p50, p95, p99,
  Max, StdDev) computed from each run's primary metric (total duration ms for HTTP/TCP, RTT avg
  ms for UDP echo, throughput MB/s for all bulk-transfer modes). Shown in three places: (#35)
  - Terminal: second table printed below the existing averages table.
  - HTML report: new "Statistics Summary" card (between Timing Breakdown and UDP Probe
    Statistics); success % column colour-coded green/amber/red.
  - Excel: new "Statistics" sheet (sheet 2, directly after Summary).
- `metrics.rs`: new public `Stats` struct, `compute_stats()`, `primary_metric_label()`, and
  `primary_metric_value()` functions; 3 new unit tests for the percentile calculations. (#35)

---

## [0.2.5] – 2026-02-28 — Install script fixes

### Fixed
- `install.sh`: revert curl URL from `raw.githubusercontent.com` back to public Gist —
  `raw.githubusercontent.com` returns 404 for private repos without authentication. (#29)

### Added
- `.github/workflows/sync-gist.yml`: auto-patches the Gist via GitHub API whenever
  `install.sh` changes on `main`. Requires `GIST_TOKEN` secret (PAT with `gist` scope). (#29)

---

## [0.2.4] – 2026-02-28 — Versioning display + install hardening

### Added
- `networker-endpoint` emits `X-Networker-Server-Version` on every response via the
  `add_server_timestamp` middleware. (#26)
- `ServerTimingResult` gains `server_version: Option<String>` — captured in JSON per attempt. (#26)
- Terminal summary prints both **Client version** and **Server version** rows. (#26)
- HTML report Run Summary card shows Client and Server version rows. (#26)
- Version logged at tester startup. (#26)
- `CHANGELOG.md` added following Keep a Changelog format. (#27)

### Changed
- Workspace version bumped to `0.2.4` in `Cargo.toml` — cascades to both binaries. (#26)

### Fixed
- Upload throughput showed absurdly high values (millions of MB/s) because `total_duration_ms`
  includes receiving the server's JSON response body — noise unrelated to the upload transfer.
  `ttfb_ms` is now the primary denominator: it starts just before `send_request()` and stops
  when the server sends response headers. Because `networker-endpoint` only responds after
  draining the full body, `ttfb_ms ≈ upload wire time`. Formula: `max(server_recv_ms, ttfb_ms)`. (#25)
- `install.sh`: added `--force` to `cargo install` so every run unconditionally rebuilds the
  binary, preventing a stale binary when cargo's git-SHA cache considers the installed rev
  current. (#28)
- `install.sh`: prints the installed version at the end (e.g. `networker-tester 0.2.4`)
  for immediate confirmation. (#28)

---

## [0.2.3] – 2026-02-27 — Upload throughput: max() guard

### Fixed
- Upload throughput: changed denominator from `server_recv_ms.unwrap_or(total_duration_ms)`
  to `max(server_recv_ms, total_duration_ms)` so the larger (correct) value is always used.
  Prevents near-zero `server_recv_ms` (kernel-buffer race on same-machine connections) from
  producing absurdly high throughput values. (#24)

---

## [0.2.2] – 2026-02-27 — Throughput unit tests

### Added
- Full unit test coverage for all throughput calculation paths in `runner/throughput.rs`. (#23)

---

## [0.2.1] – 2026-02-27 — Upload throughput: server-side timing

### Fixed
- Upload throughput: switched denominator to `server_recv_ms` from `Server-Timing: recv;dur=X`
  header — the time the server spent draining the request body, accurate regardless of network
  path. (#22)

---

## [0.2.0] – 2026-02-27 — Extended metrics: TCP kernel stats, retries, server timing, Excel

### Added
- **TCP kernel stats** — 8 new fields on `TcpResult`: `retransmits`, `total_retrans`, `snd_cwnd`,
  `snd_ssthresh`, `rtt_variance_ms`, `rcv_space`, `segs_out`, `segs_in`.
  - Linux: read via `TCP_INFO` socket option (no root required).
  - macOS: read via `TCP_CONNECTION_INFO` (`tcp_connection_info` struct at `IPPROTO_TCP` opt `0x24`).
- **Application-level retries** — `--retries N` CLI flag; failed probes are retried up to N times;
  `retry_count` field added to `RequestAttempt`.
- **Server timing** — `ServerTimingResult` struct captures `Server-Timing` header fields (`recv`,
  `proc`, `total`), `X-Networker-Server-Timestamp`, clock skew estimate, and echoed
  `X-Networker-Request-Id`.
- **Excel output** — `--excel` CLI flag; generates an `.xlsx` report alongside JSON + HTML using
  `rust_xlsxwriter`. 8 sheets: Summary, HTTP Timings, TCP Stats, TLS Details, UDP Stats,
  Throughput, Server Timing, Errors.
- **Privilege notice** — on Linux without root, a startup message explains which metrics are still
  captured vs. what would require elevated privileges.
- **`networker-endpoint` server timing** — `/download` returns `Server-Timing: proc;dur=X`;
  `/upload` returns `Server-Timing: recv;dur=X` and echoes `X-Networker-Request-Id`.
- **SQL migrations** — `sql/05_ExtendedTcpStats.sql` (8 new `TcpResult` columns + `RetryCount`
  on `RequestAttempt`) and `sql/06_ServerTiming.sql` (`ServerTimingResult` table). (#21)

---

## [0.1.0] – 2026-02-27 — Initial release

### Added
- **Workspace** — Cargo workspace with two crates: `networker-tester` (CLI) and
  `networker-endpoint` (server).
- **Probe modes** — `http1`, `http2`, `tcp`, `udp`, `download`, `upload`;
  HTTP/3 gated behind `--features http3`.
- **Per-phase timing** — DNS, TCP connect, TLS handshake, TTFB, total; measured using raw
  `hyper 1.x` connection APIs.
- **TLS** — `rustls 0.23` with `ring` provider; self-signed cert via `rcgen`; `--insecure` flag.
- **UDP echo** — configurable probe count, RTT percentiles, jitter, loss%.
- **Download/upload throughput probes** — `GET /download?bytes=N` and `POST /upload`.
- **Output formats** — JSON, HTML report (embedded CSS, protocol comparison tables),
  SQL Server via `tiberius`.
- **`networker-endpoint`** — axum-based server; routes: `/health`, `/echo`, `/download`,
  `/upload`, `/delay`, `/headers`, `/status/:code`, `/http-version`, `/info`;
  ALPN HTTP/1.1 + HTTP/2; `Server-Timing` headers; `X-Networker-Request-Id` echo.
- **SQL Server schema** — `dbo.TestRun`, `dbo.RequestAttempt`, `dbo.HttpResult`, `dbo.TcpResult`,
  `dbo.TlsResult`, `dbo.UdpResult`, `dbo.ThroughputResult`, `dbo.ServerTimingResult`;
  stored procedures; sample queries.
- **CI** — GitHub Actions on Ubuntu + Windows; `cargo test`, `cargo fmt --check`, `cargo clippy`.
- **Installation script** — public Gist serves `install.sh`; compiles from private repo via SSH.

---

[Unreleased]: https://github.com/irlm/networker-tester/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/irlm/networker-tester/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/irlm/networker-tester/compare/v0.3.3...v0.4.0
[0.3.3]: https://github.com/irlm/networker-tester/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/irlm/networker-tester/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/irlm/networker-tester/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/irlm/networker-tester/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/irlm/networker-tester/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/irlm/networker-tester/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/irlm/networker-tester/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/irlm/networker-tester/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/irlm/networker-tester/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/irlm/networker-tester/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/irlm/networker-tester/releases/tag/v0.1.0
