# C# .NET 10 Reference API

AletheBench reference implementation using ASP.NET Core minimal APIs on Kestrel.

## Why Kestrel Minimal API

Kestrel is the default, highest-performance HTTP server in the .NET ecosystem.
Minimal APIs (introduced in .NET 6) remove the MVC/controller overhead entirely,
making this the lowest-overhead way to serve HTTP from .NET. This gives .NET its
best possible showing in benchmarks — no framework tax, just Kestrel + routing.

Kestrel regularly appears in the top tier of the TechEmpower Framework Benchmarks
for plaintext and JSON workloads.

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | /health           | Runtime identity and .NET version        |
| GET    | /download/{size}  | Stream N bytes (0x42, 8 KiB chunks)      |
| POST   | /upload           | Consume body, return byte count          |

## Build

Requires .NET 10 SDK.

```bash
./build.sh
# Output: ./publish/csharp-net10 (~30-50 MB self-contained, trimmed)
```

## Run

```bash
# Direct
BENCH_CERT_DIR=. ./publish/csharp-net10

# Docker
docker build -t alethabench-csharp .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp
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

## Expected Binary Size

With `PublishTrimmed` and `InvariantGlobalization`, the self-contained binary
is typically 30-50 MB for linux-x64. This includes the .NET runtime, Kestrel,
and all dependencies — no external runtime installation needed.

## .NET Version Notes

- **net10.0** targets .NET 10 (release November 2025), the current LTS.
- `X509Certificate2.CreateFromPemFile` loads PEM certs directly (no pfx conversion).
- `InvariantGlobalization` reduces binary size by excluding ICU data.
- HTTP/2 is natively supported by Kestrel; HTTP/3 (QUIC) would require
  `HttpProtocols.Http1AndHttp2AndHttp3` and `libmsquic` on the host.
