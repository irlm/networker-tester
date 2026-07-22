# Rust Quality Review — Current Crates (2026-07)

Scope: `networker-tester`, `networker-endpoint`, `networker-log`. Retired crates
(`-dashboard`, `-agent`, `-common`) excluded. Read-only review focused on
measurement correctness, panic/hang paths, error taxonomy, and resource bounds —
not style. CI gates clippy `-D warnings`; 930 lib + 32 integration tests pass.

**Bottom line:** the measurement math is well-defended (mbps guards zero/negative
transfer windows; UDP parsing is length-guarded; DNS timing excludes resolver
construction). The one genuinely product-affecting defect is a **functional gap**:
protocol validation probes never run in live runs, so `validated_http_versions`
and `protocol_runs` are always empty in shipped output. The rest are localized
hang/taxonomy/resource issues.

Several plausible-looking hazards were investigated and **cleared** (see
"Verified non-issues"). They are recorded so they aren't re-flagged.

---

## Ranked findings

### P1 — measurement wrong / crash / hang

**Q1. Protocol validation probes never execute in live runs → `validated_http_versions` + `protocol_runs` always empty.**
`crates/networker-tester/src/url_test_cli.rs:57-60` hardcodes
`protocol_probe_available: false`, and neither
`execute_primary_page_diagnostic_impl` (browser, `url_diagnostic.rs:702`; nor
no-browser, `:956`) ever calls `execute_protocol_validation_probes`
(`url_diagnostic.rs:288`). The only caller is a unit test (`:1207`). Result: the
per-protocol h1/h2/h3 validation table and the negotiated-version list are empty
in every shipped run — the DB columns (`postgres.rs:1135`, `mssql.rs:549`) persist
empty strings, and JSON output carries `[]`.
**Fix:** in `run_url_test_cli`, set `protocol_probe_available` from
`detect_capabilities()` (not `false`) and call
`orchestrator.execute_protocol_validation_probes(&mut run, &request).await?`
after `execute_primary_page_diagnostic`. Size: ~10 lines + one integration test
asserting non-empty `protocol_runs` on a live run.

---

### P2 — latent hang / error-taxonomy / resource

**Q2. HTTP/3 request/response/body phases have no per-phase timeout.**
`crates/networker-tester/src/runner/http3.rs:387` (`send_request`), `:425`
(`recv_response`), `:453` (`recv_data` body loop) each bare-`await`. Only the
QUIC connect is timeout-wrapped (`:315`). A server that completes the handshake
then stalls at the HTTP/3 layer relies solely on QUIC's idle timeout as a
backstop; a slow-but-alive server (periodic keepalive, dribbled body) can stall
the probe well past the configured `timeout_ms`, contaminating `total_ms`.
**Fix:** wrap each of the three phases in
`tokio::time::timeout(Duration::from_millis(cfg.timeout_ms), …)` and classify a
fire as `ErrorCategory::Timeout`.

**Q3. HTTP request-phase errors collapse to `ErrorCategory::Http` unless the message contains "timed out".**
`crates/networker-tester/src/runner/http.rs:637-641` classifies by substring on
`"timed out"` only. Connection resets, broken pipes, and mid-stream TLS failures
surfaced by reqwest all become `Http`, mislabeling transport/TLS failures as
HTTP-layer failures and misdirecting incident triage.
**Fix:** inspect the reqwest error kind (`e.is_connect()` → `Tcp`,
`e.is_timeout()` → `Timeout`, TLS-source → `Tls`) before defaulting to `Http`.

**Q4. `/asset` allocates the full response body (up to 100 MiB) in memory per request.**
`crates/networker-endpoint/src/routes.rs:1083`: `Body::from(vec![0u8; n])` with
`n` capped at 100 MiB. The sibling `/download` path deliberately streams
(`:834` comment) to avoid this; `/asset` does not. A few concurrent
`/asset?bytes=104857600` requests spike memory into the GBs on a diagnostic
server with no concurrency cap.
**Fix:** stream the zero fill with `Body::from_stream` over a fixed chunk
generator (mirror `download_response`), or lower the `/asset` cap sharply.

**Q5. HTTP/3 QUIC connection spawns one task per stream/connection with no bound.**
`crates/networker-endpoint/src/http3_server.rs` spawns per-connection (`:48`) and
per-request (`:107`) tasks without a semaphore. The endpoint is a diagnostic
target that legitimately receives load-test traffic; an unbounded stream/connection
fan-out is a task/memory-exhaustion vector.
**Fix:** gate spawns behind a `tokio::sync::Semaphore` (per-connection stream cap
+ global connection cap).

**Q6. Log DB sink drops entries silently under backpressure with no drop metric.**
`crates/networker-log/src/db_layer.rs:~202` uses `try_send` and drops on a full
channel (`batch.rs` `CHANNEL_CAPACITY = 10_000`) with no counter. In a long run
against a slow/down Postgres sink, log loss is invisible — the exact moment you
most need the logs (an incident) is when they vanish.
**Fix:** increment a dropped-entry counter on `try_send` failure and emit a
throttled `eprintln`/metric so loss is observable; document the drop policy.

---

### P3 — polish / defense-in-depth

**Q7. DNS `lookup_ip` isn't bounded by the probe's configured `timeout_ms`.**
`crates/networker-tester/src/runner/dns.rs:107` bare-`await`s `lookup_ip`. It is
*not* an unbounded hang — hickory's `ResolverOpts` default (`:61`, `:66`) caps
per-query timeout (~5s) × attempts, so worst case is ~10s regardless of the
user's `timeout_ms`. Still, DNS timing can exceed a user's 2s timeout without
being cut off.
**Fix:** wrap `lookup_ip` in `tokio::time::timeout(timeout_ms, …)` so DNS honors
the same budget as the rest of the probe.

**Q8. `Response::builder()…body().unwrap()` in endpoint handlers.**
Multiple sites (`routes.rs:1073`, `:1084`, header `from_str().unwrap()` at
`:1095`, etc.). Bodies/headers here are internally constructed so the unwraps are
locally provable; low risk. Left as defense-in-depth: prefer `expect("static
response body is valid")` to document the invariant, or return an error response.

**Q9. HAR artifact copy uses `file_name().unwrap_or_default()`.**
`crates/networker-tester/src/url_test_cli.rs:144`: an empty file name silently
copies to the directory root. Guard with an explicit skip + `capture_error` when
`file_name()` is `None` rather than producing a garbage destination path.

---

## Verified non-issues (investigated, cleared — do not re-flag)

- **UDP truncated-packet parsing** (`udp_throughput.rs:357`, `:444`): the
  `try_into().unwrap_or([0;4])` never fires — `:357` is guarded by
  `n > DATA_HDR_LEN` (8) and `:444` by `n == CTRL_LEN`, and the slice is a fixed
  `[..4]`/`[8..12]` of a pre-sized buffer. No silent zeroing occurs.
- **Throughput negative/zero transfer window** (`throughput.rs:607` → `mbps`):
  `mbps` (`:701`) returns `None` for `transfer_ms <= 0.0` (test
  `mbps_negative_transfer_ms_returns_none`, `:794`). No NaN/Inf/divide-by-zero.
- **Endpoint `/proc/meminfo` blocking read** (`routes.rs:271`): called only from
  `SystemMeta::collect()` at startup (`:76`, documented "blocking is acceptable"),
  not per request.
- **summary.rs / baseline.rs empty-slice division/index panics**: all flagged
  sites are preceded by `is_empty()` early returns; guards are present, not just
  "brittle". No live panic path found.
