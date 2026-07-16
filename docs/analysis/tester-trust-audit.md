# networker-tester Trust Audit

Read-only audit of the Rust probe engine (`crates/networker-tester`), covering (1) metrics
completeness, (2) measurement validity, (3) test-suite validity. Every finding was verified by
re-reading the cited code. Date: 2026-07-15. Baseline: `main` @ 0064038.

Severity: **P0** = measurements are wrong/misleading today · **P1** = missing metric or invalid
methodology that undermines trust · **P2** = improvement.

---

## Executive summary

The per-phase timing core (Instant-based, correct phase ordering, kernel TCP_INFO telemetry,
bootstrap-CI adaptive benchmarking) is fundamentally sound — but five P0 defects make specific
numbers wrong today: TLS handshake time includes OS trust-store loading on every attempt
(rustls, native, and standalone TLS probes); UDP jitter is computed on *sorted* RTTs and is
therefore not jitter at all; UDP-upload loss% is fabricated (`received = sent`) and its
throughput window includes the CMD_REPORT wait; and every DNS measurement queries hardcoded
Google DNS (8.8.8.8) instead of the system resolver, silently measuring a path the user's
applications never take. A cluster of P1s further erodes cross-mode comparability: three
different HTTP "success" rules (<400 vs <500), download throughput computed from *requested*
rather than received bytes, all-zero (maximally compressible) payloads, HTTP/3 errors all
classified `Http`, and p95/p99 printed for the default n=3. The test suite has exactly one
timing-accuracy assertion (TTFB ≥ 90 ms on /delay?ms=100, lower-bound only), so every P0 above
is invisible to CI; the jitter unit tests use pre-sorted inputs, masking the sort bug.

---

## 1. Metrics inventory (what exists today)

| Domain | Captured today | Source |
|---|---|---|
| DNS | resolution duration_ms, resolved IP list, query name, success | `runner/dns.rs` (`DnsResult`, metrics.rs:807) |
| TCP | connect ms, local/remote addr, MSS, smoothed RTT + variance, cwnd, ssthresh, retransmits (in-flight + lifetime), rcv_space, segs in/out, congestion algorithm, delivery rate, kernel min-RTT (Linux/macOS best-effort) | `runner/socket_info.rs`, `TcpResult` (metrics.rs:817) |
| TLS | handshake ms, protocol version, cipher suite, ALPN, leaf subject/issuer/expiry, full chain + SANs (tls mode), backend label, resumption kind (full/full-hrr/resumed), TLS1.3 ticket count, cold-vs-warm handshake pair (tlsresume) | `runner/tls.rs`, `runner/http.rs:1028` |
| HTTP | status, headers/body size, TTFB (send→first header byte), total ms, negotiated version, response headers, payload bytes, throughput MB/s, goodput MB/s, process CPU ms, voluntary/involuntary context switches (Unix), server-side timing (recv/proc/total, server CSW), clock-skew estimate, upload byte verification (X-Networker-Received-Bytes) | `runner/http.rs`, `runner/throughput.rs` |
| HTTP/3 | QUIC handshake ms (as `TlsResult`), TTFB, total, status, body size, CPU/CSW | `runner/http3.rs` |
| UDP echo | per-probe RTTs, min/avg/p95, jitter (buggy — see V2), loss %, success count | `runner/udp.rs`, `aggregate_udp_rtts` |
| UDP throughput | datagrams sent/received, bytes_acked (upload), loss %, transfer window, MB/s | `runner/udp_throughput.rs` |
| Page-load | asset count/fetched, total bytes, total ms, TTFB, connections opened, per-asset timings, per-connection TLS ms, TLS overhead ratio, CPU ms, warm-connection flag | `runner/pageload.rs` |
| Browser | load ms, DOMContentLoaded ms, TTFB (Navigation Timing L1), resource count, transferred bytes, per-protocol resource counts | `runner/browser.rs` |
| Baseline / env | TCP-connect RTT baseline (min/avg/max/p50/p95), network classification (loopback/LAN/Internet), pre-benchmark environment + stability checks (incl. correct ordered jitter), client+server HostInfo (OS, arch, cores, RAM, region), packet-capture summary (retransmissions, dup-ACKs, resets, transport shares) | `baseline.rs`, `capture.rs` |
| Aggregates | count/min/mean/p50/p95/p99/max/stddev per protocol×payload; benchmark mode: bootstrap median CI (1024 resamples, 95%), adaptive sample sizing, pilot/overhead/warmup/cooldown phase separation | `metrics.rs:1439`, `benchmark.rs` |

