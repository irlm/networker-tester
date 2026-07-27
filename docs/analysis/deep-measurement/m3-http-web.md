# M3 Deep Measurement Audit — HTTP & Web Layer

**Date:** 2026-07-27 · **Scope:** `http1/http2/http3`, `curl`, throughput modes
(`download*/upload*/web*`), native page-load (`pageload*`), real-browser
(`browser*`), `websocket`. · **Predecessor:** `docs/analysis/measurement-gap-analysis-2026-07.md`
(2026-07-24) — this report goes one level deeper on the HTTP/web module only and
re-scores where the codebase has moved.

Scoring rubric (unchanged from the prior audit): **0–100 = user value /40 +
measurement-trust impact /20 + effort-inverse /20 + product fit /20.**
Component scores are shown as `V/T/E/F`.

---

## (a) Current state

### a.1 HTTP/1.1 + HTTP/2 probe — `crates/networker-tester/src/runner/http.rs`

- Phase ladder `dns → tcp → tls → http_handshake → ttfb → total` with the timed
  region carefully scoped: TLS config built *before* the handshake timer
  (`http.rs:436-529`), the H1/H2 connection handshake split out as
  `http_handshake_ms` (`http.rs:709`, `http.rs:762`) so throughput windows can
  exclude setup.
- **TTFB semantics:** `ttfb_ms` = `send_request()` start → response headers
  (`http.rs:719-730`). For uploads this deliberately spans body write + server
  drain (documented in `throughput.rs:25-55`). Note this is *not* the browser
  definition of TTFB (`responseStart − navigationStart`, which includes
  DNS/connect/TLS and redirects); the two are stored under the same name in
  `HttpResult.ttfb_ms` vs `BrowserResult.ttfb_ms` (`metrics.rs:1934`,
  `metrics.rs:2352`) — a documented-nowhere semantic divergence.
- Post-transfer kernel TCP stats via dup(2)-fd `TCP_INFO` (`SocketProbe`,
  `http.rs:330-336`, `http.rs:604-607`) → `HttpResult.socket_stats`
  (`metrics.rs:1968-1973`): cwnd, retrans, delivery-rate, min-RTT, CC algorithm.
  Prior-audit gap #5 is **closed** for h1/h2/pageload (`pageload.rs:518-521`,
  `pageload.rs:886-888`).
- `Server-Timing` parsing incl. LagHound `app;dur` → network-vs-server split
  with anomaly flag and clock-skew estimate (`http.rs:1020-1090`).
- Redirects **observed, never followed** — a 3xx with `Location` counts as one
  redirect (`http.rs:1004-1012`); no hop timing exists anywhere in the native
  path.
- Content metadata captured but not acted on: `content_encoding` +
  `content_length_header` (`http.rs:991-1002`). Crucially the probe **sends no
  `Accept-Encoding` header** (`build_request`, `http.rs:880-886`), so a
  well-behaved origin will answer identity — `content_encoding` will almost
  always be `None` and compression is effectively *unmeasured*, not just
  un-analyzed (the prior audit's gap #9 "capture headers" closed only the
  reporting half).
- `security_headers` exists on `HttpResult` but is populated **only** in the
  URL-diagnostic path (`url_diagnostic.rs:342`, `target_runner.rs:911`); plain
  probe modes always emit `None` (`http.rs:982`).
- Error taxonomy is structural (source-chain downcast, `http.rs:1371-1412`) —
  but the walk stops at io/rustls/Elapsed. It does **not** downcast
  `h2::Error`, so `GOAWAY`, `REFUSED_STREAM`, `ENHANCE_YOUR_CALM` etc. all
  collapse into a stringly `ErrorCategory::Http`.
- Upload payload is seeded-PRNG incompressible (`http.rs:800-850`) — correct
  per iperf3 practice.

### a.2 HTTP/3 — `runner/http3.rs`, `pageload.rs:1373-1783`

- QUIC handshake / stream / TTFB ladder with a single per-probe deadline
  (`http3.rs:388-394`), structural error classification (`http3.rs:163-194`),
  DNS parity with h1/h2 (`http3.rs:203-240`).
- TLS 1.3 resumption + 0-RTT measured via a follow-up connection inside the
  attempt (`measure_quic_resumption`, `http3.rs:275-352`) — real early data on
  the wire, acceptance = `ZeroRttAccepted`. Prior-audit gap #3 is **closed**.
