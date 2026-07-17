# Python Reference API

AletheBench Python reference API using uvicorn + starlette.

Implements the frozen contract in `benchmarks/shared/API-SPEC.md` (family C):
all `/health`, `/download/{size}`, `/upload`, and `/api/*` endpoints. The
shared dataset (`bench-data.json`, spec §2) is **required** — startup fails
if it cannot be loaded. Worker policy (spec §3): `BENCH_WORKERS` maps to
uvicorn `--workers` (default = logical CPU count). uvicorn has no HTTP/3, so
the server does not advertise Alt-Svc.

## Why uvicorn + starlette?

A pure-stdlib Python HTTP server (e.g. `http.server`) is single-threaded,
blocking, and lacks async I/O -- it would be unrealistically slow and not
representative of how Python is actually deployed in production. The goal of
AletheBench is to compare runtimes under their **best realistic configuration**,
not their worst.

uvicorn + starlette is the fastest production-ready async Python stack:

- **uvicorn** is the standard ASGI server, built on `uvloop` (libuv) for
  event-loop performance close to native
- **starlette** is the lightweight ASGI framework that FastAPI is built on,
  with near-zero overhead over raw ASGI
- Together they represent the ceiling of Python HTTP performance without
  resorting to C extensions or Cython rewrites

This keeps the comparison fair: each runtime uses its idiomatic, production-grade
HTTP stack (Kestrel for .NET, built-in http2 for Node.js, uvicorn+starlette for
Python).

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | `/health`         | Runtime identity and Python version      |
| GET    | `/download/{size}`| Stream `size` bytes of 0x42 in 8 KiB chunks |
| POST   | `/upload`         | Consume request body, return byte count  |

## Local development

```bash
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt

# Without TLS (development)
uvicorn server:app --host 0.0.0.0 --port 8080

# With TLS
uvicorn server:app --host 0.0.0.0 --port 8443 \
    --ssl-keyfile /opt/bench/key.pem \
    --ssl-certfile /opt/bench/cert.pem
```

## Docker

```bash
docker build -t alethabench-python .
docker run -p 8443:8443 \
    -v /path/to/certs:/opt/bench:ro \
    alethabench-python
```

## Deploy to VM

```bash
./deploy.sh user@host --cert-dir /path/to/local/certs
```