## 2. Metrics gaps

| Gap | Sev | Benchmark tool that has it | Suggested implementation |
|---|---|---|---|
| Resolver identity + system-resolver path never recorded (and Google DNS hardcoded — see V1) | P0/P1 | curl (`time_namelookup` via system), dig | `tokio_from_system_conf()`; add `resolver` field to `DnsResult` |
| No per-record-type DNS timing (A vs AAAA), CNAME chain, TTLs, DNSSEC, cache hit/miss | P1 | dig, smokeping DNS probe | hickory `lookup(RecordType::A/AAAA)` separately; record TTL + chain |
| No QUIC-level stats: path RTT, lost packets, congestion events, 0-RTT, version negotiation, migration | P1 | curl (`--http3` + stats), qlog | quinn `Connection::stats()` and `rtt()` after transfer — nearly free |
| Redirect chains not followed or recorded (`redirect_count` hardcoded 0) | P1 | curl (`num_redirects`, `url_effective`) | optional follow-redirects mode recording per-hop timing |
| No content validation on downloads (size/hash) — throughput uses requested bytes | P1 | iperf3 (verifies stream), curl (`size_download`) | assert `body_size_bytes == requested`; optional xxhash of body |
| No UDP one-way delay, reordering %, or RFC 3550 jitter; send timestamp embedded but unused | P1 | iperf3 (jitter/OWD per RFC 3550), mtr | match echoes by seq across the whole window; compute interarrival jitter |
| Throughput is single-shot: no time-sliced sub-interval stats (ramp/sustained vs burst), no pacing, no parallel streams | P1 | iperf3 (`-i 1` intervals, `-P`), k6 | sample bytes-transferred every 100 ms during transfer; report per-slice MB/s + slope |
| TLS: no handshake sub-phase split, OCSP stapling status, not_before/days-to-expiry, key-exchange group, 0-RTT | P2 | openssl s_client, ssllabs | rustls exposes negotiated group; OCSP via `ocsp_response` in verifier |
| TCP kernel stats sampled only immediately after connect — retransmits *during* transfer invisible | P2 | iperf3 (`tcpi` at end), ss | re-sample `SocketInfo` after body completes; report delta |
| Browser probe: no FCP/LCP/CLS, uses deprecated Navigation Timing L1 | P2 | Lighthouse, WebPageTest | CDP `Performance.getMetrics` / PerformanceObserver injection |
| Local interface name, MTU, default gateway not captured (MSS is a proxy) | P2 | iperf3/mtr environment blocks | getifaddrs lookup keyed by `local_addr` |
| Proxy usage not flagged in results (env proxies silently reroute measurements) | P1 | curl (`--write-out %{url_effective}` + verbose) | add `proxy_used: Option<String>` to `RequestAttempt` |
| Cold/warm not annotated: every attempt is a fresh DNS+TCP+TLS (by design) but nothing in the output says so; only pageload has `connection_reused` | P2 | curl `--keepalive`, k6 reuse control | document; add `connection_cold: bool` for symmetry |
| No happy-eyeballs / per-IP fallback; first resolved IP only (`pick_ip`, http.rs:1097) | P2 | curl | try v6 then v4 with 250 ms stagger, record which won |

## 3. Validity findings (are the numbers trustworthy?)

Clock use is correct throughout: all durations use `std::time::Instant` (monotonic); `Utc::now()`
appears only in metadata timestamps and the (inherently wall-clock) skew estimate. No
SystemTime-in-timing-path misuse was found.