- **`quinn::Connection::stats()` is never called** (verified: no `stats()` /
  `ConnectionStats` reference in `http3.rs` or `pageload.rs`). H3 attempts
  carry `socket_stats: None` and pageload3 explicitly writes
  `per_connection_socket_stats: vec![]` with the comment "QUIC runs over UDP —
  no TCP socket" (`pageload.rs:1752-1753`). True, but quinn exposes the QUIC
  equivalent (see gap G1) — the h1/h2-vs-h3 comparison is currently asymmetric:
  a retransmission-riddled h3 run is indistinguishable from a clean one.

### a.3 curl cross-validation probe — `runner/curl.rs`

- `--write-out` ladder (`curl.rs:23-24`): namelookup/connect/appconnect/
  starttransfer/total, code, size, negotiated version, redirects; cumulative
  values correctly differenced (`curl.rs:166`, `curl.rs:205-209`, `curl.rs:248`).
- Not requested from curl though available: `%{remote_ip}` (so
  `resolved_ips` is empty, `curl.rs:149`), `%{time_redirect}`,
  `%{num_connects}`, `%{size_header}`, `%{time_posttransfer}`. TLS
  version/cipher hardcoded "unknown" (`curl.rs:212-213`).

### a.4 Throughput modes — `runner/throughput.rs`

- Direction-correct windows: download = `total − ttfb − http_handshake`
  (`throughput.rs:605-614`), upload = `max(server recv;dur, ttfb)`
  (`throughput.rs:649-664`), webupload plausibility cap at 100 GB/s
  (`throughput.rs:681-699`). Download/upload byte verification fails the
  attempt on mismatch (`throughput.rs:96-127`, `throughput.rs:622-641`).
  Goodput includes DNS+TCP+TLS overhead (`throughput.rs:135-148`).
- **Single connection, single transfer, one number.** No multi-connection
  aggregation, no time-sliced samples, no ramp-up (slow-start) exclusion: the
  transfer window starts at the first body byte, so on short transfers the
  average is dominated by congestion-window growth. Ookla-class tools use
  multiple parallel connections and adaptively discard the ramp; ndt7
  deliberately measures single-connection fair share — we implement only the
  ndt7-style measurement while implicitly promising the Ookla-style "link
  capacity" number.

### a.5 Native page-load — `runner/pageload.rs`

- pageload1: up to 6 keep-alive H1.1 connections, round-robin assignment
  (`pageload.rs:270-340`); pageload2: one H2 connection, all assets truly
  concurrent (`pageload.rs:1123-1166`); pageload3: one QUIC connection, all
  streams concurrent (`pageload.rs:1676-1716`). Per-connection TLS cost and
  post-transfer TCP stats captured (`pageload.rs:398-431`). Warm/cold
  connection-reuse comparison (`warmup_pageload2`, `pageload.rs:1831+`).
- Trust nits found in this pass:
  1. **HTTP 404 counts as "fetched"** — asset success test is `status < 500`
     (`pageload.rs:390`, `pageload.rs:1173`, `pageload.rs:1722`). A page whose
     assets all 404 (zero real bytes) reports `success: true` with tiny
     `total_bytes`.
  2. **`asset_timings_ms` misalignment in pageload2/3** — pageload1 indexes by
     original asset id (`pageload.rs:391`), but pageload2/3 `push()` only
     successful fetches (`pageload.rs:1170-1178`, `pageload.rs:1718-1728`), so
     on any failure the vector no longer corresponds to
     `PageLoadResult.asset_timings_ms`'s documented per-asset meaning
     (`metrics.rs:2311-2312`).
  3. **No per-asset TTFB vs download split** — per-asset data is a single
     `elapsed` (queue+TTFB+body). H2/H3 HOL-blocking or prioritization effects
     are invisible; only the total spread hints at them.
  4. **Actually-achieved concurrency is not recorded** — pageload2 fires all N
     streams, but the effective in-flight count (bounded by the server's
     `SETTINGS_MAX_CONCURRENT_STREAMS`, which hyper does not expose) is never
     observed.

### a.6 Real browser — `runner/browser.rs` (chromiumoxide 0.9, `Cargo.toml:47`)

- Sophisticated protocol forcing: browser1 via plain-HTTP URL rewrite
  (`browser.rs:154-173`), browser2 `--disable-quic`, browser3 via SPKI-pin +
  `--origin-to-force-quic-on` + Alt-Svc warm-up + restored background
  networking (`browser.rs:313-447`, `browser.rs:579-595`). Per-run isolated
  profile (`browser.rs:283-298`).
