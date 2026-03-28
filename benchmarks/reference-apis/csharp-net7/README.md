# C# .NET 7 Reference API

AletheBench reference implementation using ASP.NET Core minimal APIs on Kestrel.

## Why .NET 7

.NET 7 (STS, November 2022) brought significant performance improvements over .NET 6,
including faster HTTP/2 handling in Kestrel, built-in rate limiting middleware, output
caching, and improved minimal API features like endpoint filters. The JIT compiler
received on-stack replacement (OSR) improvements and dynamic PGO enhancements that
benefit long-running server workloads.

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | /health           | Runtime identity and .NET version        |
| GET    | /download/{size}  | Stream N bytes (0x42, 8 KiB chunks)      |
| POST   | /upload           | Consume body, return byte count          |

## Build

Requires .NET 7 SDK.

```bash
./build.sh
# Output: ./publish/csharp-net7 (~30-50 MB self-contained, trimmed)
```

## Run

```bash
# Direct
BENCH_CERT_PATH=./cert.pem BENCH_KEY_PATH=./key.pem ./publish/csharp-net7

# Docker
docker build -t alethabench-csharp-net7 .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp-net7
```

## Deploy

```bash
./deploy.sh user@vm-host --cert-dir /path/to/certs
```

## Configuration

| Environment Variable | Default             | Description          |
|---------------------|---------------------|----------------------|
| BENCH_CERT_PATH     | /opt/bench/cert.pem | TLS certificate path |
| BENCH_KEY_PATH      | /opt/bench/key.pem  | TLS private key path |
| BENCH_PORT          | 8443                | Listen port          |

## .NET Version Notes

- **net7.0** is an STS release with performance improvements and rate limiting (November 2022).
- Kestrel HTTP/2 throughput improvements over .NET 6.
- Dynamic PGO and on-stack replacement enhancements benefit server workloads.
- `InvariantGlobalization` reduces binary size by excluding ICU data.
