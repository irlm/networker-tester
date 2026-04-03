# Application Benchmark Mode Design

**Date:** 2026-04-03
**Status:** Approved

## Overview

Add a new "Application" benchmark type alongside the existing "Full Stack" mode. Application mode measures real-world API performance by placing a reverse proxy (nginx, IIS, Caddy, Traefik, HAProxy, Apache) in front of each language server and using Chrome as the client — simulating how APIs are actually deployed and consumed.

## Motivation

Full Stack mode tests raw language/protocol performance (language handles TLS, HTTP directly). This is useful for stack comparison but doesn't reflect production reality where:

- APIs sit behind reverse proxies
- Browsers (not raw HTTP clients) make the requests
- Multiple concurrent API calls happen per page load
- HTTP/2 multiplexing and HTTP/3 QUIC gains only materialize with connection reuse

Application mode answers: "How fast is my Java API behind nginx, as experienced by a real browser?"

## Two Benchmark Types

### Full Stack (existing, unchanged)

- Language handles TLS, HTTP/1.1, HTTP/2, HTTP/3 directly on :8443
- Client: networker-tester (raw HTTP)
- No reverse proxy
- HTTP/3 only for languages with native QUIC support (12 of 18)
- Measures: raw network stack + language performance

### Application (new)

- Language runs plain HTTP on localhost:8080 (no TLS)
- Reverse proxy on :8443 handles TLS + HTTP/2 + HTTP/3
- Client: Chrome browser (headful or headless, forced HTTP version)
- ALL 18 languages get full protocol support via the proxy
- Measures: real-world browser-to-API experience through proxy

## Proxy Support

All selected proxies deploy on the **same endpoint VM**. The orchestrator swaps proxies between runs — one active at a time on :8443, language stays running on :8080.

### Proxy Matrix

| Proxy | Linux | Windows | HTTP/3 |
|-------|-------|---------|--------|
| nginx (mainline 1.27+) | yes | yes | yes |
| IIS + ARR | - | yes | yes |
| Caddy | yes | yes | yes |
| Traefik | yes | yes | yes |
| HAProxy | yes | yes | no |
| Apache httpd | yes | yes | no |

HTTP/3 mode is filtered out for proxies without QUIC support (HAProxy, Apache).

## Reference API Endpoints

All 18 language reference APIs add these JSON endpoints **alongside** the existing `/health`, `/download/{size}`, `/upload` endpoints (which remain for Full Stack mode). Each endpoint performs **real per-request computation** (parameters vary per request to prevent caching):

| Endpoint | Work | Response Size |
|----------|------|---------------|
| `GET /api/users?page=N&sort=field&order=asc` | Generate users, sort by requested field, paginate | ~15KB |
| `POST /api/transform` | Validate schema, transform/enrich fields, compute hash | ~2KB |
| `GET /api/aggregate?range=start,end` | Generate time-series, compute avg/p50/p95/max, group by category | ~5KB |
| `GET /api/search?q=term&limit=N` | Build dataset, regex/fuzzy match, score and rank | ~8KB |
| `POST /api/upload/process` | Receive binary payload, compute checksum, compress, return stats | ~500B |
| `GET /api/delayed?ms=N&work=light` | Sleep N ms (simulated I/O wait), then light computation | ~1KB |

### Timing Headers (Server-Timing RFC 7440)

Both the language server and the proxy inject timing into the `Server-Timing` header:

**Language server** adds:
```
Server-Timing: app;dur=12.3
```

**Proxy** adds its own entry (via header injection in proxy config):
```
Server-Timing: app;dur=12.3, proxy;dur=4.8
```

For proxies that support request timing natively:
- **nginx:** `$request_time` / `$upstream_response_time` via `add_header`
- **Caddy:** `reverse_proxy` transport timing
- **Traefik:** access log timing fields, injected via middleware
- **IIS:** ARR timing via server variables
- **HAProxy:** `%Tt` / `%Ta` timing fields
- **Apache:** `%D` (request duration in microseconds)

**Proxy timing normalization:** Different proxies expose timing differently (nginx `$request_time` includes upstream, HAProxy `%Tt` vs `%Ta` differ, Apache `%D` is total only). To ensure valid cross-proxy comparisons, each proxy config is tuned to emit **two raw fields**:

