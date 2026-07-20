# LagHound SDK — Endpoint Contract v1

**Status:** authoritative spec — v1, frozen once the first SDK ships.
**Machine-readable twin:** [`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json)
(language SDKs and the tester pin their conformance tests to it, exactly like
`shared/modes.json`).
**Audience:** implementers of the C#/JS/Python/Rust/Go SDKs and of the tester's
SDK-aware probe mode.

---

## 1. What this is

A customer embeds a tiny **LagHound endpoint** into their existing production
app. It mounts a handful of diagnostic routes under a configurable prefix.
Our multi-cloud tester fleet (`crates/networker-tester`) then probes the
customer's *real* app from outside and splits total request time into:

```
DNS  →  TCP connect  →  TLS handshake  →  network transfer  →  SERVER PROCESSING
```

The first four phases are measured client-side by the tester (it already does
this for every HTTP probe — see `metrics.rs` `DnsResult` / `TcpResult` /
`TlsResult` / `HttpResult`). The last one — server processing — is the piece
only the customer's app can report, and it does so via a **`Server-Timing`
response header** (§4). That header is the entire reason this SDK exists.

Two non-negotiable properties drive every rule below:

1. **Production-safe.** The SDK must never amplify an outage: hard byte caps,
   rate limits, concurrency caps, a kill switch, and zero per-request
   allocation proportional to request size.
2. **Invisible.** Without the shared token the routes are indistinguishable
   from a 404. No banner, no `WWW-Authenticate`, nothing for a scanner to
   fingerprint.

---

## 2. Mounting & configuration

| Config key      | Default      | Notes |
|-----------------|--------------|-------|
| `prefix`        | `/laghound`  | All routes live under it. Must start with `/`, no trailing slash. |
| `token`         | *(required)* | Shared secret, set at SDK init. Min 16 bytes. See §5. |
| `download_cap_bytes` | `4194304` (4 MiB) | Effective cap for `/download`. Clamped to the absolute max. |
| `upload_cap_bytes`   | `4194304` (4 MiB) | Effective cap for `/upload`. Clamped to the absolute max. |
| `rate_per_ip`   | `10` req/s, burst `20` | §6.2 |
| `rate_global`   | `50` req/s, burst `100` | §6.2 |
| `max_concurrent`| `8` | In-flight requests across all LagHound routes. §6.3 |
| `max_concurrent_transfers` | `2` | In-flight `/download` + `/upload` combined. §6.3 |
| `byte_budget`   | *(off)* | Optional sampling budget, e.g. 256 MiB / 10 min window. §6.4 |
| `app_name`      | *(off)* | Optional label echoed on `/health` and `/info`. Never auto-derived from the host app. |

**Absolute maxima (not configurable, enforced even if config asks for more):**
download/upload payloads: **32 MiB** (`33554432` bytes).

**Environment overrides** (read by every SDK, evaluated per §6.5):

- `LAGHOUND_DISABLED=1` — kill switch, everything 404s.
- `LAGHOUND_TOKEN` — token source when not passed programmatically.

Routes MUST be mounted only when a token is available; an SDK initialized
without any token MUST refuse to mount (fail-closed) rather than mount open
routes.

---

## 3. Routes

All paths below are relative to `prefix`. Every response (success and
enveloped errors) carries `Server-Timing` (§4) and
`Cache-Control: no-store, no-cache, must-revalidate`. Bare 404s (§5, §6.5)
carry neither.

### 3.1 `GET /health`

Cheap liveness + capability discovery. MUST be O(1): the body is precomputed
at init except `uptime_s`. No I/O, no locks beyond an atomic read.

```json
{
  "contract": "v1",
  "status": "ok",
  "sdk": { "lang": "csharp", "version": "1.0.0" },
  "app": "checkout-api",
  "uptime_s": 86123,
  "routes": { "health": true, "echo": true, "download": true, "upload": true, "info": true }
}
```

- `sdk.lang` — one of `csharp | js | python | rust | go`.
- `sdk.version` — the SDK package version (semver).
- `app` — omitted unless `app_name` is configured.
- `routes` — capability map. An operator may disable individual routes
  (e.g. no `/upload` in a cost-sensitive egress environment); the tester reads
  this map before scheduling probes. `health` is always `true` when the SDK is
  mounted and enabled.

### 3.2 `GET /echo`

The primary latency + server-split probe target. Returns a **fixed** payload:

```json
{ "contract": "v1", "ok": true }
```

- The body MUST be byte-for-byte constant for the lifetime of the process and
  MUST be < 1 KiB. It MUST NOT reflect any request input (no headers, no
  query, no body echo) — reflection is an amplification and injection surface.
- Request bodies, if sent, are ignored (not drained beyond the framework's
  minimum; SDKs SHOULD reject bodies > 64 KiB on this route with `413`).
- `Server-Timing: app;dur=<ms>` here is the number the tester subtracts from
  TTFB to compute the network-vs-server split (§4.3).

### 3.3 `GET /download?bytes=N`

Sustained server→client throughput.

- `bytes` — requested payload size. Omitted → default `4194304` (4 MiB).
  Present but unparsable/negative → `400 invalid_param` (a silent default on a
  garbage value would make measurements lie).
- Effective size = `min(N, download_cap_bytes, 33554432)`. Requests above the
  cap are **clamped, not rejected**; the actual size is reported via
  `Content-Length` and `X-LagHound-Bytes: <actual>` so the tester can detect
  the clamp and annotate the run.
- Body: fill byte `0x42` (`'B'`, matching `networker-endpoint`'s
  `DOWNLOAD_FILL`), **streamed** in chunks ≤ 64 KiB from a single per-process
  read-only buffer. Implementations MUST NOT allocate memory proportional to
  `N` per request (no allocation bombs).
- `Content-Type: application/octet-stream`, `Content-Length` always set.
- `Server-Timing: app;dur=<ms>` measures setup time only (before the first
  chunk is written), mirroring `networker-endpoint`'s `proc` semantics.

### 3.4 `POST /upload`

Sustained client→server throughput.

- The body is **drained and counted, never buffered**. Peak memory per request
  MUST be O(chunk), not O(body).
- Cap = `min(upload_cap_bytes, 33554432)`. Enforcement:
  - If `Content-Length` is present and exceeds the cap → immediate
    `413 payload_too_large` **without reading the body**.
  - If chunked/unknown length, drain up to cap, then respond `413` and close
    the connection (do not keep reading).
- Success response:

```json
{ "contract": "v1", "received_bytes": 4194304 }
```

- Headers: `X-LagHound-Bytes: <received>` (so the tester detects truncation
  without parsing JSON), `Server-Timing: recv;dur=<ms>, app;dur=<ms>` where
  `recv` is the body-drain wall time and `app` is post-drain processing
  (typically ≈0).

### 3.5 `GET /info`

Config echo for fleet inventory and debugging — **minus secrets**.

```json
{
  "contract": "v1",
  "sdk": { "lang": "csharp", "version": "1.0.0" },
  "app": "checkout-api",
  "prefix": "/laghound",
  "uptime_s": 86123,
  "token_set": true,
  "caps": { "download_bytes": 4194304, "upload_bytes": 4194304, "absolute_max_bytes": 33554432 },
  "limits": {
    "rate_per_ip": { "rps": 10, "burst": 20 },
    "rate_global": { "rps": 50, "burst": 100 },
    "max_concurrent": 8,
    "max_concurrent_transfers": 2,
    "byte_budget": null
  },
  "routes": { "health": true, "echo": true, "download": true, "upload": true, "info": true }
}
```

- The token value, or anything derived from it (hash, prefix, length), MUST
  NOT appear anywhere. Only the boolean `token_set`.
- No hostnames, IPs, env-var dumps, or host-app config — the SDK echoes only
  its own config keys listed above.

---

## 4. `Server-Timing` — the network-vs-server split

### 4.1 Syntax

Standard `Server-Timing` header (W3C), comma-separated metrics, each
`name;dur=<milliseconds>` with `dur` as a non-negative decimal:

```
Server-Timing: app;dur=12.3
Server-Timing: recv;dur=41.0, app;dur=0.4
Server-Timing: app;dur=57.1, total;dur=57.1, mark-db;dur=41.9, mark-render;dur=9.8
```

Constraints: ≤ 8 metrics per response, total header value ≤ 512 bytes,
metric names `[a-z0-9-]{1,32}`, `dur` values in ms with ≤ 3 decimal places.

### 4.2 Metric registry

| Metric | Required | Where | Meaning |
|--------|----------|-------|---------|
| `app`  | **MUST — every response** | all routes | Server processing time: from request fully received (headers for GET; body fully drained for `/upload`) to response headers ready. THE number for the split. |
| `recv` | MUST on `/upload` | upload | Body drain wall time. |
| `total`| SHOULD (alias, `= app` — or `= recv + app` on upload) | all routes | Compat alias: today's tester already parses `total`/`recv`/`proc` (`runner/http.rs::parse_server_timing_header`) but not yet `app`. Emitting `total` makes v1 SDKs measurable by every already-deployed tester. |
| `mark-<name>` | MAY | any | Custom marks from the host app (e.g. `mark-db`, `mark-cache`) surfaced in reports as a server-side breakdown. Names must match `mark-[a-z0-9]{1,24}`. |

SDKs MUST ignore-and-forward nothing: only metrics the SDK itself measured go
in the header. Host apps add marks through the SDK's API (e.g.
`laghound.mark("db", elapsed)`), never by string-concatenating headers.

### 4.3 Who consumes it, and the split formula

- **Today, unchanged:** `http1`/`http2`/`http3`/`curl` probes parse
  `recv`/`proc`/`total` into `ServerTimingResult`
  (`metrics.rs`: `recv_body_ms`, `processing_ms`, `total_server_ms`).
  Emitting `total` (§4.2) lights those fields up with zero tester changes.
- **Next wave (`sdkprobe`, §8):** parses `app` and `mark-*` and computes:

```
server_processing_ms = app.dur
network_transfer_ms  = ttfb_ms − app.dur          (request upstream + response first byte)
dns_ms / tcp_ms / tls_ms                            (already measured client-side)
```

`ttfb_ms` is the tester's existing request-write→first-response-byte timer
(`HttpResult.ttfb_ms`). When `app.dur > ttfb_ms` (clock/measure anomaly) the
tester clamps `network_transfer_ms` to 0 and flags the attempt.

### 4.4 CORS / browser probes

Responses SHOULD include `Timing-Allow-Origin: *` so browser-based probes
(`browser1..3`) can read the timing. This exposes durations only — never data.

---

## 5. Authentication

- Shared secret via **`X-LagHound-Token: <token>`**.
- **Also accepted:** `Authorization: Bearer <token>` — equivalent in every
  way. Rationale: `networker-tester` already ships a `--bearer-token` flag
  (used for `BENCH_API_TOKEN`-protected endpoints) and has **no** generic
  custom-header flag for probe requests; accepting Bearer means every deployed
  tester can authenticate today. If both headers are present,
  `X-LagHound-Token` wins; the other is ignored (not compared).
- Comparison MUST be **constant-time** over the full token length (e.g.
  `CryptographicOperations.FixedTimeEquals`, `crypto.timingSafeEqual`,
  `hmac.compare_digest`, `subtle::ConstantTimeEq`, `subtle.ConstantTimeCompare`).
  Length mismatch MUST NOT short-circuit observably (compare against a padded
  or hashed representation).
- **Bad or missing token → `404 Not Found`**, not 401/403:
  - The 404 MUST be indistinguishable from the host framework's "route does
    not exist" response where the SDK can produce it; otherwise a bare,
    body-less `404` with no LagHound headers, no envelope, no `Server-Timing`,
    no `WWW-Authenticate`.
  - Applies to *all* routes **including `/health`** — this differs
    deliberately from `networker-endpoint` (whose `/health` is auth-exempt for
    load balancers): the customer's own LB health checks their own app, not
    ours, and an open `/health` is a fingerprint.
- **Order of checks:** kill switch → rate/concurrency limits → auth → route
  logic. Rate limiting runs before auth so token brute-forcing is throttled
  like everything else; limiter rejections on unauthenticated traffic are
  also bare 404s (not 429) to stay invisible.
- Token rotation: SDKs MAY accept a list of ≤ 2 tokens (current + previous)
  to allow zero-downtime rotation; each compared constant-time.

---

## 6. Safety (non-negotiable)

Every rule here exists so that embedding LagHound can never make a customer
outage worse. Implementations that cannot meet a rule MUST NOT mount the
affected route (and reflect that in the `/health` capability map).

### 6.1 Byte caps

- Download and upload: configurable cap (default **4 MiB**), absolute max
  **32 MiB** that config cannot exceed. Enforcement per §3.3/§3.4.
- `/echo` body cap 64 KiB; oversized → `413`.

### 6.2 Rate limits

- **Per-IP:** token bucket, default 10 req/s, burst 20. IP taken from the
  socket peer; SDKs MUST NOT trust `X-Forwarded-For` unless the operator
  explicitly configures a trusted-proxy list.
- **Global:** token bucket, default 50 req/s, burst 100, across all LagHound
  routes in the process.
- Exceeded (authenticated traffic) → `429` with `Retry-After: <s>` header and
  the error envelope (§7). Exceeded (unauthenticated) → bare `404` (§5).
- Limiter state is in-process and O(bounded): per-IP table capped (default
  10 000 entries, LRU eviction) so an address-spraying attacker cannot grow
  memory.

### 6.3 Concurrency caps

- Default **8** in-flight LagHound requests per process; excess → `429`,
  `Retry-After: 1`.
- Of those, at most **2** may be `/download` or `/upload` transfers. This is
  the primary "never amplify an outage" control: a struggling app can be
  serving at most 2 × 4 MiB of diagnostic traffic.

### 6.4 Sampling / byte budget (optional)

When `byte_budget` is configured (e.g. `{"bytes": 268435456, "window_s": 600}`),
the SDK tracks transfer bytes per sliding window; once exhausted, `/download`
and `/upload` respond `429` with `Retry-After` set to the window remainder.
`/health`, `/echo`, `/info` are never budget-limited (they're O(1)).

### 6.5 Kill switch

`LAGHOUND_DISABLED=1` → every route returns a bare `404` (identical to the
bad-token response). Evaluated at request time; implementations MAY cache the
env read for ≤ 1 s. Flipping the variable (or the platform equivalent, e.g.
app-setting restart) disables the SDK without a code deploy.

### 6.6 Zero logging & zero reflection

- The SDK MUST NOT log request bodies, tokens, or full header sets — at any
  log level. Permitted log line: method, route, status, duration, byte count.
- No route reflects arbitrary request input into a response (§3.2). `/upload`
  responds with counts only. Error messages are fixed strings from §7's table
  — never interpolated request data.

### 6.7 Failure posture

Fail closed, degrade honest: internal SDK errors (limiter failure, budget
store failure) → `429` envelope, never a pass-through. The SDK MUST NOT crash
the host process; every handler is wrapped so a panic/exception inside
LagHound code converts to a `500` envelope confined to the LagHound route.

---

## 7. Response & error envelopes

Every JSON success body carries `"contract": "v1"` at the top level (§3
examples). Enveloped errors:

```json
{
  "contract": "v1",
  "error": {
    "code": "rate_limited",
    "message": "rate limit exceeded",
    "retry_after_ms": 1000
  }
}
```

| HTTP | `error.code`        | When | Notes |
|------|---------------------|------|-------|
| 400  | `invalid_param`     | Unparsable `bytes` value, bad query | Fixed message; never echoes the offending value. |
| 404  | *(bare — no envelope)* | Bad/missing token; kill switch; unknown subpath under prefix | §5, §6.5. |
| 405  | `method_not_allowed`| Wrong method on a known route (authenticated) | |
| 413  | `payload_too_large` | Upload over cap; echo body over 64 KiB | |
| 429  | `rate_limited`      | Rate/concurrency/budget exceeded; internal limiter failure | `Retry-After` header MUST be present; `retry_after_ms` mirrors it. |
| 500  | `internal`          | Caught handler exception | Fixed message `"internal error"`. |

Forward compat: consumers MUST ignore unknown JSON members (same additive rule
as `Networker.Contracts.ProbeRunResult`); a `contract` value other than `"v1"`
signals a breaking revision and consumers MUST NOT guess.

---

## 8. Tester compatibility map

How each existing `shared/modes.json` mode relates to a LagHound-embedded app
(target = the customer's real origin):

| Mode(s) | Works today? | Against | Notes |
|---------|--------------|---------|-------|
| `dns`, `tcp`, `tls`, `tlsresume`, `native` | **Unchanged** | host:port | No LagHound route involved; measures the customer's real DNS/TCP/TLS. |
| `http1`, `http2`, `http3`, `curl` | **Unchanged** (with `--bearer-token`) | `GET {prefix}/echo` | Auth works via Bearer (§5). Server split lights up via the `total` compat alias (§4.2) into `ServerTimingResult.total_server_ms`. |
| `webdownload`, `download`, `download1/2/3` | **Unchanged** (with `--bearer-token`) | `GET {prefix}/download?bytes=N` | Same `?bytes=N` shape as `networker-endpoint /download`. Tester must respect the 32 MiB abs max and detect clamping via `X-LagHound-Bytes`. |
| `webupload`, `upload`, `upload1/2/3` | **Unchanged** (with `--bearer-token`) | `POST {prefix}/upload` | Truncation detectable via `X-LagHound-Bytes` (analogue of `X-Networker-Received-Bytes`). |
| `udp`, `udpdownload`, `udpupload` | **N/A** | — | SDK is HTTP-embedded; no UDP listener in a customer app. |
| `pageload*`, `browser*` | **N/A** for SDK routes | — | Customers' real pages can still be probed directly; not a LagHound-route concern. |
| `apibench` | **N/A** | — | Benchmark-suite-only, needs the reference dataset. |
| **`sdkprobe`** | **NEW — next wave** | `GET {prefix}/echo` (+ `/health` capability read) | Composite mode: discovers capabilities from `/health`, runs echo probes, parses `app` + `mark-*`, and emits the DNS / TCP / TLS / network-transfer / server-processing five-way split (§4.3) as its primary metric. |

**The ask on `shared/modes.json` (do NOT add yet — the control-plane wave owns
that PR, per the drift guards in `modes_manifest_guard.rs`,
`modes-manifest.test.ts`, and `ModesManifestTests.cs`):**

```json
{ "id": "sdkprobe", "level": "tester", "catalog": true, "family": "http",
  "name": "SDK Probe", "description": "Server split",
  "detail": "Probes a customer-embedded LagHound endpoint — splits total time into DNS, TCP, TLS, network transfer, and server processing via Server-Timing",
  "group": "HTTP" }
```

That wave also touches: `Protocol` enum + `primary_metric_label/value` in
`metrics.rs`, `dispatch_once`/`log_attempt` in `dispatch.rs`,
`print_summary` in `summary.rs`, `parse_server_timing_header` in
`runner/http.rs` (add `app` + `mark-*`), `docs/deploy-config.md`, and an
integration test — the standard "Adding a New Protocol Variant" checklist.

---

## 9. Conformance

An SDK is v1-conformant when it passes the conformance suite pinned to
`shared/sdk-contract-v1.json`. The suite (shipped with each SDK wave) asserts,
at minimum: route shapes and envelopes byte-compatible with §3/§7; `app` on
every response; clamping + `X-LagHound-Bytes`; bare-404 behavior for bad
token, kill switch, and unauthenticated rate-limit hits; constant-time compare
(reviewed, not timed); streaming memory bound (RSS delta < 8 MiB during a
32 MiB download under the abs-max config); and the 429/`Retry-After` path.
