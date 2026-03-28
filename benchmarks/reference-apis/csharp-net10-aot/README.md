# C# .NET 10 AOT Reference API

AletheBench reference HTTP API implementation using C# with .NET 10 Native AOT compilation.

## Overview

This is the Native AOT variant of the .NET 10 reference API. It uses the same Kestrel
minimal API surface as the JIT variant but is compiled ahead-of-time into a single native
binary. No .NET runtime is required on the target machine.

### Key differences from the JIT variant

- **Single native binary** — no .NET runtime or SDK needed at deploy time
- **Faster cold start** — no JIT compilation, no assembly loading overhead
- **Smaller deployment footprint** — single file vs. self-contained publish (~150 MB)
- **AOT-compatible JSON** — uses `JsonSerializerContext` with `[JsonSerializable]`
  source-generated serializers instead of anonymous objects and reflection
- **Uses `WebApplication.CreateSlimBuilder`** — excludes AOT-incompatible middleware

### Tradeoffs

- **Must build on target OS** — Native AOT does not support cross-compilation.
  A linux-x64 binary must be built on a linux-x64 machine.
- **No dynamic code generation** — reflection-based serializers, `System.Linq.Expressions`
  compilation, and runtime code emit are not available
- **Longer build times** — AOT compilation (especially ILC linking) takes significantly
  longer than a standard `dotnet publish`

## Endpoints

| Method | Path              | Description                                    |
|--------|-------------------|------------------------------------------------|
| GET    | `/health`         | `{"status":"ok","runtime":"csharp-net10-aot","version":"..."}` |
| GET    | `/download/{size}`| Streams `size` bytes (repeating `0x42` pattern)|
| POST   | `/upload`         | Reads body, returns `{"bytes_received": N}`    |

All endpoints served over HTTPS (port 8443) with HTTP/1.1 and HTTP/2 support.

## Build

**Requires .NET 10 SDK and native toolchain (clang, zlib) on the target OS.**

```bash
# On an Ubuntu 24.04 machine:
sudo apt-get install -y clang zlib1g-dev
./build.sh
```

The AOT binary is written to `./publish/csharp-net10-aot`.

## Deploy

```bash
./deploy.sh <VM_IP>
```

Copies the single native binary and TLS certificates to `/opt/bench/` on the target VM
and starts the server.

## Docker

```bash
docker build -t alethabench-csharp-net10-aot .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp-net10-aot
```

## Expected characteristics

| Metric           | Expectation                                      |
|------------------|--------------------------------------------------|
| Binary size      | ~15-30 MB (single native binary)                 |
| Cold start       | <50 ms (no JIT, no runtime init)                 |
| Idle memory      | ~10-20 MB RSS                                    |
| Throughput       | Comparable to JIT after JIT warmup; may be       |
|                  | slightly lower due to lack of profile-guided     |
|                  | optimization at runtime                          |
