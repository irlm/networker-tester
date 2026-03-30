# C# .NET 8 AOT Reference API

AletheBench reference HTTP API implementation using C# with .NET 8 Native AOT compilation.

## Overview

This is the Native AOT variant of the .NET 8 reference API. It uses the same Kestrel
minimal API surface as the JIT variant but is compiled ahead-of-time into a single native
binary. No .NET runtime is required on the target machine.

.NET 8 was the first production-ready release with Native AOT support for ASP.NET Core.
While fully functional, it may have limitations compared to later .NET AOT releases:

- **Fewer trimming-safe APIs** — some ASP.NET Core features are not AOT-compatible in .NET 8
- **Larger binary size** — the ILC linker is less aggressive at dead-code elimination
- **No frozen object heap** — runtime startup optimizations added in later versions are absent
- **Source generator coverage** — some source generators produce less efficient code than .NET 9+

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
| GET    | `/health`         | `{"status":"ok","runtime":"csharp-net8-aot","version":"..."}` |
| GET    | `/download/{size}`| Streams `size` bytes (repeating `0x42` pattern)|
| POST   | `/upload`         | Reads body, returns `{"bytes_received": N}`    |

All endpoints served over HTTPS (port 8443) with HTTP/1.1 and HTTP/2 support.

## Build

**Requires .NET 8 SDK and native toolchain (clang, zlib) on the target OS.**

```bash
# On an Ubuntu 22.04/24.04 machine:
sudo apt-get install -y clang zlib1g-dev
./build.sh
```

The AOT binary is written to `./publish/csharp-net8-aot`.

## Deploy

```bash
./deploy.sh <VM_IP>
```

Copies the single native binary and TLS certificates to `/opt/bench/` on the target VM
and starts the server.

## Docker

```bash
docker build -t alethabench-csharp-net8-aot .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp-net8-aot
```

## Expected characteristics

| Metric           | Expectation                                      |
|------------------|--------------------------------------------------|
| Binary size      | ~20-40 MB (single native binary)                 |
| Cold start       | <50 ms (no JIT, no runtime init)                 |
| Idle memory      | ~10-25 MB RSS                                    |
| Throughput       | Comparable to JIT after JIT warmup; may be       |
|                  | slightly lower than .NET 9/10 AOT due to less    |
|                  | mature AOT optimizations                         |
