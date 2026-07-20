# LagHound Go sample app

A tiny `net/http` service that mounts the LagHound diagnostic endpoint at
`/laghound` next to two trivial app routes, so external probes see realistic
`Server-Timing`.

## Run

```bash
PORT=8085 LAGHOUND_TOKEN=demo-token-laghound go run .
```

- `PORT` — listen port (default `8085`).
- `LAGHOUND_TOKEN` — shared probe token (default `demo-token-laghound`).

## Routes

| Route | What it does |
|-------|--------------|
| `GET /` | returns `go sample ok` |
| `GET /work` | sleeps ~30 ms, records a `mark-work` Server-Timing mark, returns `done` |
| `/laghound/*` | the LagHound endpoint (health / echo / download / upload / info) |

## Probe it

The SDK accepts `X-LagHound-Token` or `Authorization: Bearer <token>`:

```bash
TOKEN=demo-token-laghound
curl -H "X-LagHound-Token: $TOKEN" http://localhost:8085/laghound/health
curl -H "X-LagHound-Token: $TOKEN" http://localhost:8085/laghound/echo -i        # see Server-Timing: app;dur=...
curl -H "X-LagHound-Token: $TOKEN" "http://localhost:8085/laghound/download?bytes=1048576" -o /dev/null
curl -H "X-LagHound-Token: $TOKEN" --data-binary @- http://localhost:8085/laghound/upload < somefile
curl -i http://localhost:8085/work        # your own route, carries mark-work;dur=...
```

Without the token every `/laghound/*` route is an indistinguishable `404` —
that is by design (contract §5). Set `LAGHOUND_DISABLED=1` to kill the endpoint
entirely without a redeploy.
