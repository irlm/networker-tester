# Networker Bench — Go Reference API

Go net/http implementation of the Networker Bench reference API contract.

## Why net/http stdlib

Go's standard library HTTP server is production-grade out of the box. Unlike most
languages where the stdlib HTTP server is a toy, Go's `net/http` is what powers
production services at scale. Adding a framework (Gin, Echo, Fiber) would add
dependency weight and abstraction overhead without meaningful capability gains.
This makes Go unique among the benchmark candidates: **the stdlib _is_ the
idiomatic choice**.

## Contract

Implements the frozen contract in `benchmarks/shared/API-SPEC.md` (family C):
`/health` (byte-constant), `/download/{size}` (0x42 fill, 8 KiB chunks, 2 GiB
clamp), `/upload`, and the seven `/api/*` JSON endpoints, all served from the
shared dataset `bench-data.json` (**load failure is fatal** — no PRNG
fallback). Worker policy (§3): `BENCH_WORKERS` maps to `GOMAXPROCS`
(default = logical CPU count).

All endpoints served over TLS on port 8443 (plain HTTP when no certs found).

## Build

```bash
./build.sh
```

Produces a **static, zero-dependency Linux binary** via cross-compilation.
Go's built-in cross-compiler (`GOOS=linux GOARCH=amd64 CGO_ENABLED=0`) means
no Docker, no VM, no toolchain install — just `go build`. The resulting binary
runs on any Linux without libc or runtime dependencies.

Typical binary size: ~7 MB.

## Deploy

```bash
./deploy.sh <host> [ssh-key]
```

Copies the static binary and TLS certs to `/opt/bench/` on the target host.

## Docker

```bash
# Generate certs first
../../shared/generate-cert.sh
docker build -t alethabench-go .
docker run -p 8443:8443 -v /opt/bench:/opt/bench alethabench-go
```

## Configuration

| Variable         | Default      | Description                                 |
|------------------|--------------|---------------------------------------------|
| LISTEN_ADDR      | :8443        | Address to listen on                        |
| BENCH_PORT       | 8443         | Port (when LISTEN_ADDR unset)               |
| BENCH_CERT_DIR   | /opt/bench   | PEM cert/key directory (absent → plain HTTP)|
| BENCH_WORKERS    | CPU count    | GOMAXPROCS (API-SPEC.md §3)                 |
| BENCH_API_TOKEN  | unset        | Bearer token for all routes except /health  |
| BENCH_DATA_PATH  | unset        | Explicit dataset path (failure → exit 1)    |

TLS certificates are read from `$BENCH_CERT_DIR/cert.pem` + `key.pem`.
`bench-data.json` is required at `$BENCH_DATA_PATH`, `/opt/bench/bench-data.json`,
or `../shared/bench-data.json` — startup is fatal without it.
