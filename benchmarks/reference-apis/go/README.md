# AletheBench — Go Reference API

Go net/http implementation of the AletheBench reference API contract.

## Why net/http stdlib

Go's standard library HTTP server is production-grade out of the box. Unlike most
languages where the stdlib HTTP server is a toy, Go's `net/http` is what powers
production services at scale. Adding a framework (Gin, Echo, Fiber) would add
dependency weight and abstraction overhead without meaningful capability gains.
This makes Go unique among the benchmark candidates: **the stdlib _is_ the
idiomatic choice**.

## Endpoints

| Method | Path              | Description                               |
|--------|-------------------|-------------------------------------------|
| GET    | /health           | JSON health check with Go runtime version |
| GET    | /download/{size}  | Stream `size` bytes (0x42, 8 KiB chunks)  |
| POST   | /upload           | Consume body, return bytes received        |

All endpoints served over TLS on port 8443.

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

| Variable    | Default | Description           |
|-------------|---------|-----------------------|
| LISTEN_ADDR | :8443   | Address to listen on  |

TLS certificates are read from `/opt/bench/cert.pem` and `/opt/bench/key.pem`.
