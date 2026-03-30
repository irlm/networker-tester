# C# .NET 9 Reference API

AletheBench reference implementation using ASP.NET Core minimal APIs on Kestrel.

## Why .NET 9

.NET 9 (STS, November 2024) is the latest stable release, building on .NET 8 LTS
with further runtime and JIT improvements. Kestrel gained improved connection handling,
and the runtime received loop optimizations, improved bounds check elimination, and
Arm64 code generation enhancements. The garbage collector added dynamic adaptation to
application size (DATAS) for better memory management in server workloads.

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | /health           | Runtime identity and .NET version        |
| GET    | /download/{size}  | Stream N bytes (0x42, 8 KiB chunks)      |
| POST   | /upload           | Consume body, return byte count          |

## Build

Requires .NET 9 SDK.

```bash
./build.sh
# Output: ./publish/csharp-net9 (~30-50 MB self-contained, trimmed)
```

## Run

```bash
# Direct
BENCH_CERT_DIR=. ./publish/csharp-net9

# Docker
docker build -t alethabench-csharp-net9 .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp-net9
```

## Deploy

```bash
./deploy.sh user@vm-host --cert-dir /path/to/certs
```

## Configuration

| Environment Variable | Default             | Description          |
|---------------------|---------------------|----------------------|
| BENCH_CERT_DIR      | /opt/bench          | Directory containing cert.pem and key.pem |
| BENCH_PORT          | 8443                | Listen port          |

## .NET Version Notes

- **net9.0** is the latest stable STS release (November 2024).
- Loop optimizations and improved bounds check elimination in the JIT.
- DATAS (Dynamic Adaptation To Application Sizes) in the GC for server workloads.
- `InvariantGlobalization` reduces binary size by excluding ICU data.
- HTTP/2 is natively supported by Kestrel; HTTP/3 available with `libmsquic`.
