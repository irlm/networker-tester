# LagHound SDK

Embed a tiny diagnostic endpoint into **your** app. The LagHound multi-cloud
tester fleet measures your *real* app from outside. It tells you where the time
goes:

```
DNS  →  TCP  →  TLS  →  network transfer  →  server processing
```

The probes measure the first four phases. Your app provides the last phase. The
SDK stamps a `Server-Timing: app;dur=<ms>` header on every response. This split
is the whole point.

- **Spec:** [`contract-v1.md`](contract-v1.md) (authoritative)
- **Machine-readable:** [`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json)
  (the SDKs and the tester pin the conformance tests to it)

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

The contract keeps the SDK safe in production:

- Per-IP and global rate limits.
- A maximum of 8 concurrent requests and 2 concurrent transfers.
- A hard 32 MiB byte cap.
- Streamed bodies. The allocation is not proportional to the request size.
- An optional byte budget with `429 + Retry-After`.
- No logging of bodies or tokens.
- No reflection of the request input.
- A kill switch. `LAGHOUND_DISABLED=1` makes every route a plain 404. This is
  the same 404 that a wrong token gets, so the routes are invisible to scanners.

## Per-language integration (the API every SDK wave must implement)

Each SDK is three steps: **install → mount → token**. The constructor and mount
names below are the contract for the language waves.

### C# (ASP.NET Core)

```csharp
// dotnet add package LagHound.AspNetCore
app.MapLagHound(new LagHoundOptions { Token = builder.Configuration["LAGHOUND_TOKEN"] });
// optional: opts.Prefix = "/laghound"; opts.DownloadCapBytes = 4 * 1024 * 1024;
```

Implementation: [`sdk/csharp/`](../../sdk/csharp/README.md) (`LagHound.Endpoint`).

### JS (Node — Express/Fastify/etc.)

```js
// npm install @laghound/node
const { laghound } = require("@laghound/node");
app.use(laghound({ token: process.env.LAGHOUND_TOKEN })); // prefix: "/laghound"
```

> Node SDK (`@laghound/endpoint`) — Express/Fastify/`http` adapters, conformance suite + sample: [`sdk/js/`](../../sdk/js/README.md).

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

Shipped: [`sdk/go/`](../../sdk/go/) — `net/http` + chi quickstart, conformance suite, and a runnable sample.

Every SDK also exposes `mark(name, duration)`. Your handlers can add custom
`Server-Timing` marks (`mark-db`, `mark-cache`, …) with it. These marks show up
in the reports as a server-side breakdown.

## Run all five languages on one host

The [`examples/`](../../examples/) harness builds and runs every SDK sample on
one target. It uses `docker compose up --build` (C# 8081, JS 8082, Python 8083,
Rust 8084, Go 8085). This harness is a cross-language conformance demo and the
"works in every language" sales demo. `examples/probe-all.sh` asserts the
contract across all five samples at once (200 + `contract:v1` with the token, a
bare 404 without it).

## Pointing the fleet at it

The existing tester modes work today (see spec §8). Use `http1/2/3` and `curl`
against `{prefix}/echo`. Use `webdownload`/`webupload` against the transfer
routes. Add `--bearer-token <token>` to each mode. The SDK accepts
`Authorization: Bearer` as an equivalent of `X-LagHound-Token`. The dedicated
`sdkprobe` mode lands with the control-plane wave. It reads the `/health`
capability map. It reports the five-way split as its primary metric.