### V1 — P0: All DNS measurements go to hardcoded Google DNS, not the system resolver
- **Evidence:** `runner/dns.rs:21` — `TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default())`. In hickory-resolver 0.24, `ResolverConfig::default()` is Google Public DNS (8.8.8.8/8.8.4.4 + v6). Every probe that resolves (http, tls, native, tlsresume) funnels through this.
- **Impact:** DNS timing measures a path the user's stack never uses; on networks where 8.8.8.8 is blocked (common in enterprise — this product's audience) the entire probe fails with a DNS error while the OS resolves fine. The resolver used is not recorded, so the report cannot even be interpreted correctly. Additionally, HTTP/3 and UDP probes use `tokio::net::lookup_host` (system resolver) — so different modes resolve via *different* resolvers within one run (`http3.rs:152`, `udp.rs:55`).
- **Fix:** `TokioAsyncResolver::tokio_from_system_conf()` (fall back to Google only if unavailable, and say so); record resolver address + source in `DnsResult`; unify H3/UDP resolution.
- Verified in code; "default = Google" is per hickory 0.24 documented behavior — a 1-minute runtime confirm: run `--modes dns` with 8.8.8.8 blackholed and observe failure while `dig` succeeds.

### V2 — P0: UDP jitter is computed on sorted RTTs — it is not jitter
- **Evidence:** `metrics.rs:1410-1420` — `rtts.sort_by(...)` happens *before* `windows(2)` successive-difference jitter. Mean of successive diffs of a sorted array telescopes to `(max − min)/(n − 1)` — a range statistic, not inter-arrival variation. The codebase itself has the correct version: `baseline.rs:165-174` `average_jitter_ms` operates on arrival-ordered samples.
- **Impact:** `UdpResult.jitter_ms` systematically underestimates and is uncorrelated with real jitter ordering (e.g. alternating 1/10/1/10 ms RTTs — true mean |Δ| = 9 ms — reports ≈ 1 ms).
- **Fix:** compute jitter from `probe_rtts_ms` in arrival order (skipping `None`s) before sorting; ideally RFC 3550 smoothed estimator. Add a unit test with out-of-order input (current tests use ascending input, masking the bug — see T2).

### V3 — P0: UDP upload loss% and datagrams_received are fabricated
- **Evidence:** `runner/udp_throughput.rs:204-217` — `datagrams_received: datagrams_sent` (the client cannot know this), and `loss = loss_percent(total_seqs, datagrams_sent)` counts only *local send() failures*. The server's authoritative `bytes_acked` (CMD_REPORT, line 397) is stored but not used for loss or throughput; throughput numerator is the full `payload_bytes` even if the server received a fraction.
- **Impact:** Under real network loss, udpupload reports 0% loss and inflated throughput. This is exactly the metric UDP bulk mode exists to measure.
- **Fix:** `loss = 1 − bytes_acked/payload_bytes`; throughput numerator = `bytes_acked`; report `datagrams_received: None`/unknown rather than a fabricated value (schema-version note: field is non-optional; bump or repurpose).

### V4 — P0: UDP upload transfer window includes the CMD_REPORT wait
- **Evidence:** `runner/udp_throughput.rs:378-398` — `t0` starts before the send loop, but `transfer_ms = t0.elapsed()` is taken *after* `wait_for_report(sock, timeout_ms)`, which blocks up to the full timeout when the report is lost/late.
- **Impact:** a single lost 12-byte CMD_REPORT turns a 50 ms transfer into `timeout_ms + 50 ms`, collapsing reported throughput by orders of magnitude.
- **Fix:** capture `transfer_ms` immediately after CMD_DONE is sent; use the report only for `bytes_acked` (and prefer a server-side receive window, which the endpoint could echo in CMD_REPORT).

### V5 — P0: TLS handshake timing includes trust-store construction on every attempt
- **Evidence:** three independent sites start the TLS stopwatch *before* building the client config:
  - `runner/http.rs:392-395` — `t_tls = Instant::now()` then `build_tls_config()` (which does `webpki_roots` extend + `rustls_native_certs::load_native_certs()` — an OS keychain/disk read — + optional CA-bundle file I/O) → duration read at :468.
  - `runner/tls.rs:168-171` — same pattern in the standalone TLS probe.
  - `runner/native.rs:438-470` — `t_tls` at :439, then connector build including `load_native_ca_bundle` file read.