```
Server-Timing: app;dur=12.3, proxy_total;dur=17.1, upstream;dur=12.5
```

Where:
- `app;dur` = language processing time (set by language server)
- `proxy_total;dur` = total time inside the proxy (from request received to response sent)
- `upstream;dur` = time the proxy spent waiting for the upstream language server

The post-processing layer normalizes:
```
Proxy Overhead = proxy_total - upstream
```

This is stored as the canonical `proxy;dur` value, ensuring apples-to-apples comparison regardless of what the proxy natively reports. Raw fields (`proxy_total`, `upstream`) are stored alongside for debugging.

**Proxy timing quality classification:** Not all proxies can emit trustworthy normalized timing. Each result is tagged:
- **`exact`**: proxy emits both `proxy_total` and `upstream` with correct semantics (nginx, HAProxy)
- **`estimated`**: proxy emits total-only timing; `upstream` is approximated from `app;dur` (Apache, some IIS configs)
- **`unavailable`**: proxy cannot inject timing headers at all

When quality is `estimated` or `unavailable`, the results page shows a warning icon on the proxy overhead column. Cross-proxy comparison views include a filter (enabled by default) to exclude `estimated` and `unavailable` proxies from side-by-side comparisons. Users can disable the filter to see all data, but annotations remain visible.

This enables three-layer decomposition:

```
Total Time (browser)  = App Time + Proxy Overhead + Network Time
App Time              = Server-Timing: app;dur (language processing)
Proxy Overhead        = proxy_total - upstream (normalized)
Network Time          = Total - App - Proxy Overhead (TLS + transport + browser)
```

**Network time sub-decomposition** (stored as raw browser timing fields, not derived):

| Field | Source | Meaning |
|-------|--------|---------|
| `dns_time` | Resource Timing: `domainLookupEnd - domainLookupStart` | DNS resolution |
| `tcp_time` | Resource Timing: `connectEnd - connectStart` | TCP handshake |
| `tls_time` | Resource Timing: `connectEnd - secureConnectionStart` | TLS handshake |
| `ttfb` | Resource Timing: `responseStart - requestStart` | Time to first byte |
| `download_time` | Resource Timing: `responseEnd - responseStart` | Response body transfer |
| `queue_time` | Resource Timing: `requestStart - fetchStart` | Browser queuing delay |

These raw fields are the source of truth. The derived `Network Time` is a convenience metric; analysis should prefer raw fields when precision matters.

All timing values throughout this spec are stored in **milliseconds (ms)**, with `real` (float) precision.

Parameters are randomized per request (seed, sort field, query term, page number, date range) so each call performs real computation. No caching, no pre-computed results.

### Cache Prevention

Randomized parameters alone are insufficient — proxy-level caching (Caddy, nginx if misconfigured) and browser cache (despite `--disable-cache`) can still interfere. Defense in depth:

1. **Language server:** every response includes:
   - `Cache-Control: no-store, no-cache, must-revalidate`
   - `Pragma: no-cache`
   - `Timing-Allow-Origin: *` (required for cross-origin Resource Timing access)
   - `Access-Control-Allow-Origin: *` (required for cross-origin fetch from test page on localhost:3333)
   - These four headers are **mandatory** for all 18 language implementations and all proxy configs must pass them through unmodified
   - **Note:** Current benchmark mode does not use credentials (no cookies, no Authorization header). If authenticated requests are added in the future, both `Timing-Allow-Origin` and `Access-Control-Allow-Origin` must switch from `*` to an explicit origin (e.g., `http://localhost:3333`), since `*` is incompatible with `credentials: include`
2. **Proxy config:** each proxy is configured to pass cache headers through without overriding, and to not cache upstream responses
3. **Chrome:** `--disable-cache` flag + `--disk-cache-size=0`
4. **Request uniqueness:** each request includes a monotonic `_t=timestamp` query parameter as final defense

### Workload Standardization

To ensure fair cross-language comparison, each endpoint has a **strict algorithmic contract**:

