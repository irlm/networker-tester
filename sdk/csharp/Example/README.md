# LagHound C# sample app

A minimal ASP.NET Core app that embeds the LagHound diagnostic endpoint
(contract v1) at `/laghound`, plus two ordinary app routes so you can see
pass-through and the 404-invisibility of the LagHound routes.

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

## Try it

```bash
# App routes (no token needed)
curl -s localhost:8081/           # -> csharp sample ok
curl -s localhost:8081/work       # -> worked for ~30ms

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