- **Impact:** `load_native_certs()` costs one-digit to tens of ms (macOS Security framework, Windows cert store) and is re-executed *per attempt*. On LAN/loopback targets where a real handshake is ~1-5 ms, reported TLS time can be dominated by local disk/keychain I/O. This also poisons `tls_overhead_ratio` in pageload and `goodput_mbps` (overhead sum) in throughput modes. HTTP/3 is affected the same way: `build_quic_endpoint` (which calls `build_tls_config`) runs after `t0` (total) though before `t_handshake` — so H3 *total_ms* absorbs it (`http3.rs:195-273`).
- **Fix:** build `ClientConfig` once per run (or lazily cache in a `OnceLock`) and always start the handshake timer at `connector.connect(...)`. Cache also removes per-attempt allocation noise.

### V6 — P1: Three inconsistent HTTP success rules across probe modes
- **Evidence:** `http.rs:553` `success: h.status_code < 500` (4xx = success); `native.rs:385,658` `< 400`; `curl.rs:258` `< 400`; `http3.rs:420` `< 500`.
- **Impact:** the same 404 target shows http1 "✓" and native/curl "✗"; success-rate comparisons across modes (a headline product feature) are apples-to-oranges. Retry logic (`main.rs:981-988`) also fires differently per mode.
- **Fix:** one rule (recommend: transport success + record status; expose `http_success` separately), documented in the JSON contract.

### V7 — P1: Download throughput computed from requested bytes with no body verification
- **Evidence:** `runner/throughput.rs:425,535` — `patch_throughput(h, payload_bytes)` uses the *requested* size; nothing checks `h.body_size_bytes == payload_bytes`. Inconsistently, webdownload uses the *actual* body size (`throughput.rs:183-185`). Uploads do verify (`verify_upload`, :96) — downloads have no equivalent.
- **Impact:** a 404/500 (still `success` under V6) or truncated body yields a throughput figure for bytes that never arrived.
- **Fix:** mirror `verify_upload` for downloads; compute MB/s from `body_size_bytes` and fail the attempt on mismatch.

### V8 — P1: Throughput transfer window includes the HTTP connection handshake
- **Evidence:** `http.rs:482-483` — `t_http` starts before `send_http1/2` which perform the hyper handshake (h2: client preface + SETTINGS exchange) inside; `ttfb_ms` starts at `t_sent` *after* the handshake (:612-622). Download window = `total − ttfb` (`throughput.rs:596-598`) therefore includes handshake time in the "body receive" denominator.
- **Impact:** systematic throughput underestimation for download2/download3 on high-RTT paths with small/medium payloads (one extra RTT in the denominator); distorts H1-vs-H2-vs-H3 comparisons — the product's core comparative claim.
- **Fix:** either timestamp first-body-byte→last-body-byte explicitly, or subtract a measured handshake sub-phase (record `http_handshake_ms` = `t_sent − t_http`).

### V9 — P1: All-zero payloads (uploads and endpoint downloads)
- **Evidence:** `http.rs:671-689` — 256 KiB static `ZERO_BYTES` streamed for all uploads; `http3.rs:328` `vec![0u8; 16*1024]`; UDP throughput data packets are zero-filled (`udp_throughput.rs:382`).
- **Impact:** any compressing/deduplicating middlebox, VPN, WAN optimizer, or proxy inflates measured throughput arbitrarily (zeros compress ~1000:1). iperf3 uses pseudorandom payloads for exactly this reason. For a tool used across enterprise WANs this is a real-world distortion, not theoretical.
- **Fix:** fill the buffer once with a fast PRNG (xorshift) at startup; keep zero-copy chunking.

### V10 — P1: HTTP/3 error taxonomy collapsed to `Http`; DNS/TCP phases absent
- **Evidence:** `http3.rs:452-486` — `h3_failed` always sets `ErrorCategory::Http`: "QUIC handshake timeout" (:269) should be `Timeout`; "DNS error" (:157) should be `Dns`; QUIC connect/TLS failures should be `Tls`/`Tcp`-analog. Also `dns: None, tcp: None` always (:421-422) — H3 resolves via `lookup_host` with no timing, so `compute_overhead_ms` (throughput.rs:135) and goodput omit DNS for H3 while including it for H1/H2.
- **Fix:** classify by phase at each `h3_failed` call site; time the `resolve_addr` call and emit a `DnsResult`.

