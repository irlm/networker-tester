# LagHound SDK for Go (`net/http`)

Embed a tiny diagnostic endpoint into your Go service. The LagHound tester
fleet then probes your *real* app from outside and splits total request time
into `DNS → TCP → TLS → network transfer → server processing`. The last phase
comes from your app, via a `Server-Timing: app;dur=<ms>` header this SDK stamps
on every response — that split is the whole point.

- **Contract:** [`docs/sdk/contract-v1.md`](../../docs/sdk/contract-v1.md) (authoritative)
- **Machine-readable:** [`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json)
  (the conformance suite in this package pins to it)
- **Zero third-party dependencies** — stdlib only (`crypto/subtle`, `net/http`, …).

## Install

```bash
go get github.com/irlm/networker-tester/sdk/go/laghound
```

## Quickstart — `net/http`

```go
package main

import (
	"net/http"
	"os"

	laghound "github.com/irlm/networker-tester/sdk/go/laghound"
)

func main() {
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("hello"))
	})

	// Mount registers both "/laghound" and "/laghound/".
	laghound.Mount(mux, laghound.Config{Token: os.Getenv("LAGHOUND_TOKEN")})

	http.ListenAndServe(":8080", mux)
}
```

Prefer to hold the `http.Handler` yourself? Use `laghound.Handler`:

```go
h := laghound.Handler(laghound.Config{Token: os.Getenv("LAGHOUND_TOKEN")})
mux.Handle("/laghound", h)
mux.Handle("/laghound/", h)
```

`Handler` **fails closed**: with no token (neither `Config.Token` nor the
`LAGHOUND_TOKEN` env var) it returns a handler that answers every request with
a bare `404` and mounts nothing. Call `laghound.New` instead if you want to
observe the configuration error:

```go
h, err := laghound.New(laghound.Config{Token: tok})
if err != nil { /* ErrNoToken, ErrTokenTooShort, ErrBadPrefix, ... */ }
```

## Quickstart — chi

`chi.Mount` strips the prefix before dispatching; the handler detects both
mounting styles, so it just works:

```go
r := chi.NewRouter()
r.Mount("/laghound", laghound.Handler(laghound.Config{Token: os.Getenv("LAGHOUND_TOKEN")}))
```

The same pattern works with anything that speaks `http.Handler`
(gorilla/mux `PathPrefix(...).Handler(...)`, `http.StripPrefix`, Echo/Gin
adapters, …).

## Custom `Server-Timing` marks

Add server-side breakdown marks (`mark-db`, `mark-cache`, …) from your own
handlers. Wrap the routes you want instrumented with `laghound.WithMarks`, then
record marks against the request context:

```go
mux.Handle("/checkout", laghound.WithMarks(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	// ... query the DB ...
	laghound.MarkSince(r.Context(), "db", start)   // -> Server-Timing: mark-db;dur=41.900
	laghound.Mark(r.Context(), "cache", 2.5)       // -> mark-cache;dur=2.500
	w.Write([]byte("ok"))
})))
```

Mark names must match `[a-z0-9]{1,24}`; invalid names, negative or non-finite
durations, and marks recorded outside a LagHound-instrumented request are
silently ignored — `Mark` never panics into your handler. At most 8
`Server-Timing` metrics are emitted per response (contract §4.1).

## Options (`laghound.Config`)

| Field | Default | Notes |
|-------|---------|-------|
| `Token` | *(required)* | Shared secret, min 16 bytes. Falls back to `LAGHOUND_TOKEN`. |
| `PreviousToken` | — | Second accepted token for zero-downtime rotation. |
| `Prefix` | `/laghound` | Must start with `/`, no trailing `/`. |
| `AppName` | — | Optional label echoed on `/health` and `/info`. |
| `DownloadCapBytes` | `4 MiB` | Clamped to the 32 MiB absolute max. |
| `UploadCapBytes` | `4 MiB` | Clamped to the 32 MiB absolute max. |
| `RatePerIP` | `{RPS: 10, Burst: 20}` | Per-client-IP token bucket. |
| `RateGlobal` | `{RPS: 50, Burst: 100}` | Process-wide token bucket. |
| `MaxConcurrent` | `8` | In-flight LagHound requests. |
| `MaxConcurrentTransfers` | `2` | In-flight `/download` + `/upload`. |
| `ByteBudget` | *(off)* | `&laghound.ByteBudget{Bytes, WindowS}` sampling budget. |
| `DisableEcho` / `DisableDownload` / `DisableUpload` / `DisableInfo` | `false` | Drop individual routes (they become bare `404` and report `false` in `/health`). `/health` is always enabled. |

## Routes

| Route | Purpose |
|-------|---------|
| `GET /health` | Liveness + capability map (O(1); precomputed at init). |
| `GET /echo` | Fixed-body latency + server-split probe target. |
| `GET /download?bytes=N` | Server→client throughput. `N` clamped to the cap; actual size reported via `Content-Length` + `X-LagHound-Bytes`. |
| `POST /upload` | Client→server throughput. Drain-and-count; over-cap `Content-Length` → `413` without reading. |
| `GET /info` | SDK version, language, config echo — never the token or any derivative. |

## Safety (by contract)

- **Invisible.** Bad/missing token → a bare `404` with no LagHound headers, on
  every route including `/health`. Same `404` the kill switch produces.
- **Kill switch.** `LAGHOUND_DISABLED=1` → every route bare `404`, evaluated
  per request, no redeploy.
- **Rate-limit before auth.** Limits run before the token check, so brute
  forcing is throttled; throttled *unauthenticated* traffic still gets a bare
  `404` (not `429`) to stay invisible.
- **Byte caps.** Config caps default 4 MiB, hard-capped at 32 MiB. Download
  clamps and reports; upload over `Content-Length` cap → `413` without reading
  the body; chunked over-cap drains to the cap then `413` + connection close.
- **Concurrency caps.** ≤ 8 in-flight requests, ≤ 2 concurrent transfers —
  the primary "never amplify an outage" control.
- **Streaming.** `/download` streams `0x42` from a single shared 64 KiB
  buffer; no allocation proportional to `N`. `/upload` drains to
  `io.Discard`; peak memory is O(chunk).
- **Constant-time auth.** Tokens are SHA-256'd and compared with
  `crypto/subtle.ConstantTimeCompare` so length never leaks.
- **Zero logging, zero reflection.** No request bodies, tokens, or headers are
  logged; no route reflects request input; error messages are fixed strings.
- **Fail-safe.** A panic inside LagHound code becomes a `500` envelope confined
  to the LagHound route — the host process survives.

## Conformance

```bash
cd sdk/go
go test ./...
go test -race ./...
```

The suite loads `../../shared/sdk-contract-v1.json` and asserts every route
shape, cap, header, and status, plus the safety behaviors (bare-404
invisibility, kill switch, clamping, byte budget, concurrency cap, rate
limits, no-secret `/info`).

## Pointing the fleet at it

Existing tester modes work today (spec §8): `http1/2/3` and `curl` against
`{prefix}/echo`, `webdownload`/`webupload` against the transfer routes — all
with `--bearer-token <token>` (the SDK accepts `Authorization: Bearer` as an
equivalent of `X-LagHound-Token`). The dedicated `sdkprobe` mode lands with the
control-plane wave.
