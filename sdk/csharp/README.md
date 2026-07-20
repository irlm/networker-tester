# LagHound.Endpoint (C# / ASP.NET Core)

Embed a tiny, production-safe diagnostic endpoint into your ASP.NET Core app so
the LagHound tester fleet can split total request time into
**DNS → TCP → TLS → network transfer → server processing**. The server-processing
slice comes from a `Server-Timing: app;dur=<ms>` header this SDK stamps on every
response — that split is the whole point.

Implements [endpoint contract v1](../../docs/sdk/contract-v1.md), pinned to the
machine-readable [`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json).
**Zero dependencies** beyond the shared ASP.NET Core framework.

## Quickstart

```csharp
using LagHound.Endpoint;

var builder = WebApplication.CreateBuilder(args);

// One-line register (fail-closed: throws at startup if no token is available).
builder.Services.AddLagHound(o =>
{
    o.Token = builder.Configuration["LAGHOUND_TOKEN"]; // or set LAGHOUND_TOKEN env
    // o.Prefix = "/laghound";
    // o.AppName = "checkout-api";
    // o.DownloadCapBytes = 4 * 1024 * 1024;
});

var app = builder.Build();
app.UseLagHound();   // mount the five routes under the prefix
app.Run();
```

Or the one-call form (no DI registration):

```csharp
app.MapLagHound(new LagHoundOptions { Token = builder.Configuration["LAGHOUND_TOKEN"] });
```

Five routes appear under the prefix (default `/laghound`):

| Route | Purpose |
|-------|---------|
| `GET /health` | Liveness + which routes are enabled |
| `GET /echo` | Latency + the network-vs-server split |
| `GET /download?bytes=N` | Server→client throughput (default 4 MiB, hard max 32 MiB) |
| `POST /upload` | Client→server throughput (same caps) |
| `GET /info` | SDK version, language, config echo (never the token) |

### Custom server-side marks

Add host-app breakdown marks (`mark-db`, `mark-cache`, …) from your handlers:

```csharp
LagHoundMarks.Mark(httpContext, "db", dbElapsed);
```

They surface as `Server-Timing: …, mark-db;dur=41.9` and show up in reports as a
server-side breakdown. Names must match `[a-z0-9]{1,24}`.

## Options

| Option | Default | Notes |
|--------|---------|-------|
| `Token` | *(required)* | Shared secret, min 16 bytes. Falls back to `LAGHOUND_TOKEN`. No token → refuses to mount. |
| `PreviousToken` | *(off)* | Optional second token for zero-downtime rotation (≤ 2 tokens). |
| `Prefix` | `/laghound` | Must start with `/`, no trailing slash. |
| `AppName` | *(off)* | Label echoed on `/health` and `/info`. Never auto-derived. |
| `DownloadCapBytes` | 4 MiB | Clamped to the 32 MiB absolute max. |
| `UploadCapBytes` | 4 MiB | Clamped to the 32 MiB absolute max. |
| `RatePerIpRps` / `RatePerIpBurst` | 10 / 20 | Per-IP token bucket. |
| `RateGlobalRps` / `RateGlobalBurst` | 50 / 100 | Global token bucket. |
| `MaxConcurrent` | 8 | In-flight LagHound requests per process. |
| `MaxConcurrentTransfers` | 2 | In-flight `/download` + `/upload` combined. |
| `ByteBudgetBytes` / `ByteBudgetWindowSeconds` | off / 600 | Optional sampling budget → `429 + Retry-After`. |
| `EnableEcho/Download/Upload/Info` | all `true` | Disabled routes are bare 404s and reported `false` in `/health`. |

## Safe in production, by contract

- **Invisible.** Without the token every route is a plain, body-less `404` — the
  same 404 a wrong token, an unknown subpath, or the kill switch produces. No
  `WWW-Authenticate`, no LagHound headers, nothing to fingerprint. This includes
  `/health`.
- **Constant-time auth.** Tokens compared with `CryptographicOperations.FixedTimeEquals`
  over a fixed-length hash, so length never short-circuits.
- **Rate limited before auth.** Per-IP + global token buckets run *before* the
  token check, so brute-forcing is throttled; unauthenticated limiter rejections
  are bare 404s (not 429), preserving invisibility.
- **Hard byte caps.** Configurable download/upload caps, absolute max 32 MiB that
  config cannot exceed. Downloads over the cap are **clamped** (actual size in
  `X-LagHound-Bytes`); uploads over the cap get `413` — the body is **not read**
  when `Content-Length` already exceeds it.
- **Streamed, never buffered.** `/download` streams `0x42` fill from a single
  per-process 64 KiB buffer; `/upload` drains-and-counts in 64 KiB chunks. No
  allocation proportional to payload size.
- **Concurrency capped.** ≤ 8 in-flight LagHound requests, of which ≤ 2 may be
  transfers — a struggling app serves at most 2 × its cap of diagnostic traffic.
- **Byte budget (optional).** Sliding-window transfer budget → `429 + Retry-After`.
- **Kill switch.** `LAGHOUND_DISABLED=1` turns every route into a plain 404 with
  no code deploy (re-read at most once per second).
- **Zero logging / zero reflection.** No bodies, tokens, or header sets logged;
  no request input reflected into responses; error messages are fixed strings.
- **Fail closed.** Any exception inside a LagHound handler becomes a `500`
  envelope confined to the LagHound route — it never crashes the host.

## Pointing the fleet at it

Existing tester modes work today with `--bearer-token <token>` (the SDK accepts
`Authorization: Bearer` as an equivalent of `X-LagHound-Token`): `http1/2/3` and
`curl` against `{prefix}/echo`, `webdownload`/`webupload` against the transfer
routes. See [contract §8](../../docs/sdk/contract-v1.md#8-tester-compatibility-map).

## Sample app

A runnable minimal host lives in [`Example/`](Example/README.md).

## Conformance

`LagHound.Endpoint.Tests` (in `Networker.sln`, covered by the `Build & audit (C#)`
CI job) runs a conformance suite that deserializes `shared/sdk-contract-v1.json`
and asserts every route shape, cap, header, and status against an in-memory host,
plus the safety properties above.
