# LagHound SDK

Embed a tiny diagnostic endpoint into **your** app; the LagHound multi-cloud
tester fleet measures your *real* app from outside and tells you where the
time goes:

```
DNS  →  TCP  →  TLS  →  network transfer  →  server processing
```

The first four phases are measured by the probes. The last one comes from
your app itself, via a `Server-Timing: app;dur=<ms>` header the SDK stamps on
every response — that split is the whole point.

- **Spec:** [`contract-v1.md`](contract-v1.md) (authoritative)
- **Machine-readable:** [`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json)
  (SDKs + tester pin conformance tests to it)

## What you get

Five routes under a configurable prefix (default `/laghound`), all behind a
shared token:

| Route | What it measures |
|-------|------------------|
| `GET /health` | Liveness + which routes are enabled |
| `GET /echo`   | Latency + the network-vs-server split |
| `GET /download?bytes=N` | Server→client throughput (default 4 MiB, hard max 32 MiB) |
| `POST /upload` | Client→server throughput (same caps) |
| `GET /info`   | SDK version, language, config echo (never secrets) |

## Safe in production, by contract

Per-IP + global rate limits, ≤ 8 concurrent requests, ≤ 2 concurrent
transfers, hard 32 MiB byte cap, streamed bodies (no allocation proportional
to request size), optional byte budget with `429 + Retry-After`, zero logging
of bodies/tokens, zero reflection of request input, and a kill switch:
`LAGHOUND_DISABLED=1` makes every route a plain 404 — the same 404 a wrong
token gets, so the routes are invisible to scanners.

## Per-language integration (the API every SDK wave must implement)

Each SDK is three steps: **install → mount → token**. Constructor/mount names
below are the contract for the language waves.

### C# (ASP.NET Core)

```csharp
// dotnet add package LagHound.AspNetCore
app.MapLagHound(new LagHoundOptions { Token = builder.Configuration["LAGHOUND_TOKEN"] });
// optional: opts.Prefix = "/laghound"; opts.DownloadCapBytes = 4 * 1024 * 1024;
```

### JS (Node — Express/Fastify/etc.)

```js
// npm install @laghound/node
const { laghound } = require("@laghound/node");
app.use(laghound({ token: process.env.LAGHOUND_TOKEN })); // prefix: "/laghound"
```

### Python (ASGI — FastAPI/Starlette/Django)

```python
# pip install laghound
from laghound import LagHoundMiddleware
app.add_middleware(LagHoundMiddleware, token=os.environ["LAGHOUND_TOKEN"])  # prefix="/laghound"
```

### Rust (axum / tower)

```rust
// cargo add laghound
let router = router.merge(laghound::router(laghound::Config::new(token)));
// Config::new(token).prefix("/laghound").download_cap(4 * 1024 * 1024)
```

Shipped SDK + runnable sample: [`sdk/rust/`](../../sdk/rust/README.md).

### Go (net/http)

```go
// go get github.com/laghound/laghound-go
mux.Handle("/laghound/", laghound.Handler(laghound.Config{Token: os.Getenv("LAGHOUND_TOKEN")}))
```

Every SDK also exposes `mark(name, duration)` so your handlers can add custom
`Server-Timing` marks (`mark-db`, `mark-cache`, …) that show up in reports as
a server-side breakdown.

## Pointing the fleet at it

Existing tester modes work today (see spec §8): `http1/2/3` and `curl` against
`{prefix}/echo`, `webdownload`/`webupload` against the transfer routes — all
with `--bearer-token <token>` (the SDK accepts `Authorization: Bearer` as an
equivalent of `X-LagHound-Token`). The dedicated `sdkprobe` mode — which reads
the `/health` capability map and reports the five-way split as its primary
metric — lands with the control-plane wave.