- `/api/users`: generate N user structs from deterministic PRNG (seed = request param), sort using language's standard library sort, paginate to 20 items
- `/api/transform`: parse JSON body, SHA-256 hash each string field, reverse arrays, return transformed object
- `/api/aggregate`: generate 10,000 time-series points from seed, compute mean/p50/p95/max, group into 5 categories
- `/api/search`: generate 1,000 string items from seed, apply regex filter, score by match position, return top N
- `/api/upload/process`: read full body, compute CRC32 + SHA-256, zlib compress, return sizes + checksums
- `/api/delayed`: sleep for requested ms (simulating DB/network I/O), then light JSON serialization. Tests how well the language's async runtime handles concurrent I/O-bound requests — critical for real API workloads where most time is spent waiting on external services. Contract: `ms` parameter range 1–100 (clamped, not rejected). Sleep MUST be async/non-blocking (e.g., `tokio::time::sleep`, `asyncio.sleep`, `setTimeout`). `work=light` is the only value (fixed — not extensible). Response includes actual sleep duration for validation

A **reference validation endpoint** (`GET /api/validate?seed=42`) returns expected output checksums for all endpoints with that seed, plus `workload_contract_version: 1`. The orchestrator calls this after deployment to verify the language implementation is correct and matches the expected contract version before benchmarking. Results store `workload_contract_version` so future workload changes don't invalidate historical comparisons.

### Error Tracking

Every measurement cycle tracks:
- **Error rate:** failed requests / total requests (HTTP 4xx/5xx, network errors)
- **Timeout rate:** requests exceeding methodology timeout
- **Retry count:** zero (no retries — errors are recorded, not hidden)

If error rate exceeds the configurable `error_threshold_percent` (default: 2%) for a language/proxy combination, the result is flagged and the orchestrator logs a warning. Results page shows error rates alongside timing data.

## Test Harness (Chrome Client)

### Chrome Determinism Flags

Chrome introduces variability (GC, rendering, scheduling, background tasks) that contaminates timing. All Chrome instances launch with deterministic flags:

```
--disable-background-networking
--disable-cache
--disable-extensions
--disable-renderer-backgrounding
--disable-features=PaintHolding
--disable-default-apps
--no-first-run
--metrics-recording-only
```

Additional per-mode:
- **Server (headless):** `--disable-gpu --headless=new`
- **Desktop:** CPU affinity pinned for Chrome process (taskset on Linux, Start-Process -ProcessorAffinity on Windows)

Chrome process is warmed up (navigate to `about:blank`, wait 2s) before measurement begins.

### HTTP Version Forcing

- **HTTP/1.1:** Chrome flag `--disable-http2`
- **HTTP/2:** Chrome default (prefers h2)
- **HTTP/3:** Chrome flags `--enable-quic --origin-to-force-quic-on=host:port`

**Addressing rule:** All benchmark runs use one fixed IPv4 address for the endpoint (no hostname, no IPv6, no DNS). This prevents DNS resolution variability and ensures `--origin-to-force-quic-on` targets a stable address. The orchestrator resolves the endpoint IP once at provisioning and uses it for all subsequent requests.

### Protocol Validation

After each test cycle, the harness validates the actual protocol used via Chrome DevTools Protocol (CDP) `Network.responseReceived` events. If the negotiated protocol doesn't match the forced version (e.g., HTTP/3 silently fell back to HTTP/2), the result is flagged as `protocol_mismatch` and excluded from aggregation. The results page shows these mismatches so users can investigate.

### Connection Control

HTTP/2 and HTTP/3 gains depend on connection reuse. The harness runs two explicit phases:

1. **Cold connections** — fresh Chrome instance per cycle, no connection reuse. Measures full TLS handshake + connection setup cost.
2. **Warm connections** — single Chrome instance, persistent connections across cycles. Measures steady-state multiplexing performance.

Results are tagged `connection_mode: cold | warm` and displayed separately. The JavaScript collector captures per-request `nextHopProtocol` and `connectStart`/`connectEnd` from Resource Timing API to identify new vs reused connections.

**TLS session tracking:** The harness records TLS handshake behavior per request:

- `tls_handshake_ms`: from Resource Timing (`connectEnd - secureConnectionStart`)
- `connection_new`: boolean — `true` if `connectStart > 0` (new connection), `false` if reused
- `protocol_actual`: from CDP `Network.responseReceived` → `protocol` field