### V11 — P1: UDP RTT statistics poisoned by fully-lost attempts
- **Evidence:** `udp.rs:124-152` — a 100%-loss attempt still gets `udp: Some(UdpResult{ rtt_avg_ms: 0.0, ... })`; `summary.rs:162-168` computes protocol stats via `primary_metric_value` without filtering `a.success`, so each dead attempt contributes 0.0 ms to min/mean/p50.
- **Impact:** a flaky UDP path reports *better* (lower) average RTT the more attempts fully fail. Min will read 0.00 ms.
- **Fix:** `primary_metric_value` for Udp should return `None` when `success_count == 0` (or stats should filter on success).

### V12 — P1: UDP echo probe mismatch handling causes cascading false loss
- **Evidence:** `udp.rs:168-200` — one `recv` per probe; a late echo from probe N arriving during probe N+1 is read, fails the `echo_seq == seq` check, and marks N+1 lost too (N was already counted lost). Probes are also strictly serialized with no send interval, so a single delayed packet stalls the run by `timeout_ms`.
- **Fix:** loop on `recv` until deadline matching any outstanding seq (map seq→send Instant); this also enables reordering and one-way-delay metrics (the embedded timestamp at :173-175 is currently never read back).

### V13 — P1: p95/p99 reported at default n=3 with no sample-size guard
- **Evidence:** default `runs = 3` (`cli.rs:783`); `print_summary` (summary.rs:155-183) and HTML/Excel always print p95/p99. With n=3 the interpolated p99 is effectively max, but the label claims a tail estimate.
- **Fix:** suppress or asterisk p95/p99 below n≈20/n≈100 respectively; or print CI from the existing bootstrap machinery (benchmark.rs already has it — reuse outside benchmark mode).

### V14 — P1: `--concurrency > 1` runs *different probe types* simultaneously
- **Evidence:** `main.rs:999-1006` — `buffer_unordered(cfg.concurrency)` over the flattened `mode_tasks` list, so e.g. a 1 GiB download and an http1 latency probe run concurrently on the same link, and both contend. Additionally CPU time and context switches are measured process-wide (`cpu_time::ProcessTime`, `getrusage(RUSAGE_SELF)` — http.rs:167-169, 539-544), so concurrent probes cross-attribute each other's CPU/CSW.
- **Impact:** latency numbers taken during a concurrent throughput transfer are dominated by self-induced queueing; CPU comparison (a marketed H1/H2/H3 differentiator) becomes noise at concurrency > 1.
- **Fix:** document that concurrency > 1 invalidates latency isolation; group throughput modes to run serially; per-attempt CPU via per-thread clock (`clock_gettime(CLOCK_THREAD_CPUTIME_ID)`) where the probe is pinned, or annotate results with `concurrency` (already in TestRun — surface a warning in reports).

### V15 — P1: Redirects neither followed nor counted; curl misreports protocol version
- **Evidence:** `http.rs:763` and `curl.rs:248` hardcode `redirect_count: 0`; hyper conn-level API never follows redirects; a 301 counts as success (V6) with a ~0-byte body. `curl.rs:242` hardcodes `negotiated_version: "HTTP/1.1"` while the spawned curl auto-negotiates h2 on HTTPS (`--write-out` includes no `%{http_version}` and no `--http1.1` flag is passed).
- **Impact:** TTFB of a redirect page is silently reported as the target's TTFB; curl-vs-http1 comparisons may actually compare h2-vs-h1.
- **Fix:** pass `--http1.1` (or capture `%{http_version}` and `%{num_redirects}`); in native probes, surface 3xx explicitly and optionally follow with per-hop records.

