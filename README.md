# Networker Tester

A cross-platform network diagnostics suite that exercises an endpoint using TCP,
HTTP/1.1, HTTP/2, HTTP/3 (optional), and UDP, collecting detailed per-phase
telemetry from user-mode code.

```
┌─────────────────────────────────────────────────────────────┐
│  networker-tester  ──────────────────────►  networker-endpoint │
│  (Rust CLI)          TCP / HTTP1 / HTTP2    (Rust server)       │
│                      HTTP3 (QUIC, optional)                     │
│                      UDP echo                                    │
│         │                                                       │
│         ▼                                                       │
│  JSON artifact + HTML report + SQL Server inserts               │
└─────────────────────────────────────────────────────────────┘
```

---

## Contents

- [Installation](#installation)
- [Prerequisites](#prerequisites)
- [Quick Start (30 minutes)](#quick-start)
- [CLI Reference](#cli-reference)
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
3. Runs `cargo install --git ssh://git@github.com/irlm/networker-tester <binary> --locked --force`
   to compile and install the binary from the private repository.
4. Prints the installed path and the installed version (e.g. `networker-tester 0.2.4`).

Compilation takes 2–5 minutes on first run (all dependencies are downloaded and compiled).
Subsequent installs are faster because cargo caches compiled artifacts.

### Upgrading

Re-run the same install command — **on every machine** where the binary is used.
The `--force` flag ensures the binary is always rebuilt from the latest commit, even
when cargo's internal cache thinks the version is current.

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

**Throughput measurement (download + upload across multiple payload sizes):**
```bash
./target/release/networker-tester \
  --target http://127.0.0.1:8080/health \
  --modes download,upload \
  --payload-sizes 4k,64k,1m \
  --runs 3 \
  --output-dir ./output
```

Each payload size is probed independently — `4k`, `64k`, and `1m` produce
3 sizes × 2 modes × 3 runs = **18 attempts**, each reporting throughput in MB/s.

**Mix latency and throughput in one run:**
```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,download,upload \
  --payload-sizes 64k,1m \
  --runs 3 \
  --insecure \
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
| `--modes` | `http1,http2,udp` | Comma-separated: `tcp,http1,http2,http3,udp` |
| `--runs` | `3` | Repetitions per mode |
| `--concurrency` | `1` | Concurrent requests per run |
| `--timeout` | `30` | Per-request timeout (seconds) |
| `--payload-size` | `256` | POST body size (bytes) for `/echo` tests |
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

### Create schema

```bash
# Linux/macOS (sqlcmd from mssql-tools)
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/02_StoredProcedures.sql

# Windows (sqlcmd in PATH after SQL Server install)
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql\01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql\02_StoredProcedures.sql
```

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
2. **Timing Breakdown** – per-protocol average DNS/TCP/TLS/TTFB/total table
3. **UDP Statistics** – RTT min/avg/p95, jitter, loss% per probe run
4. **All Attempts** – individual rows with all timing phases
5. **TLS Details** – version, cipher, ALPN, cert subject/expiry
6. **Errors** – structured error table with category and detail

The `assets/report.css` file can be customised to change colours and fonts.
The inline CSS in the HTML is a minimal fallback for offline use.

---

## Running Tests

The suite has three independent layers:

| Layer | Command | Requires | Tests |
|-------|---------|----------|-------|
| **Unit** | `cargo test --workspace --lib` | Nothing — fully offline | ~35 |
| **Integration** | `cargo test --test integration -p networker-tester` | Nothing — endpoint is in-process | ~10 |
| **SQL integration** | see below | SQL Server + `NETWORKER_SQL_CONN` | 1 |

### Layer 1 — Unit tests

Test individual functions and formulas in isolation.  No server, no network, no database.

```bash
# All crates (tester + endpoint)
cargo test --workspace --lib

# Tester only
cargo test -p networker-tester --lib

# Endpoint only
cargo test -p networker-endpoint --lib
```

What's covered: CLI parsing, payload-size suffixes, throughput formula (download vs upload
time windows), RTT aggregation, HTML rendering, JSON round-trip, TLS ALPN config, UDP probe
calculations.

### Layer 2 — Integration tests

End-to-end probe pipeline tests.  `networker-endpoint` is spawned **in-process** on random
ports — no manual server setup required.

```bash
cargo test --test integration -p networker-tester -- --test-threads=1
```

> `--test-threads=1` is required because each test spawns its own endpoint on
> random ports — concurrent tests can grab the same port before the endpoint
> binds it (TOCTOU race).

What's covered:

| Test | What it exercises |
|------|------------------|
| `http1_health_returns_200` | Full HTTP/1.1 pipeline: TCP connect → send GET → parse response |
| `http1_echo_round_trips_payload` | POST body send + response body receive |
| `http2_over_tls_negotiates_h2` | TLS handshake, ALPN `h2`, HTTP/2 framing |
| `http1_over_tls_negotiates_http11` | TLS with ALPN `http/1.1` |
| `tcp_only_mode_records_connect_time` | TCP probe stops after connect — no HTTP sent |
| `udp_probe_measures_rtt` | UDP echo: RTT min/avg/p95, loss percent |
| `http1_delay_endpoint_respected` | `/delay?ms=100` — TTFB ≥ 90 ms |
| `http1_status_endpoint_returns_correct_code` | `/status/404` → status 404 |
| `download_probe_reports_throughput` | `run_download_probe` → `/download?bytes=65536`, verifies `payload_bytes` + `throughput_mbps` |
| `upload_probe_reports_throughput` | `run_upload_probe` → `POST /upload` with 64 KiB body, verifies `payload_bytes` + `throughput_mbps` |

### Layer 3 — SQL integration tests

Verifies the tiberius write path against a real SQL Server instance.  Skipped by default
(`#[ignore]`); opt in by setting `NETWORKER_SQL_CONN`.

```bash
# Start SQL Server (Docker)
docker run --name networker-sql \
  -e ACCEPT_EULA=Y -e SA_PASSWORD="YourPass1!" \
  -p 1433:1433 -d mcr.microsoft.com/mssql/server:2022-latest

# Apply schema
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/02_StoredProcedures.sql

# Run SQL tests
export NETWORKER_SQL_CONN="Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=YourPass1!;TrustServerCertificate=true"
cargo test --workspace -- sql --include-ignored
```

In CI, enable the SQL job by setting the `NETWORKER_SQL_TESTS = true` repository variable
under *Settings → Secrets and variables → Actions*.

### HTTP/3 feature build

HTTP/3 support is behind a Cargo feature flag (adds `quinn` + `h3`):

```bash
cargo build -p networker-tester --features http3
```

### Full check (mirrors CI)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --lib
cargo test --test integration -p networker-tester -- --test-threads=1
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
