# C++ Boost.Beast Reference API

AletheBench reference HTTP server implemented in C++ using Boost.Beast and Boost.Asio with OpenSSL.

## Why Boost.Beast?

Boost.Beast is the standard C++ HTTP library built on Boost.Asio, the de facto
async I/O framework for C++. It provides the closest-to-metal HTTP
implementation available in the C++ ecosystem:

- **Zero-copy where possible** — Beast works directly with Asio buffers
- **No framework overhead** — routes are parsed manually, no middleware chain
- **Async I/O** — fully asynchronous acceptor and session handling via Asio
- **Production-grade TLS** — OpenSSL integration through Asio's SSL layer
- **Multi-threaded** — runs one I/O context across all hardware threads

The server is compiled with `-O3` (Release mode) for maximum throughput.

## Endpoints

| Method | Path              | Description                                |
|--------|-------------------|--------------------------------------------|
| GET    | `/health`         | `{"status":"ok","runtime":"cpp","version":"<__cplusplus>"}` |
| GET    | `/download/{size}`| Stream `size` bytes (0x42) in 8 KiB chunks |
| POST   | `/upload`         | Read body, return `{"bytes_received": N}`  |

## Building

**Must build on the target OS** — C++ binaries are not portable across distros.

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt-get install build-essential cmake libboost-system-dev libboost-dev libssl-dev

# Build
bash build.sh
```

## Running

```bash
# TLS certificates must exist at the default paths (or override via env vars)
BENCH_CERT_PATH=/opt/bench/cert.pem \
BENCH_KEY_PATH=/opt/bench/key.pem \
BENCH_PORT=8443 \
./build/server
```

## Docker

```bash
docker build -t alethabench-cpp .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench:ro alethabench-cpp
```

## Deploying to a VM

```bash
./deploy.sh user@host --port 8443
```

This installs build dependencies, copies source, compiles on the VM, and starts the server.