Classification into `tls_handshake_type` uses **trustworthy signals only**:
- **`full`**: assigned when `connection_new = true` AND cold connection phase
- **`0rtt`**: assigned only when CDP security details confirm QUIC early data acceptance (HTTP/3 only)
- **`resumed`**: assigned only when CDP/netlog explicitly reports TLS session ticket reuse
- **`unknown`**: default — when no trustworthy signal is available, the raw `tls_handshake_ms` is stored without classification

Do **not** classify based on timing thresholds alone — handshake duration varies by machine, OS, and TLS library. The Connection Analysis view shows handshake type distribution where classification is confident, and raw `tls_handshake_ms` for all requests regardless.

### Traffic Pattern Per Measurement Cycle

A test page fires 10 concurrent API calls simulating a dashboard page load:

- 2x `GET /api/users?page=rand&sort=rand` (CPU: sort + serialize)
- 2x `POST /api/transform` (varied payloads) (CPU: hash + transform)
- 2x `GET /api/aggregate?range=rand` (CPU: stats computation)
- 1x `GET /api/search?q=rand` (CPU: regex + ranking)
- 1x `POST /api/upload/process` (small binary) (CPU: checksum + compress)
- 2x `GET /api/delayed?ms=10&work=light` (I/O: simulated DB wait)

### Definitions

- **Page-load cycle:** the interval from dispatch of the first request in the scenario until completion of all 10 requests in that scenario. Includes browser queuing delay and the parallel request execution window. This is the primary unit of measurement.
- **Throughput:** `throughput_rps = total_requests_completed / total_measured_time_window` where the time window spans all measured cycles (excluding warmup). Cycle-level throughput (`10 / cycle_duration_sec`) is also stored for per-cycle analysis.

### Collection

JavaScript on the test page collects via Performance API + `serverTiming` + Resource Timing:

- Per-request: total time, server time (from `Server-Timing` header), proxy time (from `Server-Timing` proxy entry), network time (derived: total - server - proxy)
- Per-request: connection state (new vs reused), negotiated protocol, TLS handshake time
- Per-cycle: page-load cycle duration (time until all 10 requests complete)
- Per-cycle: error count, timeout count
- Throughput: aggregate and per-cycle (see definitions above)

Repeated for warmup + measured cycles per methodology config.

### Retry Policy

Two distinct policies apply:

- **Infrastructure retries allowed:** VM provisioning, proxy deployment, Chrome launch, health checks — these are setup operations and may be retried with backoff on transient failure.
- **Measurement retries NOT allowed:** benchmark requests during warmup and measured phases are never retried. Failures are recorded as errors in the results. This ensures error rates reflect real behavior, not hidden retries.

### Test Page Serving

The tester VM runs a lightweight local HTTP server (e.g., Python or Node one-liner) that serves the test page on `localhost:3333`. Chrome navigates to this local page, which then makes cross-origin API calls to the endpoint VM's proxy on :8443. This avoids serving the test page from the language server itself (which would add load to the thing being measured).

### Tester VM Options

| Tester Type | OS | Chrome Mode | Simulates |
|-------------|-----|-------------|-----------|
| Server (default) | Ubuntu Server | Headless | API-to-API / automated clients |
| Desktop Linux | Ubuntu Desktop 24.04 | GUI (headful) | Linux developer/user |
| Desktop Windows | Windows 11 | GUI (headful) | Enterprise user |

Desktop VMs simulate real end-user environments: real GPU compositing, real OS network stack, real TLS libraries (Schannel vs OpenSSL). Users can compare the same API from different client OSes.

**Desktop VM modes:**
- **Deterministic desktop** (default): hardened — disable OS updates, search indexing, Cortana, OneDrive, background services, Windows Defender real-time scan. Clean Chrome profile (no extensions, no sync). CPU affinity pinned. Maximizes reproducibility.
- **Real-world desktop**: no hardening — stock OS with default services running. Simulates what a real user's machine looks like. Results are noisier but more honest.

The wizard exposes this as a toggle per tester VM. Results are tagged `desktop_mode: deterministic | realworld`.

The wizard shows estimated VM size per tester type (desktop VMs require more RAM/compute).

