# Networker Tester

A cross-platform network diagnostics suite that exercises an endpoint using TCP,
HTTP/1.1, HTTP/2, HTTP/3 (optional), UDP, and bulk download/upload throughput
probes, collecting detailed per-phase telemetry from user-mode code.

```
┌──────────────────────────────────────────────────────────────────┐
│  networker-tester  ──────────────────────►  networker-endpoint   │
│  (Rust CLI)          TCP / HTTP1 / HTTP2    (Rust server)        │
│                      HTTP3 (QUIC, optional)                      │
│                      UDP echo                                    │
│                      Download / Upload (throughput)              │
│         │                                                        │
│         ▼                                                        │
│  JSON artifact + HTML report + SQL Server inserts                │
└──────────────────────────────────────────────────────────────────┘
```

---

## Contents

- [Installation](#installation)
- [Prerequisites](#prerequisites)
- [Quick Start (30 minutes)](#quick-start)
- [CLI Reference](#cli-reference)
- [Throughput Testing](#throughput-testing)
- [Endpoint Reference](#endpoint-reference)
- [SQL Server Setup](#sql-server-setup)
- [HTML Report](#html-report)
- [Running Tests](#running-tests)
- [Metrics Captured](#metrics-captured)
- [Known Limitations](#known-limitations)
- [Design Decisions](#design-decisions)

---

## Installation

> **Requirement:** your machine must have an SSH key configured for GitHub
> (`ssh -T git@github.com` should respond *"Hi \<user\>! You've successfully authenticated…"*).
> The source repository is private; the installer pulls and compiles it using your existing key —
> no personal access token needed.

### macOS and Linux

Install the **tester** (diagnostic CLI client):
```bash
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- tester
```

Install the **endpoint** (target test server — run on the machine you want to probe):
```bash
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- endpoint
```

Or download and run locally:
```bash
bash install.sh tester
bash install.sh endpoint
```

### Windows (PowerShell)

Install the **tester**:
```powershell
Invoke-RestMethod https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1 | Invoke-Expression
```

Install the **endpoint**:
```powershell
.\install.ps1 -Component endpoint
```

> **Note:** Git for Windows must be installed (bundles `ssh.exe`).
> Download from <https://git-scm.com/>.

### What the installer does

1. Verifies SSH access to GitHub (`ssh -T git@github.com`).
2. Installs Rust via [rustup](https://rustup.rs/) if `cargo` is not already present.
3. Runs `cargo install --git ssh://git@github.com/irlm/networker-tester --bin <binary> --locked`
   to compile and install the binary from the private repository.
4. Prints the installed path and a quick-start command.

Compilation takes 2–5 minutes on first run (all dependencies are downloaded and compiled).
Subsequent installs or upgrades are faster because cargo caches compiled artifacts.

### Upgrading

Re-run the same install command. `cargo install` replaces the binary in-place.

---

## Prerequisites

### All platforms
| Tool | Purpose | Version |
|------|---------|---------|
| Rust stable | Build toolchain | ≥ 1.80 |
| cargo | Package manager | (bundled with Rust) |
| SQL Server | Storage layer | 2017+ or Azure SQL (optional) |

### Linux extras
```bash
sudo apt-get install -y build-essential pkg-config libssl-dev
```

### macOS extras
```bash
xcode-select --install
```

### Windows extras
- Visual Studio Build Tools 2022 (C++ workload) **or** MSVC
- Or use [rustup on Windows](https://rustup.rs/) with the `x86_64-pc-windows-msvc` target

---

## Quick Start

### 1. Clone and build

```bash
git clone git@github.com:irlm/networker-tester.git
cd networker-tester
cargo build --release
```

Binaries are written to:
- `target/release/networker-endpoint`
- `target/release/networker-tester`

---

### 2. Start the endpoint

```bash
# Default: HTTP :8080, HTTPS :8443 (self-signed), UDP :9999
./target/release/networker-endpoint

# Custom ports
./target/release/networker-endpoint \
  --http-port 9080 \
  --https-port 9443 \
  --udp-port 9999
```

The endpoint generates a self-signed TLS certificate on first run (no
pre-generated certs needed). Pass `--insecure` to the tester to accept it.

---

### 3. Run the tester

**Basic run (HTTP/1.1 + HTTP/2 + UDP):**
```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,udp \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

**All modes including raw TCP probe:**
```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes tcp,http1,http2,udp \
  --runs 3 \
  --insecure \
  --output-dir ./output \
  --verbose
```

**Save to SQL Server:**
```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,udp \
  --runs 3 \
  --insecure \
  --save-to-sql \
  --connection-string "Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=YourPass1!;TrustServerCertificate=true" \
  --output-dir ./output
```

**Throughput measurement (download + upload):**
```bash
./target/release/networker-tester \
  --target http://127.0.0.1:8080/health \
  --modes download,upload \
  --payload-sizes 4k,64k,1m \
  --runs 3 \
  --output-dir ./output
```

**HTTP/3 (requires `--features http3`):**
```bash
cargo build --release --features http3
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http3 \
  --insecure
```

---

### 4. View outputs

```
output/
├── report.html          ← Open in browser
├── report.css           ← Stylesheet (auto-copied)
└── run-20240115-100000.json   ← Raw metrics
```

Open `output/report.html` in any browser for the formatted dashboard.

---

## CLI Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--target` | `http://localhost:8080/health` | URL to probe |
| `--modes` | `http1,http2,udp` | Comma-separated: `tcp,http1,http2,http3,udp,download,upload` |
| `--runs` | `3` | Repetitions per mode |
| `--concurrency` | `1` | Concurrent requests per run |
| `--timeout` | `30` | Per-request timeout (seconds) |
| `--payload-size` | `0` | POST body size (bytes) for `/echo` tests |
| `--payload-sizes` | — | Sizes for download/upload probes, comma-separated with k/m/g suffix (e.g. `4k,64k,1m`). Required when using `download` or `upload` modes. |
| `--udp-port` | `9999` | UDP echo port on target host |
| `--udp-probes` | `10` | UDP probe packets per run |
| `--dns-enabled` | `true` | Perform DNS resolution |
| `--ipv4-only` | — | Restrict to IPv4 |
| `--ipv6-only` | — | Restrict to IPv6 |
| `--insecure` | — | Skip TLS certificate verification |
| `--output-dir` | `./output` | Directory for JSON + HTML |
| `--html-report` | `report.html` | HTML filename |
| `--css` | `report.css` | CSS `<link>` href |
| `--save-to-sql` | — | Insert into SQL Server |
| `--connection-string` | `$NETWORKER_SQL_CONN` | ADO.NET connection string |
| `--verbose` / `-v` | — | Enable debug logging |

---

## Throughput Testing

The `download` and `upload` modes measure raw transfer speed rather than
latency. They complement the standard HTTP probes by quantifying how fast
the link can move data.

### How it works

| Mode | HTTP verb | Endpoint | Measures |
|------|-----------|----------|---------|
| `download` | GET | `/download?bytes=N` | Server → client transfer speed |
| `upload` | POST | `/upload` | Client → server transfer speed |

Throughput is computed from body-transfer time only (total response time minus
TTFB), so connection setup and time-to-first-byte do not inflate the result:

```
throughput (MB/s) = payload_bytes / (total_ms − ttfb_ms) × 1000 / 1,048,576
```

### Usage

`--payload-sizes` is **required** when either mode is active. It accepts
comma-separated values with optional `k` / `m` / `g` suffixes:

```bash
./target/release/networker-tester \
  --target http://127.0.0.1:8080/health \
  --modes download,upload \
  --payload-sizes 4k,64k,1m \
  --runs 5 \
  --output-dir ./output
```

This produces `3 payload sizes × 2 modes × 5 runs = 30` probe attempts.

### Combining with latency modes

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,download,upload \
  --payload-sizes 64k,1m \
  --runs 3 \
  --insecure \
  --output-dir ./output
```

### SQL Server — existing database migration

If you already have the `NetworkDiagnostics` schema from a previous release,
apply the migration to add the two new columns:

```bash
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/04_AddThroughput.sql
```

The migration is **idempotent** — safe to run on a fresh database or one that
already has the columns. Existing rows are unaffected (columns default to NULL).

### Output

Throughput results appear in:
- **HTML report** — dedicated "Throughput Results" table (Run #, Mode, Payload,
  Throughput MB/s, TTFB, Total) and in the "All Attempts" table
- **JSON artifact** — `http.payload_bytes` and `http.throughput_mbps` fields
  on each download/upload attempt (absent/null on normal probes for backward
  compatibility)
- **SQL Server** — `PayloadBytes` and `ThroughputMbps` columns in `dbo.HttpResult`

---

## Endpoint Reference

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Returns `{"status":"ok"}` |
| `/echo` | POST | Echoes request body verbatim |
| `/echo` | GET | Returns header info |
| `/download?bytes=N` | GET | Returns N zero bytes (max 100 MiB) |
| `/upload` | POST | Accepts body; returns `{received_bytes}` |
| `/delay?ms=N` | GET | Sleeps N ms (max 30 s) then responds |
| `/headers` | GET | Returns all received headers as JSON |
| `/status/:code` | GET | Returns the given HTTP status code |
| `/http-version` | GET | Returns the negotiated HTTP version |
| `/info` | GET | Server capabilities |
| UDP `:9999` | datagram | Echo server (reflects packet back) |

**HTTP/3 support on the endpoint:**
The endpoint uses `axum-server` which supports HTTP/1.1 and HTTP/2 via ALPN. HTTP/3 (QUIC) requires an additional listener; compile with `--features http3` (both client and endpoint) to enable experimental support.

---

## SQL Server Setup

### Option A: Docker (quickest for local dev)

```bash
docker run --name networker-sql \
  -e ACCEPT_EULA=Y \
  -e SA_PASSWORD="YourPass1!" \
  -p 1433:1433 \
  -d mcr.microsoft.com/mssql/server:2022-latest
```

### Option B: Existing SQL Server

Ensure you have a login with `db_owner` on the target database.

### Create schema (fresh install)

```bash
# Linux/macOS (sqlcmd from mssql-tools)
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/02_StoredProcedures.sql

# Windows (sqlcmd in PATH after SQL Server install)
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql\01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql\02_StoredProcedures.sql
```

### Migrate an existing database

If you installed an earlier version, apply the throughput migration to add the
`PayloadBytes` and `ThroughputMbps` columns to `dbo.HttpResult`:

```bash
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/04_AddThroughput.sql
```

This is idempotent — existing rows gain NULL in the new columns and no data is lost.

### Connection string format

```
Server=<host>[,<port>];Database=NetworkDiagnostics;User Id=<user>;Password=<pass>;TrustServerCertificate=true
```

Examples:
```
# Local named instance
Server=localhost\SQLEXPRESS;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true

# Azure SQL
Server=myserver.database.windows.net;Database=NetworkDiagnostics;User Id=user@myserver;Password=Pass!;Encrypt=true
```

### Verify inserts

```sql
-- Run query #10 from sql/03_SampleQueries.sql:
sqlcmd -S localhost -U sa -P "YourPass1!" -d NetworkDiagnostics \
  -Q "SELECT (SELECT COUNT(*) FROM dbo.TestRun) AS Runs, (SELECT COUNT(*) FROM dbo.RequestAttempt) AS Attempts"
```

---

## HTML Report

The report is a single HTML file with embedded CSS. It contains:

1. **Run Summary** – target, modes, success/failure counts, duration
2. **Timing Breakdown** – per-protocol average DNS/TCP/TLS/TTFB/total table (includes download/upload rows)
3. **UDP Statistics** – RTT min/avg/p95, jitter, loss% per probe run
4. **Throughput Results** – per-attempt table showing Mode, Payload size, Throughput (MB/s), TTFB, and Total time (only present when download/upload modes were used)
5. **All Attempts** – individual rows with all timing phases; download/upload rows show throughput in the version column
6. **TLS Details** – version, cipher, ALPN, cert subject/expiry
7. **Errors** – structured error table with category and detail

The `assets/report.css` file can be customised to change colours and fonts.
The inline CSS in the HTML is a minimal fallback for offline use.

---

## Running Tests

### Unit tests (fast, no server needed)

```bash
cargo test --workspace --lib
```

### Endpoint unit tests

```bash
cargo test -p networker-endpoint --lib
```

### Integration tests (start endpoint in-process)

```bash
cargo test --test integration -p networker-tester
```

### SQL integration tests (requires SQL Server)

```bash
export NETWORKER_SQL_CONN="Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true"
cargo test --workspace -- --include-ignored
```

### HTTP/3 build

```bash
cargo build -p networker-tester --features http3
```

### Full test matrix

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

---

## Metrics Captured

### DNS
| Field | Source |
|-------|--------|
| `query_name` | Input hostname |
| `resolved_ips` | DNS A/AAAA records |
| `duration_ms` | Elapsed from lookup call to response |
| `started_at` | UTC timestamp |

### TCP
| Field | Source |
|-------|--------|
| `local_addr` | `getsockname()` |
| `remote_addr` | Target IP:port |
| `connect_duration_ms` | `Instant` wrapping `TcpStream::connect` |
| `mss_bytes` | `TCP_MAXSEG` via `getsockopt` (Unix) |
| `rtt_estimate_ms` | `tcp_info.tcpi_rtt` (Linux only) |

### TLS
| Field | Source |
|-------|--------|
| `protocol_version` | `rustls::ClientConnection::protocol_version` |
| `cipher_suite` | `rustls::ClientConnection::negotiated_cipher_suite` |
| `alpn_negotiated` | `rustls::ClientConnection::alpn_protocol` |
| `cert_subject/issuer` | `x509-parser` on peer leaf certificate |
| `cert_expiry` | `x509-parser` validity.not_after |
| `handshake_duration_ms` | `Instant` wrapping `TlsConnector::connect` |

### HTTP
| Field | Source |
|-------|--------|
| `negotiated_version` | Determined by ALPN negotiation / connection type |
| `status_code` | Response status |
| `ttfb_ms` | `Instant` from request sent to `send_request()` future resolved |
| `total_duration_ms` | From request start to last byte of body received |
| `headers_size_bytes` | Sum of header name+value lengths |
| `body_size_bytes` | Body bytes consumed |
| `payload_bytes` | Bytes requested (download) or sent (upload); `0` for normal probes |
| `throughput_mbps` | `payload_bytes / (total_ms − ttfb_ms) × 1000 / 1MiB` in MB/s; `null` for normal probes |

### UDP
| Field | Source |
|-------|--------|
| `rtt_min/avg/p95_ms` | Per-probe round-trip times aggregated |
| `jitter_ms` | Mean of successive RTT differences |
| `loss_percent` | Probes with no response / total |

---

## Known Limitations

| Limitation | Reason | Workaround |
|-----------|--------|-----------|
| `cwnd` (congestion window) not captured | Only available via BPF/`tcp_info` which requires elevated privileges or kernel eBPF | Run as root on Linux with `SO_MARK` + BPF (out of scope) |
| `tcpi_rtt` available on Linux only | `TCP_INFO` is Linux-specific; Windows and macOS lack equivalent | Best-effort; field is NULL on non-Linux |
| MSS (`TCP_MAXSEG`) on Windows is 0 | Windows does not expose MSS via `getsockopt` in user-mode | Omitted on Windows |
| HTTP/3 is behind `--features http3` | `quinn` + `h3` are pre-1.0; quinn's API is stable but h3 is evolving | Build with `--features http3`; endpoint also needs HTTP/3 support |
| No redirect following | Redirects increase complexity of timing attribution | Use `/health` which returns 200 directly; manual redirect chain analysis is left to the operator |
| Self-signed cert on endpoint | Required for dev; always use `--insecure` flag | Replace with `--cert-file` / `--key-file` options (roadmap) |
| UDP "loss" counts out-of-order packets as lost | Simple seq-number check | For high-latency links, increase `--timeout` and `--udp-probes` |
| QUIC/HTTP3 ALPN detection | QUIC embeds TLS 1.3; there is no separate "TLS phase" – handshake_ms covers both | Documented in TlsResult for HTTP/3 |
| Throughput is single-stream only | Each probe opens one TCP connection; multi-stream parallelism is not tested | Run multiple concurrent probes with `--concurrency` for a rough aggregate |
| Throughput metric is MB/s not Mbps | The formula uses bytes/MiB division, giving megabytes per second | Multiply by 8 to convert to megabits per second |

---

## Design Decisions

### Why hyper 1.x with manual connection management?

`reqwest` (the ergonomic wrapper) abstracts away connection internals, making it impossible to inject timing hooks at the DNS, TCP, and TLS phases. Using `hyper::client::conn::{http1,http2}` directly lets us insert `Instant::now()` checkpoints around each phase with no async overhead.

### Why separate tables for each protocol phase?

A single wide `RequestAttempt` table would have many NULLs (e.g., all TLS columns are NULL for UDP probes). Separate tables keep rows dense and enable efficient filtering by protocol.  The foreign key chain is: `TestRun → RequestAttempt → {DnsResult, TcpResult, TlsResult, HttpResult, UdpResult}`.

### Why `NVARCHAR(36)` for IDs?

SQL Server's `UNIQUEIDENTIFIER` type requires binary conversion in tiberius. Using the UUID string representation is portable, readable in queries, and avoids any driver version quirks. A covering index on `RunId` keeps joins fast.

### Why `rustls` instead of `native-tls`?

`rustls` is pure Rust, cross-platform, and gives programmatic access to negotiated protocol/cipher/ALPN via its `ClientConnection` API. `native-tls` delegates to the OS TLS stack and returns far less metadata.

### Why `rcgen` for the endpoint cert?

Development setup should be instant. `rcgen` generates a valid self-signed cert in-process; no OpenSSL, no `keytool`, no manual file management. Pass `--insecure` to the tester to accept it.

### HTTP/3 gate

QUIC + h3 add significant dependency weight and are still evolving APIs. Gating behind `--features http3` keeps the default build lean and fast to compile while leaving the full stack accessible.