- Measured: `loadEventEnd`, `DOMContentLoaded`, `responseStart` via legacy
  `window.performance.timing` JS evaluation (`browser.rs:659-680`), resource
  count + per-protocol mix + bytes from `Network.responseReceived` events
  (`browser.rs:697-739`).
- Gaps/trust items in this pass:
  1. **No Core Web Vitals.** No LCP, CLS, FCP, TBT — the metrics the entire
     web-performance industry standardizes on. `BrowserResult`
     (`metrics.rs:2346-2362`) has 8 fields; a Lighthouse/WebPageTest-class run
     has none of its headline numbers in common with ours except load/DCL/TTFB.
  2. **The waterfall is received and thrown away.** `EventResponseReceived`
     already carries `response.timing` (CDP `Network.ResourceTiming`:
     dns/connect/ssl/sendStart/sendEnd/receiveHeadersEnd), `remoteIPAddress`,
     `connectionId`, `connectionReused`, `fromDiskCache` — the code reads only
     `protocol` and `content-length` (`browser.rs:706-731`).
  3. **Byte accounting:** comment at `browser.rs:599-601` says bytes come from
     JS `performance.getEntriesByType('resource')`; the code actually sums
     `content-length` headers (`browser.rs:722-731`). Sum excludes headers,
     breaks on chunked/no-CL responses, and measures *decoded-declared* not
     wire bytes. CDP `Network.loadingFinished.encodedDataLength` is the
     canonical wire-bytes source and is not subscribed.
  4. `success: load_ms > 0.0` (`browser.rs:758`) — timing-extraction failure
     produces `success: false` with `error: None` (silent failure shape).
  5. Uses deprecated `performance.timing` instead of
     `PerformanceNavigationTiming` (works today, deprecated for years).

### a.7 WebSocket — `runner/websocket.rs`

Upgrade RTT + status, seq-matched echo RTT/loss/jitter (`websocket.rs:452-514`),
same TLS trust rules as other probes. Solid for its scope; steady-state
long-hold measurement (ping/pong keepalive drift, mid-life latency shift) is
out of scope here and was scored in the prior audit.

### a.8 Delta vs the 2026-07-24 audit