## Data Model

### New Tables

**`benchmark_app_configs`** — mirrors `benchmark_configs` for application benchmarks:

| Column | Type | Notes |
|--------|------|-------|
| config_id | UUID PK | |
| project_id | UUID FK | |
| name | text | |
| template | text | nullable |
| status | text | draft / queued / running / completed / failed / cancelled |
| created_by | UUID FK | nullable |
| created_at | timestamptz | |
| started_at | timestamptz | nullable |
| finished_at | timestamptz | nullable |
| config_json | jsonb | methodology, languages |
| error_message | text | nullable |
| max_duration_secs | int | default 14400 |
| baseline_run_id | UUID | nullable |
| worker_id | text | nullable |
| last_heartbeat | timestamptz | nullable |

**`benchmark_app_testbeds`** — extends testbed with proxy selection and tester OS:

| Column | Type | Notes |
|--------|------|-------|
| testbed_id | UUID PK | |
| config_id | UUID FK | |
| cloud | text | |
| region | text | |
| topology | text | |
| vm_size | text | nullable |
| os | text | endpoint OS |
| tester_os | text | server / desktop-linux / desktop-windows |
| endpoint_vm_id | text | nullable |
| tester_vm_id | text | nullable |
| endpoint_ip | text | nullable |
| tester_ip | text | nullable |
| status | text | |
| languages | jsonb | |
| proxies | jsonb | e.g., ["nginx", "caddy", "traefik"] |

### Results Extension

Existing `benchmark_results` table gains:

| Column | Type | Notes |
|--------|------|-------|
| proxy | text | nullable — NULL = full-stack, value = proxy name |
| proxy_version | text | nullable — e.g., "nginx/1.27.2" |
| browser_version | text | nullable — e.g., "Chrome/126.0.6478.61" |
| connection_mode | text | nullable — "cold" or "warm" |
| protocol_actual | text | nullable — negotiated protocol from CDP |
| protocol_mismatch | boolean | default false — forced != actual |
| error_count | int | default 0 |
| timeout_count | int | default 0 |
| os_version | text | nullable — kernel/build version of tester VM |
| dns_ms | real | nullable — Resource Timing: domainLookupEnd - domainLookupStart |
| tcp_ms | real | nullable — Resource Timing: connectEnd - connectStart |
| tls_ms | real | nullable — Resource Timing: connectEnd - secureConnectionStart |
| ttfb_ms | real | nullable — Resource Timing: responseStart - requestStart |
| download_ms | real | nullable — Resource Timing: responseEnd - responseStart |
| queue_ms | real | nullable — Resource Timing: requestStart - fetchStart |
| proxy_total_ms | real | nullable — Server-Timing: proxy_total;dur (raw) |
| upstream_ms | real | nullable — Server-Timing: upstream;dur (raw) |
| app_ms | real | nullable — Server-Timing: app;dur |
| proxy_overhead_ms | real | nullable — derived: proxy_total - upstream (normalized) |
| tls_handshake_type | text | nullable — unknown / full / resumed / 0rtt |
| desktop_mode | text | nullable — deterministic / realworld |
| shared_cores | boolean | default false — true if 2-core VM with no pinning |
| proxy_timing_quality | text | nullable — exact / estimated / unavailable |
| workload_contract_version | int | default 1 |
| measurement_model_version | int | default 1 — bump when timing logic, collection method, or derivation formulas change |

**Normalized dimension tables** (for efficient querying at scale):

**`benchmark_proxies`** — proxy dimension:

| Column | Type |
|--------|------|
| proxy_id | serial PK |
| name | text unique |
| supports_http3 | boolean |

**`benchmark_run_groups`** — groups results from one config execution:

| Column | Type |
|--------|------|
| group_id | UUID PK |
| config_id | UUID FK |
| testbed_id | UUID FK |
| proxy | text |
| language | text |
| http_version | text |
| connection_mode | text |
| started_at | timestamptz |

## Installer Changes

### New Flag

```bash
install.sh --benchmark-server <lang> --benchmark-proxy <proxy>
```

When `--benchmark-proxy` is present:
1. Language server binds **localhost:8080** (plain HTTP, no TLS, no cert generation)
2. Proxy deploys on **:8443** with TLS + HTTP/2 + HTTP/3, reverse-proxying to localhost:8080

