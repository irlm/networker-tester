# Rust Reference API (networker-endpoint)

The Rust reference implementation is the production diagnostic server
`crates/networker-endpoint` (the `rust/` entry here is a symlink to it). It is
the **canonical family C baseline**: `benchmarks/shared/API-SPEC.md` was
derived from its routes (`src/routes.rs`) plus the orchestrator transport
contract, and wave 1 brought it fully onto the frozen spec
(`/download/{size}`, constant-work `/health` with `runtime`+`version`,
0x42/8 KiB download fill, fatal `BENCH_DATA_PATH`).

## Why hyper?

Built on **hyper 1.x** via axum — framework-minimal performance and the
fairest baseline for what a language runtime can achieve:

- **Async I/O via tokio** — epoll/kqueue-backed, no thread-per-connection
- **rustls for TLS** (ring provider) — no OpenSSL dependency
- **HTTP/1.1 + HTTP/2** via ALPN on one TLS port; **HTTP/3 (QUIC)** via Quinn
  on the same port number over UDP (`http3` feature, on by default)

## Building

From the repository root:

```bash
cargo build --release -p networker-endpoint     # use this for benchmarks
cargo build --release -p networker-endpoint --no-default-features  # without HTTP/3
```

Binary: `target/release/networker-endpoint`.

## Spec endpoints (API-SPEC.md §5)

Served on HTTP (default `:8080`) and HTTPS (default `:8443`):

| Endpoint | Spec | Notes |
|----------|------|-------|
| `GET /health` | §5.1 | byte-constant `{"status":"ok","runtime":"rust","service":"networker-endpoint","version":"<crate version>"}` |
| `GET /download/{size}` | §5.2 | 0x42 fill, 8 KiB chunks, 2 GiB clamp, `X-Download-Bytes`; `GET /download?bytes=N` kept as deprecated alias |
| `POST /upload` | §5.3 | streams/drains body; `{"received_bytes":N,"timestamp":…}`, `X-Networker-Received-Bytes`, request-id echo |
| `GET /api/users` | §5.4 | bare array, 20 of the sorted 100-user window |
| `POST /api/transform` | §5.5 | `{seed, hashed_fields, reversed_values}`; invalid JSON → 400 |
| `GET /api/aggregate` | §5.6 | quintile categories; `range` accepted and ignored |
| `GET /api/search` | §5.7 | regex-then-literal, `limit` clamped to 100, `total_matches` pre-truncation |
| `POST /api/upload/process` | §5.8 | CRC-32 + SHA-256 + zlib level 6 (RFC 1950) |
| `GET /api/delayed` | §5.9 | tokio timer sleep, `ms` clamped to [1, 100] |
| `GET /api/validate` | §5.10 | echoes the dataset's `expected_checksums` |

All `/api/*` responses carry the §1 benchmark headers (`Server-Timing`,
`Cache-Control`, `Timing-Allow-Origin`, `Access-Control-Allow-Origin`).

Diagnostic extras beyond the spec (production endpoint duties): `/echo`,
`/delay`, `/headers`, `/status/{code}`, `/http-version`, `/info`, `/page`,
`/browser-page`, `/asset`, UDP echo (`:9999`) and UDP throughput (`:9998`).

## Spec knobs (§1–§3)

| Env | Behavior |
|---|---|
| `BENCH_DATA_PATH` | dataset path; **set-but-unloadable is fatal** (exit 1). Fallbacks: `/opt/bench/bench-data.json`, then `benchmarks/reference-apis/shared/bench-data.json` (repo root cwd). As the production diagnostic server, networker-endpoint is the spec's sole PRNG-fallback exception when *no* dataset exists anywhere (§2) — benchmark runs always deploy the dataset (`rust-deploy.sh` does). |
| `BENCH_API_TOKEN` | bearer auth on every route except `/health`; 401 `{"error":"unauthorized"}` |
| `BENCH_WORKERS` | the binary does not read it directly; tokio sizes its multi-thread runtime from **`TOKIO_WORKER_THREADS`** (default = logical CPUs). `rust-deploy.sh` maps `BENCH_WORKERS` → `TOKIO_WORKER_THREADS` (mapping verified against tokio 1.52.4: a runtime built with `TOKIO_WORKER_THREADS=3` reports `num_workers() == 3`). |
| `BENCH_PORT` | the binary uses `--https-port` (default 8443); `rust-deploy.sh` maps `BENCH_PORT` → `--port`. |
| `BENCH_CERT_DIR` | **not used** — documented deviation: the endpoint generates its own self-signed CA-flagged cert at startup (required for its Chrome/QUIC page-load duties). |

## Deploying for benchmarks

```bash
cargo build --release -p networker-endpoint
BENCH_WORKERS=4 bash benchmarks/reference-apis/rust-deploy.sh user@target-vm
```

(No cert step: the endpoint self-signs at startup, see `BENCH_CERT_DIR` above.)

`rust-deploy.sh` copies the binary **and** `shared/bench-data.json` to
`/opt/bench/bench-data.json` (§2 resolution path 2) and starts the server with
the mapped env knobs. See the script header for options.

## Validate

```bash
benchmarks/validate/run-validation.sh --rust-only
```

Builds the crate, starts it against the shared dataset
(`BENCH_DATA_PATH` set), and enforces the full §10 conformance checklist
including the four §7 canonical checksums (`--require-conformance` implied).