Closed since: rpm/bufferbloat (#2 → `runner/rpm.rs`), QUIC 0-RTT (#3),
TCP-stats plumbing (#5), websocket (#10), pmtud/path/ping/dualstack modes,
security-header capture (#14, URL-diagnostic path only). Partially closed:
content-encoding (#9 — captured, never negotiated, never analyzed). The
browser-waterfall item (P3, 45) is re-scored up in G3 because the discovery
that the data is already inside events we subscribe to collapses the effort
term.

---

## (b) Professional gaps

### G1 — QUIC transport stats via `quinn::Connection::stats()` — **Score 88** (V33 T18 E18 F19)

**What:** After each h3 transfer (http3 probe, download3/upload3, pageload3),
sample `Connection::stats()` and persist a `QuicStats` struct: `PathStats`
(`rtt`, `cwnd`, `congestion_events`, `lost_packets`, `lost_bytes`,
`sent_packets`, `current_mtu`, `black_holes_detected`, PLPMTUD probe counts)
plus `udp_tx/udp_rx` (datagrams, bytes, transmits) and `frame_tx/frame_rx`
per-frame-type counts — the QUIC-side mirror of the dup-fd `TCP_INFO` work
already shipped for TCP modes.

**Why:** This is the single biggest asymmetry in the h1/h2/h3 head-to-head
story the product sells. Today a lossy h3 path and a clean one produce
identical result rows; on the TCP side the same situation is fully explained
(retrans, cwnd, delivery-rate). `frame_rx` also directly answers protocol
questions we currently can't: did the server send `MAX_STREAMS` pressure, how
many `ACK`/`PING`/`RESET_STREAM` frames, was connection migration attempted.

**Implementation path (verified):** `quinn::Connection` is a cheap clonable
handle; clone it before `h3::client::new(QuinnH3Connection::new(conn))`
consumes it (`http3.rs:481`, `pageload.rs:1549`), call `.stats()` after the
body drain, before endpoint drop. Struct fields confirmed against quinn 0.11
docs ([PathStats](https://docs.rs/quinn/latest/quinn/struct.PathStats.html),
[ConnectionStats](https://docs.rs/quinn-proto/latest/quinn_proto/struct.ConnectionStats.html)).
Additive serde fields; schema stays 1.0. Also fill
`per_connection_socket_stats`-equivalent for pageload3.

### G2 — Core Web Vitals in the browser probe (LCP, CLS, FCP, TBT) — **Score 82** (V34 T13 E16 F19)

**What:** Extend `BrowserResult` with `lcp_ms`, `cls_score`, `fcp_ms`,
`tbt_ms`, `lcp_element` (tag/URL of LCP candidate), `long_task_count`.

**Why:** LCP/CLS/FCP are the lingua franca of web performance
([web.dev/lcp](https://web.dev/articles/lcp)); a "real browser probe" that
reports load/DCL only reads as a 2015-era tool to the target audience
(SRE/perf engineers comparing against Lighthouse/WebPageTest/CrUX). TBT is
the accepted lab proxy where INP cannot be measured
([DebugBear TBT](https://www.debugbear.com/docs/metrics/total-blocking-time)).
On the synthetic `/browser-page` these become *network-attributable* vitals
(same page, protocol varied) — an angle Lighthouse itself cannot offer.

**Implementation path (verified):** chromiumoxide exposes
`Page::evaluate_on_new_document` (CDP `Page.addScriptToEvaluateOnNewDocument`)
— the same mechanism ModPageSpeed uses to avoid the observer/lifecycle race
([headless LCP/CLS write-up](https://modpagespeed.com/blog/headless-lcp-cls-measurement/)).
Inject one script registering buffered `PerformanceObserver`s for
`largest-contentful-paint`, `layout-shift` (sum entries without
`hadRecentInput`), `paint`, and `longtask` (TBT = Σ max(0, duration−50) between
FCP and TTI-ish cutoff = load + quiet window); stash results on
`window.__networkerVitals`; read them after `wait_for_navigation()` + the
existing 500 ms drain (`browser.rs:703`). Puppeteer equivalents are
well-established ([puppeteer-webperf](https://github.com/addyosmani/puppeteer-webperf),
[Addy Osmani's LCP gist](https://gist.github.com/addyosmani/c053f68aead473d7585b45c9e8dce31e)).
Caveats to encode honestly: headless viewport must be fixed (CLS/LCP depend on
viewport); report `None` when zero entries observed rather than 0.0.

### G3 — CDP resource waterfall + correct byte accounting — **Score 80** (V30 T17 E15 F18)

**What:** Per-resource records in `BrowserResult`: URL (trimmed), request
priority, `connectionId` + `connectionReused`, protocol, status,
`fromDiskCache`, wire bytes (`encodedDataLength` from
`Network.loadingFinished`), and the phase breakdown from
`response.timing` (queued→sendStart = queue/stall, sendEnd→receiveHeadersEnd
= TTFB, headers→finished = download) — i.e., the DevTools waterfall phases
([Network reference](https://developer.chrome.com/docs/devtools/network/reference)).
Derive two headline aggregates: max concurrent requests per connection
(actual h2/h3 multiplexing achieved — closes the "stream concurrency actually
used" question at the browser layer) and count of stalled-time >X ms
(connection-limit / HOL evidence).

**Why:** (1) It converts the browser probe from a scoreboard into a
diagnostic — "which phase of which resource" is the whole value of
WebPageTest-class tooling. (2) It *fixes a live trust defect*: today's
`transferred_bytes` is a `content-length` sum (`browser.rs:722-731`) that
contradicts its own comment, drops chunked responses, and ignores header
bytes; `encodedDataLength` is the wire truth and works for h1/h2/h3.

**Implementation path (verified):** all data comes from three event listeners
chromiumoxide already generates (`EventRequestWillBeSent` for priority +
timestamps, `EventResponseReceived` — *already subscribed*, `browser.rs:602` —
for `timing`/`connectionId`/`fromDiskCache`, `EventLoadingFinished` for
`encodedDataLength`). Correlate by `request_id`. Cap the per-resource vector
(e.g. 500) for third-party URL-probe pages. Prior audit scored this P3/45 on
assumed effort; the effort term collapses because no new subscription
machinery is needed.

### G4 — Multi-connection (segmented) throughput mode — **Score 76** (V32 T14 E13 F17)

**What:** New `downloadmulti`/`uploadmulti` modes (or `--connections N` on
existing throughput modes): N parallel connections each running the existing
download/upload machinery against `/download?bytes=…`, aggregated as
Σbytes / wall-window, plus per-100 ms throughput samples so ramp
(slow-start) can be excluded and a steady-state figure reported alongside the
whole-transfer average. Report both `single_conn_mbps` (existing) and
`aggregate_mbps` — they answer different questions.

**Why:** The literature is explicit that this is *the* methodological split
among speed tests: ndt7 = one TCP connection = fair-share; Ookla = adaptive
multi-connection = link capacity, and Ookla reads consistently higher on
high-latency/lossy paths ([SIGMETRICS '23 comparative
study](https://dl.acm.org/doi/10.1145/3579448)). Our single-connection number
under-reports achievable capacity exactly where users investigate ("VM says
200 Mbps, provider says 1 Gbps") — on high-BDP paths a single cubic flow
can't fill the pipe in a 1-GiB transfer. Per-connection `socket_stats`
(already plumbed) will show the per-flow cwnd story, making the
single-vs-multi delta *explainable*, which competitors don't do.

**Implementation path:** compose `run_probe` tasks in a `JoinSet` (pattern
already exists in `pageload.rs:309-340`); byte counting must move to a shared
atomic sampled by a ticker for the time-sliced series (pattern exists in
`rpm.rs` load generator). Endpoint changes: none. Protocol-variant checklist
in CLAUDE.md applies (metrics/dispatch/summary/modes.json).

### G5 — Redirect chain with per-hop timing (URL-probe path) — **Score 72** (V28 T14 E15 F15)

**What:** In the URL-diagnostic/urlprobe path only, follow up to N (default 5,
same-scheme-or-upgrade only) redirects, recording per hop: URL, status,
scheme change (http→https), full phase ladder (DNS/TCP/TLS/TTFB — new host =
new handshake), and cumulative time-to-final-200. Keep plain probe modes
observed-not-followed (correct for repeatable measurement).

**Why:** Redirects are the most common hidden latency tax on real URLs
(www→apex, http→https, geo redirects: 1–3 extra RTT-and-handshake rounds
before byte one). Every professional tool (curl `-L` + `%{time_redirect}`,
WebPageTest, Lighthouse) itemizes hops; we count at most "1, unfollowed"
(`http.rs:1004-1012`). Also unlocks the HSTS/upgrade-behavior story the
security-header audit hints at.

**Implementation path:** loop over the existing single-shot probe (each hop is
just `run_http_or_tcp` at a new URL — machinery unchanged), collect
`Vec<RedirectHop>` into the URL-diagnostic result (`url_diagnostic.rs`
already has its own result envelope). Loop-detection by URL set; cap total
budget with the existing per-probe timeout.

### G6 — h2/h3 failure semantics: GOAWAY / stream-error / error-code surfacing — **Score 67** (V22 T17 E15 F13)

**What:** In `classify_request_error` (`http.rs:1371`), downcast the chain to
`h2::Error`: `is_go_away()`, `is_reset()`, and `reason()` (REFUSED_STREAM,
ENHANCE_YOUR_CALM, PROTOCOL_ERROR…) → structured `error.detail` (e.g.
`h2_goaway:ENHANCE_YOUR_CALM`) and a correct category (a GOAWAY-with-no-error
during graceful shutdown is not the same failure as a stream reset). Same for
h3: `h3::Error` exposes the H3 error code (H3_REQUEST_REJECTED etc.) in the
h3 probe's error path (`http3.rs` maps everything to a formatted string
today).

**Why:** Load-shedding servers and intermediaries speak *through* these codes
(REFUSED_STREAM = retryable, ENHANCE_YOUR_CALM = rate-limited, GOAWAY =
connection churn). For a measurement product, "HTTP error" that is actually a
mid-flight GOAWAY misattributes server behavior to the network. Low effort,
pure trust win; no new wire work.

**Implementation path:** hyper keeps the `h2::Error` in the source chain
(hyper `Error::source()`); the crate is already in the dependency graph via
hyper's `http2` feature — add `h2` as a direct dep to name the type. Tests:
in-process hyper server issuing GOAWAY/reset.

### G7 — Conditional-request / cache-revalidation timing (ETag → 304) — **Score 64** (V24 T10 E16 F14)

**What:** A `revalidate` follow-up inside the URL-probe (mirroring the
`tlsresume`/0-RTT idiom, `http3.rs:256-274`): request once, capture
`ETag`/`Last-Modified`/`Cache-Control`, then re-request with
`If-None-Match`/`If-Modified-Since` on the same connection and on a fresh
connection. Report: `revalidation_supported` (got 304?), `full_ms` vs
`revalidate_ms`, bytes saved, and whether the origin ignores validators
(returns 200 + full body — a real-world misconfiguration).

**Why:** The 304 path is what repeat visitors actually experience; the ratio
`revalidate_ms / full_ms` and "validators ignored" verdicts are standard CDN/
origin health checks that no current mode covers. Cheap: pure composition of
the existing probe with two extra headers.

**Implementation path:** additive fields in the URL-diagnostic result;
`build_request` grows optional conditional headers. Guard: only for 2xx firsts
with a validator header present.

### G8 — Compression effectiveness measurement — **Score 62** (V24 T10 E13 F15)

**What:** Two-fetch comparison in the URL-probe: fetch with
`Accept-Encoding: gzip, br, zstd` and with `identity`, report negotiated
encoding, compressed vs identity bytes (ratio), time delta, and per
`content-type` when combined with G3 browser data. Optionally decode lengths
only (no decompression needed: identity fetch gives the uncompressed size).

**Why:** As noted in a.1, we currently *never negotiate* compression, so the
shipped `content_encoding` field is dormant — the prior audit's gap #9 was
marked closed but the measurement half never landed. "Your origin serves 1.8
MB of JS uncompressed" is a top-3 finding in any professional page audit.

**Implementation path:** flag on `RunConfig` (`accept_encoding:
Option<String>`), second fetch composed in `url_diagnostic.rs`; keep probe
modes identity-only by default so historical throughput comparability is
preserved (reporting must not change the measurement — the codebase's own
rule, `http.rs:952-954`).

### G9 — Wire-overhead accounting for h1/h2 (counting IO wrapper) — **Score 59** (V20 T14 E12 F13)

**What:** Wrap the `Box<dyn IoStream>` (`http.rs:1465-1467`) in a
byte-counting `AsyncRead/AsyncWrite` shim; report `wire_bytes_tx/rx` per
attempt. Derived: protocol efficiency = body_bytes / wire_rx (captures TLS
record + framing + header-compression overhead in aggregate); on pageload2 vs
pageload1 the delta approximates HPACK's benefit empirically.

**Why:** Exact HPACK/QPACK table accounting is not exposed by hyper/h2/h3
(see rejected R4) — but total wire bytes is the honest, implementable
approximation, and it also strengthens throughput trust (goodput vs wire
throughput distinction speedtests gloss over). Counting at the TLS-cleartext
boundary vs TCP boundary should be documented (wrapping under TLS counts
ciphertext+records; wrapping the h1/h2 side counts framing only — do the
former for the "wire" claim).

**Implementation path:** ~80-line wrapper with `Arc<AtomicU64>` pair, threaded
through `send_http1/send_http2`; additive fields on `HttpResult`. QUIC side
already covered by G1's `udp_tx/udp_rx` — consistent story across protocols.

### G10 — Early Hints (103) observation — **Score 54** (V20 T8 E14 F12)

**What:** Register hyper's client 1xx callback
([`hyper::ext::on_informational`](https://docs.rs/hyper/latest/hyper/ext/index.html))
to record: 103 received (bool), `t_103_ms` vs `ttfb_ms` (the head-start an
early-hints-aware client would get), and the `Link` preload targets. Report in
the URL-probe.

**Why:** Early Hints is the industry's post-server-push resource-hint
mechanism ([MDN 103](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/103));
CDNs (Cloudflare, Fastly) ship it and perf teams want to verify it fires and
how early. Honest caveats keep the score moderate: hyper's hook is
**HTTP/1-only on the client**, while real-world 103 is mostly deployed over
h2/h3 — so native coverage is partial; browsers exercise it internally but CDP
does not surface a distinct 103 timing event. Ship as "103 observed over
HTTP/1.1" + browser-side indirect evidence (preconnect before main response in
the G3 waterfall), and label the limitation.

**Implementation path:** one `req.extensions_mut()` hook in `send_http1`
(`http.rs:692`); additive `early_hints` field. Endpoint work optional (emit a
demo 103 on `/browser-page` for E2E tests).

### G11 — curl write-out enrichment — **Score 55** (V14 T13 E18 F10)

**What:** Extend `WRITE_OUT` (`curl.rs:23-24`) with `%{remote_ip}` (fills the
empty `resolved_ips`), `%{scheme}`, `%{num_connects}`, `%{size_header}`,
`%{time_redirect}`, and (curl ≥ 8.6) `%{time_queue}`; parse defensively for
old curl (existing "unknown" honesty pattern, `curl.rs:370-379`).

**Why:** The curl probe's purpose is cross-validation of our native stack; the
richer the overlap (remote IP actually connected to, header bytes), the more
disagreements it can catch (e.g. DNS answer divergence between hickory and
libcurl). Nearly free.

### G12 — Trust micro-fixes (bundle) — **Score 74** (V18 T20 E20 F16)

Small defects found in this audit, each cheap and each a correctness matter:

1. Page-load asset success should be `status < 400` (not `< 500`) — align
   with the unified probe rule V6 (`http.rs:610-611`); today 404 assets count
   as fetched (`pageload.rs:390`, `1173`, `1722`).
2. Fix `asset_timings_ms` index alignment in pageload2/3 (carry the asset
   index through the future, as pageload1 does).
3. Browser `transferred_bytes`: sum `Network.loadingFinished.encodedDataLength`
   (subsumed by G3; fix the stale comment either way, `browser.rs:599-601` vs
   `722-731`).
4. Browser failure shape: when nav succeeds but timing extraction fails,
   attach an `ErrorRecord` instead of `success:false, error:None`
   (`browser.rs:751-786`).
5. Record per-asset TTFB vs body time in pageload probes (split at
   `send_request` resolution — the data is already in scope,
   `pageload.rs:739-752`).
6. Document the two TTFB definitions (`HttpResult.ttfb_ms` = request→first
   byte; `BrowserResult.ttfb_ms` = navigation→responseStart) in `metrics.rs`
   doc comments and reports, so cross-mode comparisons aren't apples-to-pears.

---

## (c) Considered and REJECTED

| # | Idea | Why rejected |
|---|---|---|
| R1 | **HTTP/2 server-push measurement** | Push was disabled in Chrome 106 and removed from the ecosystem (nginx removed it; ~1.25% adoption at peak) — a dead metric. The successor mechanisms are RFC 9218 priorities and 103 Early Hints (G10). Sources: [Chrome removal blog](https://developer.chrome.com/blog/removing-push), [RFC 9218](https://www.rfc-editor.org/rfc/rfc9218.html). |
| R2 | **INP headlessly** | INP is field-only by definition — it requires real user interactions; scripted clicks measure the script, not the user. Lighthouse itself does not report INP in lab runs and uses TBT as the correlated proxy ([DebugBear](https://www.debugbear.com/docs/metrics/total-blocking-time), [unlighthouse](https://unlighthouse.dev/glossary/tbt)). TBT is included in G2 instead. |
| R3 | **Speed Index** | Requires trace screencap frames + visual-progress computation (Lighthouse shells out to the speedline module; [Chrome docs](https://developer.chrome.com/docs/lighthouse/performance/speed-index)). In Rust that means Tracing-domain screenshot capture + histogram diffing — heavy, fragile, and low-signal on our synthetic `/browser-page` whose visual progress is trivial. Revisit only if URL-probe of arbitrary pages becomes the flagship. |
| R4 | **Exact HPACK/QPACK overhead accounting** | Neither hyper, `h2`, nor `h3` expose encoder/decoder table state or per-header compressed sizes; measuring exactly would mean a custom codec or pcap+decrypt. The empirical wire-bytes approximation (G9) captures the user-visible effect at 5% of the cost. |
| R5 | **RFC 9218 prioritization behavior testing** | hyper's client API exposes no per-stream priority signaling, our endpoint (axum/hyper server) implements no prioritization scheduler, and real-world server compliance is famously inconsistent. Meaningful testing requires a purpose-built server harness — out of scope for a probe. Priority *observation* (what Chrome requested) is covered cheaply in G3. |
| R6 | **HOL-blocking lab (induced loss, h1/h2/h3 under netem)** | Requires impairment injection (tc/netem, privileged, Linux-only) between probe and endpoint — that's a testbed feature, not a field measurement; a field probe cannot ethically induce loss on production paths. The measurable field signal (per-asset stall spread across one connection vs many) falls out of G3 + a.5-item-3 instead. |
| R7 | **Full Lighthouse embedding (run Lighthouse via node)** | Would bring the metrics wholesale but adds a Node.js runtime dependency to tester VMs, ~10× probe latency, and no protocol-forcing control (our browser1/2/3 differentiator). Native CDP collection (G2/G3) keeps the footprint and the differentiation. |
| R8 | **Cross-attempt QUIC/TLS session cache for "warm" h3 attempts** | Explicitly rejected in-code for good reason: attempts are architecturally stateless and a shared ticket cache would silently warm attempts 2..n, skewing the h1/h2/h3 comparison (`http3.rs:256-266`). The in-attempt follow-up idiom already covers resumption measurement. |

---

## (d) Top-5 shortlist

| Rank | Gap | Score | One-line pitch |
|---|---|---|---|
| 1 | **G1 QUIC stats** (`Connection::stats()`) | 88 | Symmetry with TCP_INFO: makes every h3 anomaly explainable; ~an afternoon of plumbing. |
| 2 | **G2 Core Web Vitals** (LCP/CLS/FCP/TBT) | 82 | The industry's headline metrics, network-attributable via our protocol forcing; verified feasible with chromiumoxide `evaluate_on_new_document` + buffered PerformanceObserver. |
| 3 | **G3 CDP waterfall + wire bytes** | 80 | The data already arrives in events we subscribe to; also fixes the `transferred_bytes` trust defect and reveals actual h2/h3 concurrency used. |
| 4 | **G4 Multi-connection throughput** | 76 | Closes the ndt7-vs-Ookla methodology gap: report fair-share *and* link capacity, with per-flow TCP stats to explain the delta. |
| 5 | **G12 Trust micro-fixes** | 74 | 404≠fetched, asset-timing alignment, browser failure shape, TTFB semantics — cheap correctness before new features. |

G5 (redirect hops) is the best next-in-line and arguably belongs in the same
PR series as the URL-probe work; G6 (h2 GOAWAY downcast) is the best
"one-evening" item.

---

## Sources

- quinn 0.11 [`PathStats`](https://docs.rs/quinn/latest/quinn/struct.PathStats.html) and [`ConnectionStats`](https://docs.rs/quinn-proto/latest/quinn_proto/struct.ConnectionStats.html) (docs.rs)
- [hyper `ext` module — `on_informational` (client 1xx callbacks)](https://docs.rs/hyper/latest/hyper/ext/index.html)
- [MDN — 103 Early Hints](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/103)
- [Chrome for Developers — Removing HTTP/2 Server Push](https://developer.chrome.com/blog/removing-push); [RFC 9218 — Extensible Prioritization Scheme for HTTP](https://www.rfc-editor.org/rfc/rfc9218.html)
- [web.dev — Largest Contentful Paint](https://web.dev/articles/lcp); [ModPageSpeed — Measuring LCP and CLS in a headless browser](https://modpagespeed.com/blog/headless-lcp-cls-measurement/); [addyosmani/puppeteer-webperf](https://github.com/addyosmani/puppeteer-webperf); [LCP-in-Puppeteer gist](https://gist.github.com/addyosmani/c053f68aead473d7585b45c9e8dce31e)
- [DebugBear — Total Blocking Time](https://www.debugbear.com/docs/metrics/total-blocking-time); [unlighthouse — TBT](https://unlighthouse.dev/glossary/tbt); [Chrome docs — Speed Index](https://developer.chrome.com/docs/lighthouse/performance/speed-index)
- [Comparative Analysis of Ookla Speedtest and NDT7 (ACM SIGMETRICS 2023)](https://dl.acm.org/doi/10.1145/3579448) ([arXiv preprint](https://arxiv.org/pdf/2205.12376)); [Cloudflare AIM docs](https://developers.cloudflare.com/speed/aim/)
- [Chrome DevTools — Network features reference (waterfall phases)](https://developer.chrome.com/docs/devtools/network/reference); [DebugBear — DevTools Network tab](https://www.debugbear.com/blog/devtools-network)
- [chromiumoxide (docs.rs)](https://docs.rs/chromiumoxide) / [GitHub](https://github.com/mattsse/chromiumoxide)

*Method: full-source read of `runner/{http,curl,throughput,pageload,browser,websocket,http3,rpm}.rs` + `metrics.rs` result structs (v0.28.x tree, 2026-07-27), reconciled against the 2026-07-24 gap analysis; external capability claims (quinn stats fields, hyper 1xx hook, chromiumoxide CDP surface, CWV lab-measurability, speed-test methodology) verified via the cited sources before scoring.*