### Proxy Deploy Functions

New functions in install.sh:

| Function | Platform | Setup |
|----------|----------|-------|
| `deploy_proxy_nginx` | Linux | mainline repo, `proxy_pass http://127.0.0.1:8080`, `listen 8443 quic` |
| `deploy_proxy_nginx_win` | Windows | nginx Windows build, reverse proxy config |
| `deploy_proxy_iis` | Windows | Enable IIS + ARR + URL Rewrite, HTTP/3 via registry |
| `deploy_proxy_caddy` | Linux/Windows | Single binary, Caddyfile `reverse_proxy localhost:8080` |
| `deploy_proxy_traefik` | Linux/Windows | Single binary, static YAML config |
| `deploy_proxy_haproxy` | Linux/Windows | Package install, frontend/backend config |
| `deploy_proxy_apache` | Linux/Windows | mod_proxy + mod_ssl, VirtualHost config |

### Proxy Swap

```bash
install.sh --benchmark-proxy-swap <proxy>
```

Stops the currently running proxy, deploys and starts the new one on :8443. Language server on :8080 stays running.

**Isolation protocol after swap:**
1. Stop current proxy process
2. Flush connections:
   - **Linux:** `ss -K dport = 8443` (kill connections to proxy port)
   - **Windows:** `netsh int tcp reset` (targeted TCP reset, NOT `netsh int ip reset` which is too broad)
3. Wait for TCP TIME_WAIT drain:
   - **Linux:** check `ss -s | grep timewait` — if count > 100, wait additional 5s (repeat up to 3x)
   - **Windows:** check `netstat -an | findstr TIME_WAIT | find /c /v ""` — same threshold
4. Monitor ephemeral port usage: if available ports < 10,000, log warning and extend stabilization. Track port usage trend across consecutive swaps — if usage grows monotonically toward exhaustion, throttle the benchmark rate (add 10s between language runs) to allow OS recycling
5. On tester VM: restart Chrome to clear QUIC connection cache and TLS session tickets
6. Wait 5-second stabilization window
7. Start new proxy
8. Health check passes before benchmarking begins

Optionally (configurable via methodology): restart language server between proxy swaps for strict isolation (prevents any cross-proxy state leakage in the language runtime).

### Health Check

Hits the proxy on :8443 `/health` endpoint — validates the full proxy-to-language chain is up before benchmarking starts.

## Orchestrator Changes

### Config Extensions

```rust
pub struct DashboardBenchmarkConfig {
    // ... existing fields ...
    pub benchmark_type: String,  // "fullstack" or "application"
}

pub struct TestbedConfig {
    // ... existing fields ...
    pub proxies: Vec<String>,     // e.g., ["nginx", "caddy"]
    pub tester_os: String,        // "server" | "desktop-linux" | "desktop-windows"
}
```

### Execution Planner

The combinatorial matrix (testbeds x proxies x languages x HTTP versions x connection modes) can grow large. The orchestrator includes an execution planner that:

1. **Calculates total combinations** before starting and reports to dashboard
2. **Estimates duration** based on methodology (warmup + measured cycles per combination)
3. **Enforces limits:**
   - `max_combinations`: hard cap per config (default: 500, configurable)
   - `max_duration_secs`: existing field, default 14400 (4 hours)
4. **Supports execution modes:**
   - **Full sweep:** run every combination (default)
   - **Quick mode:** subset — 1 proxy per OS, top 3 languages, warm connections only
   - **Sampling mode:** deterministic random subset of N combinations from the full matrix, controlled by `sampling_seed` (default: config_id hash). Same config = same subset for reproducibility.

The Review step in the wizard shows the combination count and estimated duration, warns if exceeding limits.

### Resource Isolation

On the endpoint VM, proxy and language server are pinned to separate CPU cores using proportional allocation (see CPU Pinning Strategy section below). Tools:

- **Linux:** `taskset` for CPU affinity, optionally `cgroup` for memory limits
- **Windows:** `Start-Process -ProcessorAffinity` / Job Objects

### Execution Flow (Application Mode)

