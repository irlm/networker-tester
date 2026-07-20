# LagHound Python sample

A minimal "real service" (stdlib `wsgiref`, no framework) with the LagHound
diagnostic endpoint mounted at `/laghound`, so a LagHound fleet can probe it.

```bash
PORT=8083 LAGHOUND_TOKEN=demo-token-laghound python3 app.py
```

- `GET /` → `python sample ok` (the app's own route — LagHound never touches it)
- `GET /work` → ~30 ms of simulated processing
- `GET /laghound/health` → `404` without the token (scanner-invisible), the
  JSON health envelope with `X-LagHound-Token: demo-token-laghound`
- `GET /laghound/echo|download|upload|info` per the [contract](../../../docs/sdk/contract-v1.md)

This is port **8083** in the combined multi-language deployment
(`examples/` at the repo root runs all five language samples on one target).
