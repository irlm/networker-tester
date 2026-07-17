# Networker Bench Java Reference API

HTTPS server for Networker Bench using JDK 21 built-in APIs only.

## Why JDK Built-in HttpsServer

This implementation deliberately avoids frameworks like Spring Boot, Netty, or
Tomcat. The goal is to measure the Java platform itself:

- **No framework overhead** — `com.sun.net.httpserver.HttpsServer` is part of the
  JDK. There is no classpath scanning, dependency injection, or annotation
  processing at startup. Cold-start time reflects the JVM, not a framework.
- **Minimal footprint** — the compiled JAR is a few kilobytes. Memory usage at
  idle is the JVM baseline, not inflated by connection pools and caches that
  frameworks allocate eagerly.
- **Reproducible** — the only dependency is the JDK itself. No build tool
  (Maven/Gradle) is required, no transitive dependency graph to audit.

## Worker policy (API-SPEC.md §3)

The `HttpServer` executor is a **fixed thread pool** sized by `BENCH_WORKERS`
(default = logical CPU count) — the one knob every reference implementation
shares. `/api/delayed` completes from a scheduler thread, so the timer never
blocks a pool worker.

## Contract

Implements the frozen contract in `benchmarks/shared/API-SPEC.md` (family C):
`/health` (byte-constant), `/download/{size}` (0x42 fill, 8 KiB chunks),
`/upload`, and the seven `/api/*` JSON endpoints, all served from the shared
dataset `bench-data.json` (**load failure is fatal** — no PRNG fallback).
JSON parsing/serialization uses a complete recursive-descent implementation
(the old hand-rolled scanner broke on escaped quotes — audit F5).

All endpoints listen on port 8443 (HTTPS; plain HTTP when no certs are found).

## Quick Start

```bash
# Build
./build.sh

# Run locally (needs cert.pem + key.pem in /opt/bench or set BENCH_CERT_DIR)
BENCH_CERT_DIR=../../shared java -jar server.jar

# Docker
docker build -t alethabench-java .
docker run -v /opt/bench:/opt/bench -p 8443:8443 alethabench-java

# Deploy to VM
./deploy.sh user@host
```

## Requirements

- JDK 21+
- TLS certificate and key in PEM format at `$BENCH_CERT_DIR` (default `/opt/bench`)
- `bench-data.json` at `$BENCH_DATA_PATH`, `/opt/bench/bench-data.json`, or
  `../shared/bench-data.json` (required — startup is fatal without it)

| Variable        | Default    | Description                                |
|-----------------|------------|--------------------------------------------|
| `BENCH_PORT`    | `8443`     | Listen port                                |
| `BENCH_CERT_DIR`| `/opt/bench`| PEM cert/key directory (absent → plain HTTP)|
| `BENCH_WORKERS` | CPU count  | Fixed thread-pool size (§3)                |
| `BENCH_API_TOKEN`| unset     | Bearer token for all routes except `/health`|
| `BENCH_DATA_PATH`| unset     | Explicit dataset path (failure → exit 1)   |
