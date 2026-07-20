# @laghound/endpoint

Embed a tiny **LagHound diagnostic endpoint** (contract v1) into your Node app.
The LagHound multi-cloud tester fleet then probes your *real* service from
outside and splits total request time into:

```
DNS  →  TCP  →  TLS  →  network transfer  →  server processing
```

The first four phases are measured by the probes. The last one comes from your
app, via a `Server-Timing: app;dur=<ms>` header this SDK stamps on every
response — that split is the whole point.

- **Zero runtime dependencies** — `node:crypto` (`timingSafeEqual`),
  `node:http`, `node:stream` only.
- **One factory, three mounting styles** — Express/Connect middleware, a
  Fastify plugin, or a bare `node:http` handler.
- Ships **ESM + CJS** with type declarations.
- Pinned to [`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json);
  the authoritative spec is [`docs/sdk/contract-v1.md`](../../docs/sdk/contract-v1.md).

## Install

```bash
npm install @laghound/endpoint
```

Runtime requires Node ≥ 18.17 (the published `dist/` is plain JS). Building and
running the test suite from source needs Node ≥ 22.6 (native TypeScript
type-stripping).

## Quickstart

The single `laghound(options)` factory returns a handler that detects how it is
being mounted. `token` is required — without one (or `LAGHOUND_TOKEN` in the
environment) the factory **throws** rather than mount open routes.

### Express / Connect

```js
import express from "express";
import { laghound } from "@laghound/endpoint";

const app = express();
app.use(laghound({ token: process.env.LAGHOUND_TOKEN })); // prefix: "/laghound"

// ... your app routes ...
app.listen(8080);
```

Unmatched paths fall through to `next()`, so mount it anywhere in the chain.

### Fastify

```js
import Fastify from "fastify";
import { laghound } from "@laghound/endpoint";

const fastify = Fastify();
await fastify.register(laghound({ token: process.env.LAGHOUND_TOKEN }));

fastify.get("/", async () => "ok");
await fastify.listen({ port: 8080 });
```

The plugin hooks `onRequest` and hijacks the reply only for requests under the
prefix; everything else routes normally.

### Bare `node:http`

```js
import http from "node:http";
import { laghound } from "@laghound/endpoint";

const lh = laghound({ token: process.env.LAGHOUND_TOKEN });

http
  .createServer((req, res) => {
    // Try LagHound first; if it didn't own the path, handle it yourself.
    if (req.url?.startsWith("/laghound")) return lh(req, res);
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("ok\n");
  })
  .listen(8080);
```

Used as the *only* server handler (`http.createServer(lh)`), unmatched paths
get a bare `404`.

### CommonJS

```js
const { laghound } = require("@laghound/endpoint");
app.use(laghound({ token: process.env.LAGHOUND_TOKEN }));
```

## Routes

All paths are relative to the prefix (default `/laghound`) and require the
token (§ Auth). See the contract for full response shapes.

| Route | Purpose |
|-------|---------|
| `GET /health` | Liveness + which routes are enabled (O(1)). |
| `GET /echo` | Latency + the network-vs-server split. Fixed body, no reflection. |
| `GET /download?bytes=N` | Server→client throughput. Default 4 MiB, hard max 32 MiB, clamped-and-reported via `X-LagHound-Bytes`. |
| `POST /upload` | Client→server throughput. Drained-and-counted, never buffered. |
| `GET /info` | SDK version, language, config echo — never the token. |

## Options

```ts
laghound({
  token,                    // string | [current, previous] — required (min 16 bytes). Falls back to LAGHOUND_TOKEN.
  prefix,                   // default "/laghound" — must start with "/", no trailing slash
  downloadCapBytes,         // default 4 MiB, clamped to the 32 MiB absolute max
  uploadCapBytes,           // default 4 MiB, clamped to the 32 MiB absolute max
  ratePerIp,                // { rps, burst } — default { rps: 10, burst: 20 }
  rateGlobal,               // { rps, burst } — default { rps: 50, burst: 100 }
  maxConcurrent,            // default 8 in-flight LagHound requests
  maxConcurrentTransfers,   // default 2 in-flight /download + /upload
  byteBudget,               // { bytes, windowS } — off by default
  appName,                  // optional label echoed on /health + /info (never auto-derived)
  routes,                   // { echo?, download?, upload?, info? } — disable individual routes
  trustedProxies,           // socket peers allowed to set X-Forwarded-For (default: XFF ignored)
});
```

### Custom `Server-Timing` marks

The handler exposes `mark(name, durationMs)` so your handlers can add
server-side breakdown marks that show up in reports:

```js
const lh = laghound({ token: process.env.LAGHOUND_TOKEN });
app.use(lh);

// elsewhere, after timing a dependency:
lh.mark("db", 41.9);     // surfaces as `mark-db;dur=41.9` on the next /echo
```

Mark names must match `[a-z0-9]{1,24}`.

## Auth

- Send the shared secret as `X-LagHound-Token: <token>` **or**
  `Authorization: Bearer <token>` (equivalent; `X-LagHound-Token` wins if both
  are present).
- Comparison is constant-time (`crypto.timingSafeEqual` over SHA-256 digests).
- **A bad or missing token returns a bare `404`** — no body, no LagHound
  headers, no `WWW-Authenticate`. The routes are indistinguishable from a route
  that does not exist, including `/health`. Nothing for a scanner to
  fingerprint.
- Pass `token: [current, previous]` for zero-downtime rotation (≤ 2 tokens).

## Safe in production, by contract

- **Byte caps** — configurable download/upload cap (default 4 MiB), hard 32 MiB
  absolute max that config cannot exceed. Over-cap downloads are *clamped and
  reported*; over-cap uploads get `413` **without reading the body**.
- **Rate limits** — per-IP (10 rps / burst 20) and global (50 rps / burst 100)
  token buckets; the per-IP table is LRU-capped so address spraying can't grow
  memory. Authenticated over-limit → `429 + Retry-After`; unauthenticated →
  bare `404`.
- **Concurrency caps** — ≤ 8 in-flight LagHound requests, of which ≤ 2 may be
  `/download`/`/upload` transfers. A struggling app serves at most 2 × 4 MiB of
  diagnostic traffic.
- **Streaming** — downloads stream ≤ 64 KiB chunks from a single per-process
  buffer; uploads are drained and counted. No allocation proportional to
  request size.
- **Optional byte budget** — `{ bytes, windowS }` sliding window; once
  exhausted, transfers get `429 + Retry-After`.
- **Kill switch** — `LAGHOUND_DISABLED=1` turns every route into the same bare
  `404` a wrong token gets, without a code deploy.
- **Zero logging / zero reflection** — never logs bodies or tokens; no route
  reflects request input; error messages are fixed strings.
- **Fail-closed** — refuses to mount without a token; an exception inside a
  LagHound handler becomes a `500` envelope confined to the route, never
  crashing the host.

## Scripts

```bash
npm run build   # tsc → dist/esm (ESM) + dist/cjs (CJS), then stamp the CJS package.json
npm test        # node --test on the conformance + safety suites (contract-pinned)
npm run lint    # tsc --noEmit type-check
```

## Pointing the fleet at it

Existing tester modes work today (contract §8): `http1/2/3` and `curl` against
`{prefix}/echo`, `webdownload`/`webupload` against the transfer routes — all
with `--bearer-token <token>`. The dedicated `sdkprobe` mode (reads the
`/health` capability map, reports the five-way split) lands with the
control-plane wave.
