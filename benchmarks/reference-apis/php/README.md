# Networker Bench — PHP Reference API

PHP async HTTP server powered by Swoole.

Implements the frozen contract in `benchmarks/shared/API-SPEC.md` (family C):
all `/health`, `/download/{size}`, `/upload`, and `/api/*` endpoints. The
shared dataset (`bench-data.json`, spec §2) is **required** — startup fails
if it cannot be loaded. Worker policy (spec §3): `BENCH_WORKERS` maps to
Swoole `worker_num` (default = logical CPU count). `/api/delayed` uses a
coroutine sleep, not `usleep`, so it never blocks a worker.

## Why Swoole?

Swoole is the only production-ready async HTTP server for PHP. Traditional PHP
deployment models (Apache mod_php, PHP-FPM behind nginx) involve a separate
web server that proxies to PHP workers, adding latency and complexity that would
conflate the benchmark with the proxy layer rather than measuring PHP itself.

Swoole runs PHP as a **long-lived, event-driven process** with built-in HTTP
server, TLS, and coroutine support. This is the closest PHP gets to the
single-process, async server model used by Go, Node.js, and Rust, making it
the fairest comparison point for Networker Bench.

**Linux only**: Swoole requires Linux and does not support macOS or Windows.
This is acceptable for Networker Bench since all benchmark targets are Linux VMs.

## Endpoints

| Method | Path              | Description                               |
|--------|-------------------|-------------------------------------------|
| GET    | /health           | JSON health check with PHP version        |
| GET    | /download/{size}  | Stream `size` bytes (0x42, 8 KiB chunks)  |
| POST   | /upload           | Consume body, return bytes received       |

All endpoints served over TLS on port 8443.

## Local development (Linux only)

```bash
# Install Swoole
pecl install swoole

# Without TLS: if $BENCH_CERT_DIR/cert.pem + key.pem are absent the server
# falls back to plain HTTP on the same port (application mode behind a
# TLS-terminating reverse proxy)
php server.php

# With TLS
BENCH_CERT_DIR=/path/to/certs php server.php
```

## Docker

```bash
docker build -t alethabench-php .
docker run -p 8443:8443 \
    -v /path/to/certs:/opt/bench:ro \
    alethabench-php
```

## Deploy to VM

```bash
./deploy.sh user@host --cert-dir /path/to/local/certs
```