### P2 validity notes (verified, lower impact)
- `mbps()` divides by 1024² but labels "MB/s" — values are MiB/s (`throughput.rs:664`, `udp_throughput.rs:435`). Fix label or divisor.
- curl TTFB baseline subtracts `time_appconnect`, not `time_pretransfer` (`curl.rs:228-233`) — includes request-prep gap curl itself excludes.
- H3 `TlsResult.started_at` is set to `Utc::now()` *after* the body completes (`http3.rs:390-400`) — timestamp semantics wrong.
- `SocketInfo` snapshot is taken immediately after connect only (http.rs:296); retransmits/cwnd during the transfer are never observed. Re-sample post-transfer.
- DNS `duration_ms` includes resolver construction (`dns.rs:19-21` — t0 before `TokioAsyncResolver::tokio(...)`); cheap today, not if switched to system-conf (file read) — start the timer at `lookup_ip`.
- Retries publish only the final attempt (`dispatch.rs:422-424` `published_logical_attempts` = last only); intermediate failures vanish from the run (only `retry_count` survives). Document, or emit discarded attempts with a `superseded` flag.
- Per-protocol "Avg" table (summary.rs:109-144) mixes partial phase timings from failed attempts with successful ones.
- Clock-skew estimate assumes symmetric path (`ttfb/2`, http.rs:812-818) — fine, but should be labeled an estimate in reports.
- `headers_size_bytes` approximates wire size (k+v+4) — HTTP/2 HPACK makes this not a wire measurement; rename or document.
- Self-measurement overhead is otherwise well controlled: logging (`log_attempt`) happens after timing regions; serialization is post-run. The one exception is config/trust-store construction (V5).

## 4. Test-suite findings (do the tests prove correctness?)

### T1 — P1: Only one timing-accuracy assertion in the whole suite, and it's one-sided
- **Evidence:** `tests/integration.rs:397-417` `http1_delay_endpoint_respected` asserts `ttfb_ms >= 90.0` for `/delay?ms=100`. No upper bound; no equivalent for TCP, TLS, DNS, total, pageload, browser, or throughput. Everything else asserts presence/nonzero (`>= 0.0`, `> 0.0`, field is Some).
- **Impact:** none of V2–V5, V7–V8 can be caught by CI. A regression that doubles every reported TLS time would pass all tests.
- **Fix:** use the existing `/delay` endpoint for two-sided bounds (`90 ≤ ttfb ≤ 100 + ε_CI`); add: total ≈ delay + ε; TLS-time sanity on loopback (`< 50 ms`); a rate-limited download route (or token-bucket in the test endpoint) asserting measured MB/s within ±15% of the configured rate; UDP loss accuracy via a stub server that drops every Nth datagram (assert loss ≈ 1/N for both udp and udpupload — the latter fails today per V3).

### T2 — P1: Jitter unit tests use pre-sorted input, masking the V2 sort bug
- **Evidence:** `metrics.rs:1578-1606` — all `aggregate_udp_rtts` tests feed ascending sequences (1..10; 5,10,15), for which sorted == arrival order.
- **Fix:** add `[1, 10, 1, 10]` (expect jitter 9.0, not 1.0) — this test fails on current code and locks in the fix.

### T3 — P1: Error paths largely untested
- Verified present: connection-refused (h3 only, `http3.rs` tests), unresolvable host (dns + h3), CA-bundle errors, curl/native missing-binary/feature stubs, 404 status capture (`integration.rs:420-434` — asserts status only, not the success flag).
- Missing: HTTP timeout (delay > timeout → assert `ErrorCategory::Timeout` and that no partial HttpResult leaks into stats); TCP refused for http1 (assert `Tcp` category); TLS verification failure (self-signed *without* `insecure` → assert `Tls`); 5xx semantics (nothing pins the `< 500` / `< 400` rules — pinning them would have exposed V6); upload-mismatch end-to-end (unit-tested only); proxy path (CONNECT tunnel logic has zero tests).

### T4 — P2: Statistics functions — good unit coverage
`compute_stats` is tested against hand-computed known values including stddev and interpolated
percentiles (metrics.rs:1892-1906); benchmark bootstrap/median/adaptive-stop logic has tests in
`main.rs`'s test module. This part is solid. Gap: no property test that p50 ≤ p95 ≤ p99 ≤ max.

