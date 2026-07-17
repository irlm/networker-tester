# AletheBench — Node.js Reference API

HTTP/2 diagnostic server for cross-language network benchmarking.

## Why built-in `http2` (no Express, no dependencies)

AletheBench compares runtime performance across languages. Each reference API
must measure the **language runtime and standard library**, not a third-party
framework. Using Node's built-in `http2` module ensures:

- **Zero framework overhead** — no middleware chains, no routing libraries.
  The request dispatcher is a single function with string comparisons.
- **Apples-to-apples comparison** — Go uses `net/http`, Rust uses `hyper`
  (the de-facto stdlib-level HTTP crate), Python uses `asyncio`. Node's
  `http2` is the equivalent built-in primitive.
- **No dependency supply-chain risk** — `package.json` has an empty
  `dependencies` block. Nothing to audit, nothing to update.
- **Reproducible builds** — the Docker image copies two files. No
  `node_modules`, no lockfile churn.

## Contract

Implements the frozen contract in `benchmarks/shared/API-SPEC.md` (family C):
`/health` (byte-constant), `/download/{size}` (0x42 fill, 8 KiB chunks),
`/upload`, and the seven `/api/*` JSON endpoints, all served from the shared
dataset `bench-data.json` (**load failure is fatal** — no PRNG fallback).

## Worker policy (API-SPEC.md §3)

Scaling uses the `cluster` module: the primary forks `BENCH_WORKERS` worker
processes (default = logical CPU count) sharing the listen port.

| Variable         | Default     | Description                                 |
|------------------|-------------|---------------------------------------------|
| `BENCH_PORT`     | `8443`      | Listen port                                 |
| `BENCH_CERT_DIR` | `/opt/bench`| PEM cert/key directory (absent → plain HTTP)|
| `BENCH_WORKERS`  | CPU count   | Cluster worker processes (§3)               |
| `BENCH_API_TOKEN`| unset       | Bearer token for all routes except `/health`|
| `BENCH_DATA_PATH`| unset       | Explicit dataset path (failure → exit 1)    |

## Tests

```bash
node test.js   # pure-logic unit tests against the shared dataset
```

## Running locally

```bash
# Generate self-signed certs (if you don't have them)
mkdir -p /opt/bench
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout /opt/bench/key.pem -out /opt/bench/cert.pem \
  -days 365 -nodes -subj "/CN=localhost"

# Start
BENCH_CERT_DIR=/opt/bench PORT=8443 node server.js
```

## Docker

```bash
docker build -t alethabench-nodejs .
docker run -v /opt/bench:/opt/bench -p 8443:8443 alethabench-nodejs
```

## Deploy to VM

```bash
sudo bash deploy.sh --cert-dir /opt/bench --port 8443
```
