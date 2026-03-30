# Rust Reference API (networker-endpoint)

## Why hyper?

The Rust reference API is `networker-endpoint`, built on **hyper 1.x** via axum.
This gives raw, framework-minimal performance: hyper is the HTTP implementation
used by most Rust web frameworks, and using it directly (through the thin axum
layer) eliminates abstraction overhead. This makes it the fairest baseline for
measuring what a language runtime can achieve with minimal framework tax.

Key properties:
- **Zero-copy where possible** — download payloads are generated in-memory, no disk I/O
- **Async I/O via tokio** — epoll/kqueue-backed, no thread-per-connection
- **rustls for TLS** — no OpenSSL dependency, pure-Rust TLS with ring crypto
- **HTTP/1.1 + HTTP/2** via ALPN negotiation on a single TLS port
- **HTTP/3 (QUIC)** via Quinn, on the same port number over UDP

## Building

From the repository root:

```bash
# Debug build
cargo build -p networker-endpoint

# Release build (use this for benchmarks)
cargo build --release -p networker-endpoint

# Without HTTP/3 support
cargo build --release -p networker-endpoint --no-default-features
```

The binary is at `target/release/networker-endpoint`.

## Endpoints

All endpoints are served on both HTTP (default :8080) and HTTPS (default :8443).

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Returns `{"status":"ok"}` — used for liveness probes |
| `/download` | GET | Returns a configurable-size payload (`?size=1048576` bytes) |
| `/upload` | POST | Accepts a body, returns byte count and elapsed time |
| `/echo` | POST/GET | Echoes back the request body |
| `/delay` | GET | Returns after `?ms=100` milliseconds — measures scheduling latency |
| `/headers` | GET | Echoes all request headers back as JSON |
| `/status/:code` | GET | Returns the given HTTP status code |
| `/http-version` | GET | Reports which HTTP version was negotiated |
| `/info` | GET | Server metadata: OS, arch, CPU cores, memory, region |
| `/page` | GET | Page-load manifest (list of assets to fetch) |
| `/browser-page` | GET | Full HTML page for browser-based page-load testing |
| `/asset` | GET | Serves individual assets referenced by `/page` manifest |

## Deploying for Benchmarks

```bash
# 1. Build the release binary
cargo build --release -p networker-endpoint

# 2. Generate the shared TLS certificate (if not already done)
bash benchmarks/shared/generate-cert.sh

# 3. Deploy to a target VM
bash benchmarks/reference-apis/rust-deploy.sh user@target-vm
```

See `rust-deploy.sh` for options (custom ports, etc.).
