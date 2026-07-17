# nginx Static-Server Baseline

This is **not** a real API implementation. It is the infrastructure baseline:
pure network stack, kernel `sendfile`, zero application code — the ceiling no
application server can exceed on the same hardware.

Contract: `benchmarks/shared/API-SPEC.md` (frozen v1). Per §9, nginx serves
**only** the transport endpoints and never implements the `/api/*` compute
workloads.

## Capability profile

| Endpoint / mode | Served | Notes |
|---|---|---|
| `GET /health` (§5.1) | yes | `{"status":"ok","runtime":"nginx","version":"<nginx version>"}` — byte-constant per process, auth-exempt |
| `GET /download/{size}` (§5.2) | partial | pre-generated 0x42 files only (`0, 1024, 8192, 65536, 1048576, 10485760`); other sizes → 404 JSON; non-integer → 400 JSON; `X-Download-Bytes` + `Server-Timing` sent |
| `POST /upload` (§5.3) | partial | body drained at line rate (lingering close, up to 600 s); `received_bytes` echoes `Content-Length` (chunked → `0`); `X-Networker-Received-Bytes` + `X-Networker-Request-Id` echo sent |
| `GET/POST /api/*` (§5.4–5.10) | **no** | `501` JSON with the §1 benchmark headers — **exclude nginx from apibench** (audit C5) |
| HTTP/1.1 + HTTP/2 | yes | TLS on 8443 |
| HTTP/3 (QUIC) | yes | requires nginx **mainline 1.25+** — installed from nginx.org by `deploy.sh`, the orchestrator, and `install.sh` alike; `Alt-Svc` advertised |
| Errors | JSON | every error status returns `{"error":"…"}` (§1: no HTML error pages) |

## Spec knobs (§1, §3)

nginx reads a config file, not env vars, and the orchestrator / `install.sh`
copy `nginx.conf` verbatim. The knobs are therefore applied as **line edits**
of `nginx.conf` by `deploy.sh` (remote sed) and `docker-entrypoint-bench.sh`:

| Env | Maps to | Default |
|---|---|---|
| `BENCH_WORKERS` | `worker_processes` | `auto` (= all logical CPUs, the §3 default) |
| `BENCH_PORT` | `listen` + `Alt-Svc` port | `8443` |
| `BENCH_CERT_DIR` | `ssl_certificate`/`_key` dir (Docker entrypoint only) | `/opt/bench` |
| `BENCH_API_TOKEN` | bearer auth on all routes except `/health` (401 JSON otherwise) | disabled |

Deploy paths that copy the config verbatim (orchestrator, `install.sh`) get
the defaults; pinned-worker runs (`BENCH_WORKERS=1`) must deploy via
`deploy.sh` or Docker.

## Honest limitations (documented, by design)

- `/download/{size}` works only for pre-generated sizes — edit
  `generate-download-files.sh` to add more. There is no 2 GiB clamp behavior:
  non-pre-generated sizes are 404, not clamped.
- `/upload` cannot count drained bytes (no app code): `received_bytes` echoes
  the request `Content-Length`; chunked-encoding uploads report `0`.
- `Server-Timing` durations are constant `0.0` — there is no application
  handler to time.
- `/health` results must never be ranked (spec §4); the `/api/*` 501s mean
  nginx appears in throughput/transport comparisons only.

## Docker

```bash
docker build -t bench-nginx benchmarks/reference-apis/nginx
docker run --rm -p 8443:8443 -p 8443:8443/udp bench-nginx          # baked self-signed cert
# with the shared benchmark certs (mount as files — do NOT mount a volume
# over /opt/bench, it would hide the pre-generated download payloads):
docker run --rm -p 8443:8443 \
  -v "$PWD/benchmarks/shared/cert.pem:/opt/bench/cert.pem:ro" \
  -v "$PWD/benchmarks/shared/key.pem:/opt/bench/key.pem:ro" \
  -e BENCH_WORKERS=1 bench-nginx
```

The shared `bench-data.json` mount used by the app-language images is not
applicable here (no `/api/*` workloads).

## Deploy to a VM

```bash
# 1. Generate the shared TLS certificate (one-time)
bash benchmarks/shared/generate-cert.sh

# 2. Deploy (installs nginx mainline from nginx.org for http2/http3)
BENCH_WORKERS=auto bash benchmarks/reference-apis/nginx/deploy.sh user@hostname
```

## Validate

```bash
benchmarks/validate/run-validation.sh --url=https://<host>:8443 --name=nginx
```

`/health`, `/download/1024` and the header checks pass; the `/api/*` and
checksum checks report conformance failures by design — nginx is the
transport-only baseline (§9) and is excluded from apibench.
