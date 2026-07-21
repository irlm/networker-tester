# LagHound multi-language deploy harness

Runs **all five** LagHound SDK sample services — C#, JS, Python, Rust, Go — on
**one** target, each mounting the LagHound diagnostic endpoint (contract v1) at
`/laghound` behind a shared token. A LagHound fleet can then probe every
language from a single host.

This does double duty:

- **Cross-language conformance demo** — one `probe-all.sh` run asserts the
  contract (200 + `contract:v1` with the token, bare 404 without it) across all
  five implementations at once.
- **Sales demo** — "LagHound works in every language," live, on one box.

## Quick start

```bash
cd examples
cp .env.example .env          # optional; sets LAGHOUND_TOKEN (default demo-token-laghound)
docker compose up --build     # build + start all five services

# in another shell:
./probe-all.sh                # PASS/FAIL table across all five languages
```

Each sample exposes three routes (per the [SDK contract](../docs/sdk/README.md)):

| Route          | What it is                                                    |
|----------------|--------------------------------------------------------------|
| `GET /`        | the app's own liveness route (`<lang> sample ok`)            |
| `GET /work`    | ~30 ms of simulated work (realistic server-side split)       |
| `/laghound/*`  | the LagHound endpoint — `health`/`echo`/`download`/`upload`/`info`, token-gated |

## Port map

| Language | Host port | Container | Base image                          |
|----------|-----------|-----------|-------------------------------------|
| C#       | **8081**  | `laghound-csharp`  | `mcr.microsoft.com/dotnet/aspnet:10.0` |
| JS/Node  | **8082**  | `laghound-js`      | `node:22-slim`                      |
| Python   | **8083**  | `laghound-python`  | `python:3.12-slim`                  |
| Rust     | **8084**  | `laghound-rust`    | `debian:trixie-slim` (multi-stage `rust:1-slim`) |
| Go       | **8085**  | `laghound-go`      | `debian:trixie-slim` (multi-stage `golang:1.26`) |

All five share `LAGHOUND_TOKEN` (from `.env`, default `demo-token-laghound`),
`restart: unless-stopped`, and a healthcheck against their own `GET /` route.
Each Dockerfile builds from the **repo root** as context so it can `COPY sdk/<lang>`.

## Try it by hand

```bash
TOKEN=demo-token-laghound
# With the token -> 200 + JSON health envelope
curl -H "X-LagHound-Token: $TOKEN" localhost:8081/laghound/health   # csharp
curl -H "X-LagHound-Token: $TOKEN" localhost:8085/laghound/health   # go
# Without the token -> bare 404 (invisible, identical to a route that isn't there)
curl -i localhost:8083/laghound/health                              # python
```

## Point a LagHound target at these five endpoints

Each `/laghound/echo` (and the transfer routes) is a valid tester target. Give
the fleet the five URLs plus the bearer token — the SDK accepts
`Authorization: Bearer <token>` as an equivalent of `X-LagHound-Token`:

```
http://<host>:8081/laghound/echo    # csharp
http://<host>:8082/laghound/echo    # js
http://<host>:8083/laghound/echo    # python
http://<host>:8084/laghound/echo    # rust
http://<host>:8085/laghound/echo    # go
```

Existing tester modes work today (spec §8): `http1/2/3` and `curl` against
`{prefix}/echo`, `webdownload`/`webupload` against the transfer routes — all
with `--bearer-token <token>`. See [`docs/sdk/README.md`](../docs/sdk/README.md)
for the mode → route mapping and the pending dedicated `sdkprobe` mode.

## Cross-language conformance demo

`probe-all.sh` is the one-command proof. For each service it asserts:

1. `GET /laghound/health` **with** the token → HTTP 200 and `"contract":"v1"`.
2. `GET /laghound/health` **without** the token → HTTP 404 (invisibility).

It prints a PASS/FAIL table and exits non-zero on any failure — usable as a
smoke gate after `docker compose up`:

```bash
HOST=1.2.3.4 ./probe-all.sh    # probe a remote VM running this harness
```

The per-SDK unit conformance suites (pinned to
[`shared/sdk-contract-v1.json`](../shared/sdk-contract-v1.json)) run in CI via
[`.github/workflows/sdk-conformance.yml`](../.github/workflows/sdk-conformance.yml);
this harness is the *live, cross-process, cross-language* complement.

## Deploying to a single cloud VM

See [`deploy-to-vm.md`](deploy-to-vm.md) for Azure/AWS/GCP steps (install Docker,
clone, `compose up`, open ports 8081-8085 or reverse-proxy them).
