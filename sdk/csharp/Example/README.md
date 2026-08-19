# LagHound C# sample app

A minimal ASP.NET Core app that embeds the LagHound diagnostic endpoint
(contract v1) at `/laghound`, plus a small interactive reference page and a
simulated-work route. It is the C# service used by the local multi-language
harness and the low-cost Azure reference deployment.

This project is intentionally **not** part of `Networker.sln` — it stays out of
the CI build matrix and exists for local demos and deployment targets.

## Run

```bash
export DOTNET_ROOT=$HOME/.dotnet10 PATH=$HOME/.dotnet10:$PATH   # local .NET 10 SDK
dotnet run --project sdk/csharp/Example
```

Environment:

| Var             | Default               | Meaning |
|-----------------|-----------------------|---------|
| `LAGHOUND_TOKEN`| `demo-token-laghound` | Shared secret for the LagHound routes |
| `PORT`          | `8081`                | Listen port |
| `LAGHOUND_PUBLIC_DEMO` | *(off)* | `1` disables transfer routes and tightens rate limits for a shared public demo |

## Try it

```bash
# App routes (no token needed)
open http://localhost:8081/       # interactive reference page
curl -s localhost:8081/work       # -> C# handler completed in ~30 ms

# LagHound routes (token required; without it they're a plain 404)
curl -s localhost:8081/laghound/health                                    # -> 404 (invisible)
curl -s -H "X-LagHound-Token: demo-token-laghound" localhost:8081/laghound/health
curl -s -H "Authorization: Bearer demo-token-laghound" localhost:8081/laghound/echo
curl -si -H "X-LagHound-Token: demo-token-laghound" "localhost:8081/laghound/download?bytes=1048576" | head
curl -si -H "X-LagHound-Token: demo-token-laghound" localhost:8081/laghound/info

# Kill switch: every route becomes a plain 404, no code change
LAGHOUND_DISABLED=1 dotnet run --project sdk/csharp/Example
```

Point the tester fleet at `/laghound/echo` with `--bearer-token demo-token-laghound`
to light up the network-vs-server split (see `docs/sdk/contract-v1.md` §8).

For the scale-to-zero Azure deployment, see [`examples/azure/`](../../../examples/azure/README.md).
