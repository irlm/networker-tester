# C# .NET 8 Reference API

AletheBench reference implementation using ASP.NET Core minimal APIs on Kestrel.

## Why .NET 8

.NET 8 (LTS, November 2023) is the current long-term support release with significant
performance improvements across the board. Kestrel received named pipe transport,
HTTP/2 and HTTP/3 improvements, and reduced memory allocations. The JIT compiler
gained dynamic PGO enabled by default, producing faster steady-state throughput. Native
AOT support expanded to ASP.NET Core, and `RequestDelegateGenerator` source-generates
minimal API handlers at compile time for zero-reflection overhead.

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | /health           | Runtime identity and .NET version        |
| GET    | /download/{size}  | Stream N bytes (0x42, 8 KiB chunks)      |
| POST   | /upload           | Consume body, return byte count          |

## Build

Requires .NET 8 SDK.

```bash
./build.sh
# Output: ./publish/csharp-net8 (~30-50 MB self-contained, trimmed)
```

## Run

```bash
# Direct
BENCH_CERT_PATH=./cert.pem BENCH_KEY_PATH=./key.pem ./publish/csharp-net8

# Docker
docker build -t alethabench-csharp-net8 .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp-net8
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

- **net8.0** is the current LTS with significant performance improvements (November 2023).
- Dynamic PGO enabled by default for better steady-state throughput.
- `RequestDelegateGenerator` source-generates minimal API handlers at compile time.
- `InvariantGlobalization` reduces binary size by excluding ICU data.
- HTTP/2 is natively supported by Kestrel; HTTP/3 available with `libmsquic`.