```
For each testbed:
  1. Provision endpoint VM + tester VM (desktop if selected)
  2. Install Chrome on tester VM (with determinism flags)
  3. For each proxy in testbed.proxies:
     a. Deploy proxy on endpoint :8443 (pinned per CPU Pinning Strategy)
     b. For each language in testbed.languages:
        i.   Deploy language on endpoint localhost:8080 (pinned per CPU Pinning Strategy)
        ii.  Validate implementation: GET /api/validate?seed=42
        iii. Health check :8443 (full chain readiness)
        iv.  For each HTTP version (h1, h2, h3 — filtered by proxy capability):
             - Validate protocol via CDP after first request
             - Cold connection phase: fresh Chrome per cycle
             - Warm connection phase: persistent Chrome instance
             - Collect results via Performance API + Server-Timing
        v.   Stop language
     c. Proxy swap isolation protocol (flush connections, wait 5s)
     d. Stop proxy
  4. Report results via callback endpoints (including version metadata)
  5. Teardown (if auto_teardown)
```

## Frontend

### Sidebar

```
Benchmarks
  |- Full Stack    (existing wizard, unchanged)
  |- Application   (new wizard)
```

### Application Wizard Steps

1. **Template** — presets:
   - "Linux API Stack" (nginx + caddy, top 6 languages)
   - "Windows API Stack" (IIS + nginx, .NET variants)
   - "Proxy Comparison" (all OS-compatible proxies, 2-3 languages)
   - "Custom"

2. **Testbeds** — cloud, region, size, endpoint OS, **tester OS** (server/desktop-linux/desktop-windows), **proxy multi-select** (filtered by endpoint OS, at least one required per testbed)

3. **Languages** — same multi-select as full-stack (all 18 available)

4. **Methodology** — adapted for browser testing:
   - Concurrent API calls per page load (default: 10, matching the standard traffic pattern; user-configurable)
   - Page load repetitions (warmup + measured)
   - Target relative error
   - HTTP versions to test (filtered by proxy capability)
   - Connection modes: cold, warm, or both (default: both)
   - Execution mode: full sweep / quick / sampling
   - Strict isolation toggle: restart language server between proxy swaps (default: off)
   - Error threshold % (default: 2%)
   - Sampling seed (for sampling mode reproducibility)

5. **Review** — summary:
   - Total combinations: testbeds x proxies x languages x HTTP versions x connection modes
   - Estimated duration based on methodology preset
   - Warning if exceeding `max_combinations` limit
   - Estimated cloud cost (VM hours x price tier)

### Results Views

**By Language** — how one language performs across all proxies:
```
Python API Performance
  nginx:   Total 45ms | Server 12ms | Network 33ms
  Caddy:   Total 48ms | Server 12ms | Network 36ms
  Traefik: Total 52ms | Server 12ms | Network 40ms
```
Answers: "Which proxy is best for my Python API?"

**By Proxy** — how all languages perform behind one proxy:
```
nginx Proxy Performance
  Rust:    Total 28ms | Server 2ms  | Network 26ms
  Go:      Total 32ms | Server 5ms  | Network 27ms
  Python:  Total 45ms | Server 12ms | Network 33ms
```
Answers: "Which language is fastest behind nginx?"

**Matrix Heatmap** — all proxies x all languages, color-coded by total time. Quick visual for best combinations.

**Time Decomposition** — per result: three-layer breakdown (app / proxy / network) with avg, p50, p95. Shows exactly where the bottleneck is.

**Connection Analysis** — cold vs warm comparison per combination. Shows TLS handshake cost, connection reuse ratio, HTTP/2 multiplexing gain, HTTP/3 0-RTT benefit.

**Error Dashboard** — error rate, timeout rate, protocol mismatches per combination. Flags unreliable results.

**Cross-Mode Comparison** — compare Application results to Full Stack results for the same language/protocol to quantify total proxy overhead.

**Version Metadata** — each result shows proxy version, Chrome version, OS version for reproducibility.

**Measurement Quality Badge** — each result row shows a quality indicator derived from:
- Protocol validated (no mismatch)
- Raw browser timing fields complete (Timing-Allow-Origin worked)
- Proxy timing normalized (exact quality)
- CPU isolation (pinned, not shared)
- Environment (deterministic, not real-world)

