# C# .NET 6 Reference API

AletheBench reference implementation using ASP.NET Core minimal APIs on Kestrel.

## Why .NET 6

.NET 6 is the first LTS release to support minimal APIs, introduced as a lightweight
alternative to MVC controllers. This removes the controller/routing overhead entirely,
giving .NET its best possible showing in benchmarks. .NET 6 was the first release to
unify the platform under a single SDK and was the recommended LTS for production
workloads from November 2021 through November 2024.

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | /health           | Runtime identity and .NET version        |
| GET    | /download/{size}  | Stream N bytes (0x42, 8 KiB chunks)      |
| POST   | /upload           | Consume body, return byte count          |

## Build

Requires .NET 6 SDK.

```bash
./build.sh
# Output: ./publish/csharp-net6 (~30-50 MB self-contained, trimmed)
```

## Run

```bash
# Direct
BENCH_CERT_PATH=./cert.pem BENCH_KEY_PATH=./key.pem ./publish/csharp-net6

# Docker
docker build -t alethabench-csharp-net6 .
docker run -p 8443:8443 -v /path/to/certs:/opt/bench alethabench-csharp-net6
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

- **net6.0** is the first LTS with minimal APIs (November 2021).
- `X509Certificate2.CreateFromPemFile` loads PEM certs directly (no pfx conversion).
- `InvariantGlobalization` reduces binary size by excluding ICU data.
- HTTP/2 is natively supported by Kestrel.