### T5 — P2: Flakiness risks / why `--test-threads=1` is required
- `free_port()` (integration.rs:39-53) is bind-drop-reuse TOCTOU; every test boots a full 4-listener endpoint. Parallel tests can collide on ports and saturate loopback → serialized by convention (documented in CLAUDE.md, not enforced in code — consider `serial_test` or a shared `OnceCell` endpoint).
- `runner/dns.rs` unit tests (`unresolvable_hostname_returns_error`, `resolves_localhost` at dns.rs:121-155) require reachability of Google DNS (per V1) — they fail on airgapped CI even though "localhost" itself is hosts-file served; the negative test needs a live resolver to return NXDOMAIN.
- Browser/curl/native tests self-skip gracefully — good pattern.

### T6 — P2: Feature-matrix and contract coverage
- `--no-default-features` stub is CI-built and its error message is asserted (integration.rs:890-899); http3 tests gated on the feature; both DB backends have `#[ignore]`d integration tests with fixtures. Adequate.
- `tests/json_contract.rs` pins `schema_version == "1.0"` and per-phase field *presence* — but not shape: a renamed optional field (e.g. `goodput_mbps`) would not trip it. Add a golden-file snapshot of a fully-populated attempt (serde_json to committed fixture) so any silent shape drift is caught before the C# consumer breaks.

## 5. Prioritized TODO

### P0 — measurements are wrong today (5)
1. Move TLS/QUIC client-config construction out of the handshake timer; cache config per run (`http.rs:392`, `tls.rs:168`, `native.rs:439`, `http3.rs` total). 
2. Compute UDP jitter on arrival-ordered RTTs (RFC 3550-style); add the out-of-order regression test (`metrics.rs:1410`).
3. UDP upload: derive loss and throughput numerator from `bytes_acked`; stop fabricating `datagrams_received` (`udp_throughput.rs:204-217`).
4. UDP upload: end the transfer window at CMD_DONE, not after the CMD_REPORT wait (`udp_throughput.rs:398`).
5. Resolve via the system resolver (with recorded fallback), record resolver identity, unify H3/UDP resolution (`dns.rs:21`).

### P1 — trust/methodology (13)
6. Unify HTTP success semantics across http/native/curl/h3; pin with a test (V6/T3).
7. Verify download body size; compute download MB/s from received bytes (V7).
8. Exclude the HTTP connection handshake from throughput windows; record `http_handshake_ms` (V8).
9. Random-fill upload/download payloads (V9).
10. Fix H3 ErrorCategory classification; add H3 DNS timing (V10).
11. Exclude fully-lost UDP attempts from RTT stats (V11).
12. Rework UDP echo receive loop (outstanding-seq map → fixes false loss, enables reordering + one-way delay) (V12).
13. Guard/annotate p95/p99 at small n; surface bootstrap CIs outside benchmark mode (V13).
14. Serialize throughput vs latency probes under `--concurrency`; per-thread CPU attribution or warning (V14).
15. Record redirects; fix curl protocol attribution (`--http1.1` or `%{http_version}`) (V15).
16. Record proxy usage in `RequestAttempt` (gap table).
17. Two-sided timing-accuracy tests on `/delay` + rate-limited throughput accuracy test + drop-every-Nth UDP loss test (T1).
18. Timeout/TLS-failure/5xx error-path tests asserting `ErrorCategory` (T3).

### P2 — improvements (10)
19. QUIC stats from quinn (`Connection::stats()`/`rtt()`); 0-RTT + version-negotiation capture.
20. Re-sample `SocketInfo` after transfer; report retransmit delta.
21. DNS per-record-type timing, CNAME chain, TTL capture; start timer at `lookup_ip`.
22. Time-sliced throughput sub-intervals (ramp vs sustained), optional pacing/parallel streams.
23. Fix MB/s vs MiB/s labeling; curl `time_pretransfer` baseline; H3 `TlsResult.started_at`.
24. Browser: FCP/LCP via CDP Performance domain; migrate off Navigation Timing L1.
25. Interface/MTU capture; happy-eyeballs with which-family-won recorded.
26. Emit superseded retry attempts (or document last-only publishing); exclude failed attempts from the phase-average table.
27. Golden-file JSON contract snapshot; endpoint fixture sharing + `serial_test` to de-flake integration tests.
28. TLS extras: OCSP stapling status, key-exchange group, not_before/days-to-expiry.