Badge levels: "benchmark-grade" (all green), "usable" (minor gaps), "advisory" (significant gaps — interpret with caution).

**Confidence Intervals** — each metric (total, app, proxy, network) includes 95% confidence interval, variance, and standard deviation alongside p50/p95. Computed per-combination across measured page-load cycles (excluding warmup, protocol mismatch runs, and failed requests). Cold and warm phases are aggregated separately. Unit: per page-load cycle (time until all 10 requests complete). Per-request CIs are also available in the detailed drilldown view.

**Browser Version Note** — Chrome version changes can affect HTTP/3 behavior, connection reuse policy, and timing precision. When comparing across runs, results with different `browser_version` values are annotated. For high-confidence regression tracking, pin Chrome to a specific version using the `--chrome-version` flag in the Chromium download step.

**Baseline Drift Detection** — when `baseline_run_id` is set, results page shows % change vs baseline per combination, flagging regressions and improvements automatically. Thresholds are configurable: `baseline_regression_percent` (default: 10%) and `baseline_improvement_percent` (default: 10%). Recommended tighter defaults for deterministic/server environments (5%), looser for real-world desktop (15%).

## Golden Run Validation Plan

Before implementing the full system, validate measurement correctness with a minimal "golden run":

**Setup:**
- 1 proxy: nginx
- 2 languages: Rust (fast) and Python (slow)
- 2 protocols: HTTP/2 and HTTP/3
- 2 connection modes: cold and warm
- 1 testbed: Ubuntu Server (headless Chrome)

**Expected invariants (sanity checks):**

| Invariant | Expected | If violated |
|-----------|----------|-------------|
| warm < cold (total time) | Warm connections should be faster (no TLS handshake) | Connection control is broken |
| Rust app_time < Python app_time | Rust processes faster | Workload contract mismatch |
| proxy overhead stable | nginx overhead similar for both languages | Timing normalization broken |
| h3 cold <= h2 cold (after warmup) | QUIC 0-RTT should match or beat TCP+TLS | Protocol forcing not working |
| Server-Timing app;dur ≈ upstream;dur | Proxy upstream matches language self-report | Timing header pipeline broken |
| error_rate ≈ 0% | Clean run, no errors | Deployment or config issue |
| protocol_mismatch = false | Forced protocol actually used | Chrome flags not applied |

**Negative control (validates safety checks work):**

| Test | Action | Expected |
|------|--------|----------|
| Force HTTP/3 against HAProxy (no QUIC) | Request h3 mode for a proxy that doesn't support it | `protocol_mismatch = true`, result excluded from aggregation |
| Remove `Timing-Allow-Origin` header | Omit the header from one test language config | Raw timing fields (dns_ms, tcp_ms, etc.) are zeroed/absent; measurement quality badge degrades to "advisory" |

If the negative control does NOT raise `protocol_mismatch`, the detection pipeline is broken — halt and fix.

**Execution:**
- Run golden config before any user-facing deployment
- If any invariant fails or negative control doesn't trigger, halt and investigate — do not ship broken measurement
- Golden run config is stored as a built-in template ("Validation Run") in the wizard

## CPU Pinning Strategy

For VMs with varying core counts, use proportional allocation:

| VM Cores | Proxy | Language | OS/Chrome |
|----------|-------|----------|-----------|
| 2 | shared | shared | shared (annotated `shared_cores: true`) |
| 4 | cores 0-1 (50%) | cores 2-3 (50%) | shared with language |
| 8 | cores 0-1 (25%) | cores 2-5 (50%) | cores 6-7 (25%) |
| 16+ | cores 0-3 (25%) | cores 4-11 (50%) | cores 12-15 (25%) |

Language always gets the most cores — it's the primary subject of measurement. Proxy gets enough to avoid being the bottleneck. OS/system gets remainder.

## Low-Noise Preset

For high-confidence regression tracking, a built-in "Low Noise" template provides:
- 1 proxy (nginx)
- 1 language (user's choice)
- Pinned CPU (4+ core VM required)
- Deterministic desktop or server only
- Extended warmup (20 cycles)
- Warm connections only
- Single HTTP version (user's choice)

This minimizes variance for detecting small performance changes between runs.
