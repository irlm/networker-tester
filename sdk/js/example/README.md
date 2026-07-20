# LagHound Node sample

A tiny "real service" that embeds [`@laghound/endpoint`](../README.md) at
`/laghound`, alongside two ordinary application routes. Bare `node:http`, **zero
dependencies** — it runs straight from a checkout (Node ≥ 22.6 strips the SDK's
TypeScript source; if you `npm run build` in the parent first, it uses the
compiled `dist/` instead).

## Routes

| Route | Response |
|-------|----------|
| `GET /` | `js sample ok` |
| `GET /work` | `done` after ~30ms of simulated work (recorded as a `mark-work` Server-Timing) |
| `/laghound/*` | The LagHound diagnostic endpoint (contract v1) — needs the token |

## Run

```bash
# from sdk/js/example
node server.mjs
```

Configuration (both optional):

- `LAGHOUND_TOKEN` — shared secret (default `demo-token-laghound`)
- `PORT` — listen port (default `8082`)

```bash
LAGHOUND_TOKEN=my-long-secret-token PORT=9000 node server.mjs
```

## Try it

```bash
# app routes
curl localhost:8082/
curl localhost:8082/work

# LagHound routes (need the token)
curl -H "X-LagHound-Token: demo-token-laghound" localhost:8082/laghound/health
curl -H "X-LagHound-Token: demo-token-laghound" localhost:8082/laghound/echo -i        # note Server-Timing: app;dur=...
curl -H "X-LagHound-Token: demo-token-laghound" "localhost:8082/laghound/download?bytes=1048576" -o /dev/null -s -w "%{size_download}\n"

# without the token -> bare 404 (invisible), same as a route that doesn't exist
curl -i localhost:8082/laghound/health
```
