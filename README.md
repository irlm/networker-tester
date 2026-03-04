# Networker Tester

A cross-platform network diagnostics suite that probes an endpoint across TCP, HTTP/1.1,
HTTP/2, HTTP/3 (optional), and UDP, collecting per-phase telemetry that is normally buried
in the kernel or scattered across multiple tools.

```
┌────────────────────────────────────────────────────────────────────┐
│  networker-tester  ──────────────────────►  networker-endpoint     │
│  (Rust CLI)           TCP / HTTP1 / HTTP2   (Rust server)          │
│                        HTTP3 (QUIC, opt.)                          │
│                        UDP echo                                    │
│         │                                                          │
│         ▼                                                          │
│  JSON artifact  ·  HTML report  ·  Excel workbook  ·  SQL Server   │
└────────────────────────────────────────────────────────────────────┘
```

---

## Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Probe Modes](#probe-modes)
- [CLI Reference](#cli-reference)
- [Configuration File](#configuration-file)
- [Getting All Low-Level Metrics](#getting-all-low-level-metrics)
- [Multi-Target Comparison](#multi-target-comparison)
- [Page-Load Protocol Comparison](#page-load-protocol-comparison-h1--h2--h3--browser)
- [Endpoint Reference](#endpoint-reference)
- [Output Formats](#output-formats)
- [SQL Server Setup](#sql-server-setup)
- [Metrics Captured](#metrics-captured)
- [Running Tests](#running-tests)
- [Known Limitations](#known-limitations)
- [Design Decisions](#design-decisions)

---

## Installation

> **Requirement:** SSH key configured for GitHub
> (`ssh -T git@github.com` → *"Hi \<user\>! You've successfully authenticated…"*).
> The source repository is private; the installer compiles from it using your existing key.

### macOS and Linux

```bash
# Install the diagnostic CLI client
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- tester

# Install the target test server (run on the machine you want to probe)
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- endpoint
```

### Windows (PowerShell)

```powershell
$GistUrl = 'https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1'

# Install the diagnostic CLI client (tester — default)
Invoke-RestMethod $GistUrl | Invoke-Expression

# Install the target test server (endpoint)
# irm | iex cannot forward parameters, so download first then run:
Invoke-WebRequest $GistUrl -OutFile "$env:TEMP\networker-install.ps1"
& "$env:TEMP\networker-install.ps1" -Component endpoint
```

> Compatible with **Windows PowerShell 5.1** and PowerShell 7+. The installer
> auto-detects and offers to install Git for Windows and Visual C++ Build Tools
> via `winget` if they are absent.

### What the installer does

The installer uses a **rustup-style interactive UX**: it shows a system info table, a
numbered plan, and a `1) Proceed / 2) Customize / 3) Cancel` prompt before doing anything.

**Auto-detected install mode:**
- **Release mode** (fast, ~10 s) — if `gh` CLI is installed and authenticated
  (`gh auth login`), downloads the pre-built binary for your platform from the latest
  GitHub release.
- **Source mode** (slower, ~5–10 min) — compiles from the private Git repo via
  `cargo install`; requires an SSH key for GitHub.

**In source mode, the installer also auto-detects and offers to install missing dependencies:**
- Git (via `brew` / `apt-get` / `dnf` / `pacman` / `zypper` / `apk` / `winget`)
- Visual C++ Build Tools on Windows (via `winget`, required by Rust's MSVC target)
- Rust (via [rustup](https://rustup.rs/))

Compilation takes 2–5 minutes on first run; subsequent runs are faster.

### Upgrading

Re-run the same install command **on every machine** where the binary is used.

### Build from source

```bash
git clone git@github.com:irlm/networker-tester.git
cd networker-tester
cargo build --release
# Binaries: target/release/networker-tester  target/release/networker-endpoint
```

---

## Quick Start

### 1 · Start the endpoint

```bash
# Default: HTTP :8080, HTTPS :8443 (self-signed), UDP echo :9999
./target/release/networker-endpoint

# Custom ports
./target/release/networker-endpoint \
  --http-port 9080 --https-port 9443 --udp-port 9999
```

### 2 · Run your first probe

```bash
# HTTP/1.1 + HTTP/2 + UDP latency — the default set
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,udp \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

Open `output/report.html` in any browser.

---

## Probe Modes

| Mode | Transport | What it measures | Requires |
|------|-----------|-----------------|---------|
| `tcp` | TCP | Connect time only (no HTTP) | — |
| `http1` | TCP + HTTP/1.1 | DNS · TCP · TLS · TTFB · Total | — |
| `http2` | TCP + HTTP/2 | DNS · TCP · TLS · TTFB · Total | TLS (ALPN) |
| `http3` | QUIC + HTTP/3 | DNS · QUIC handshake · TTFB · Total | default feature |
| `dns` | DNS | Standalone DNS resolution; resolved IPs + duration | — |
| `tls` | TCP + TLS | DNS · TCP · TLS; full cert chain, cipher, ALPN | — |
| `native` | TCP + native TLS | Same as `tls` but via OS TLS stack | `--features native` |
| `curl` | curl binary | HTTP timing via system `curl` (proxy/cert baseline) | `curl` on PATH |
| `udp` | UDP datagram | RTT min/avg/p95 · jitter · loss% | `--udp-port` |
| `udpdownload` | UDP bulk | Custom NWKT protocol: datagrams/loss/throughput (server→client) | `--payload-sizes` |
| `udpupload` | UDP bulk | Custom NWKT protocol: bytes_acked / throughput (client→server) | `--payload-sizes` |
| `download` | TCP → `/download?bytes=N` | Throughput MB/s (server→client) | `--payload-sizes` |
| `upload` | TCP → `/upload` | Throughput MB/s (client→server) | `--payload-sizes` |
| `webdownload` | TCP → target URL | HTTP timing + response body throughput (URL unchanged) | — |
| `webupload` | TCP → target URL | HTTP timing + upload throughput (URL unchanged) | `--payload-sizes` |
| `pageload` | HTTP/1.1 | Multi-asset page: ≤6 parallel conns, TLS cost, per-asset timings | HTTPS target |
| `pageload2` | HTTP/2 | Multi-asset page: single TLS conn, all assets multiplexed | HTTPS target |
| `pageload3` | HTTP/3 | Multi-asset page: single QUIC conn, all assets multiplexed | HTTPS target |
| `browser` | real Chromium | Headless Chrome via CDP: Load/DCL/TTFB/bytes/per-protocol counts | `--features browser` |

**`pageload` / `pageload2` / `pageload3` / `browser`** all rewrite the URL to the `/page` endpoint,
which serves one HTML document plus N configurable assets.
Use `--page-assets N` (default 20) and `--page-asset-size <sz>` (default 10k) to tune the load.

**`download` vs `webdownload`** (and `upload` vs `webupload`):
`download`/`upload` rewrite the URL path to `/download` or `/upload` on the target host.
`webdownload`/`webupload` fetch the target URL as-is — useful for testing external URLs or
labelling a named group separately in the report.

---

## CLI Reference

### Config file

| Flag | Default | Description |
|------|---------|-------------|
| `--config` / `-c` | — | Path to a JSON config file (see [Configuration File](#configuration-file)) |
| `--log-level` | — | `tracing` filter string (e.g. `"debug"`, `"info,tower_http=debug"`). Overrides `--verbose` and `RUST_LOG` |

### Targeting

| Flag | Default | Description |
|------|---------|-------------|
| `--target` | `http://localhost:8080/health` | URL to probe. Repeat the flag to test multiple targets in one run and get a combined comparison report: `--target URL1 --target URL2` |
| `--modes` | `http1,http2,udp` | Comma-separated probe modes (see table above) |
| `--runs` | `3` | Repetitions per mode per run cycle |
| `--concurrency` | `1` | Concurrent requests per run (best-effort) |
| `--timeout` | `30` | Per-request timeout in seconds |
| `--retries` | `0` | Retry failed probes up to N times |

### Payload and throughput

| Flag | Default | Description |
|------|---------|-------------|
| `--payload-sizes` | — | Comma-separated sizes for `download`/`upload`/`webupload` (e.g. `4k,64k,1m`). Each size produces a separate probe. |
| `--payload-size` | `0` | POST body size for `/echo` tests (bytes) |

Size suffixes: `k` = KiB (×1024), `m` = MiB (×1024²), `g` = GiB (×1024³).

### UDP

| Flag | Default | Description |
|------|---------|-------------|
| `--udp-port` | `9999` | UDP echo server port on the target host |
| `--udp-probes` | `10` | Probe packets per run |

### Connection options

| Flag | Default | Description |
|------|---------|-------------|
| `--insecure` | — | Skip TLS certificate verification (for self-signed endpoint certs) |
| `--dns-enabled` | `true` | Perform DNS resolution |
| `--ipv4-only` | — | Restrict to IPv4 addresses |
| `--ipv6-only` | — | Restrict to IPv6 addresses (conflicts with `--ipv4-only`) |
| `--no-proxy` | — | Bypass any system proxy |
| `--connection-reuse` | — | Reuse a single TCP connection across HTTP requests |

### Output

| Flag | Default | Description |
|------|---------|-------------|
| `--output-dir` | `./output` | Directory for JSON artifact, HTML report, and Excel workbook |
| `--html-report` | `report.html` | HTML filename (relative to `--output-dir`) |
| `--css` | `report.css` | CSS `<link>` href embedded in the HTML |
| `--excel` | — | Write a `.xlsx` workbook with 8 sheets alongside JSON + HTML |
| `--verbose` / `-v` | — | Enable debug logging (equivalent to `--log-level debug`) |

### SQL Server

| Flag | Default | Description |
|------|---------|-------------|
| `--save-to-sql` | — | Insert results into SQL Server |
| `--connection-string` | `$NETWORKER_SQL_CONN` | ADO.NET connection string |

---

## Configuration File

Both binaries accept a JSON config file via `--config` / `-c`. Any key in the file can be
overridden by the corresponding CLI flag.

**Priority (highest → lowest):** CLI flag > JSON config key > `RUST_LOG` env / built-in default

### tester.example.json

```json
{
  "target": "http://localhost:8080/health",
  "targets": ["http://localhost:8080/health", "https://remote:8443/health"],
  "modes": ["http1", "http2", "udp"],
  "runs": 3,
  "concurrency": 1,
  "timeout": 30,
  "payload_sizes": [],
  "udp_port": 9999,
  "udp_throughput_port": 9998,
  "udp_probes": 10,
  "insecure": false,
  "retries": 0,
  "output_dir": "./output",
  "html_report": "report.html",
  "excel": false,
  "save_to_sql": false
}
```

> **Note:** Use `"targets"` (plural) to list multiple probe targets in a config file.
> The legacy `"target"` (singular) is still supported for backward compatibility; both
> keys are merged with any `--target` flags given on the command line.

### endpoint.example.json

```json
{
  "http_port": 8080,
  "https_port": 8443,
  "udp_port": 9999,
  "udp_throughput_port": 9998,
  "log_level": "info"
}
```

### Usage

```bash
# Run the tester with a saved config; override --runs from the command line
networker-tester --config tester.example.json --runs 1

# Start the endpoint on a non-default port with verbose HTTP logs
echo '{"http_port":9090,"log_level":"info,tower_http=debug"}' > ep.json
networker-endpoint --config ep.json

# CLI flag still wins
networker-endpoint --config ep.json --http-port 8080
```

Unknown JSON keys are silently ignored (forward-compatible for future releases).
The `connection_string` key (tester) and `log_level` key (both) may be omitted — they
default to `null` / the `RUST_LOG` env var.

---

## Getting All Low-Level Metrics

### Minimum invocation — HTTP latency only

```bash
networker-tester \
  --target https://host:8443/health \
  --modes http1 \
  --runs 3 \
  --insecure
```

Captures: DNS · TCP connect · TLS handshake · TTFB · Total.

### Add TCP kernel stats (Linux / macOS, no root needed)

TCP kernel stats are captured automatically for every TCP-based probe. Run any of
`tcp`, `http1`, `http2`, `download`, `upload`, `webdownload`, `webupload` and the report
will include a **TCP Stats** card showing:

- RTT (smoothed) and RTT variance
- Minimum RTT ever observed by the kernel
- Congestion window (`cwnd`) and slow-start threshold (`ssthresh`)
- Retransmits (current queue) and total lifetime retransmits
- Receive window (`rcv_space`)
- Segments sent/received (`segs_out`/`segs_in`) — Linux ≥ 4.2
- Estimated delivery rate (MB/s) — Linux ≥ 4.9
- Congestion algorithm name ("cubic", "bbr", …)
- MSS (Maximum Segment Size)

No elevated privileges required — all fields come from `TCP_INFO` / `TCP_CONNECTION_INFO`
via `getsockopt`.

```bash
# Full low-level run: TCP connect stats + HTTP timing + UDP latency
networker-tester \
  --target https://host:8443/health \
  --modes tcp,http1,http2,udp \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

### Throughput with full HTTP timing

```bash
# Download and upload: measures MB/s + DNS/TCP/TLS/TTFB/Total
networker-tester \
  --target http://host:8080/health \
  --modes download,upload \
  --payload-sizes 4k,64k,1m \
  --runs 3 \
  --output-dir ./output
```

### Side-by-side download and upload labelled separately

`webdownload` / `webupload` rewrite the URL path to `/download` and `/upload` exactly
like their counterparts, but emit a different protocol label (`webdownload` / `webupload`)
in the report. This lets you run two named groups in a single invocation and compare them:

```bash
# Run download and webdownload back-to-back, compare results in one report
networker-tester \
  --target http://host:8080/health \
  --modes download,webdownload \
  --payload-sizes 4k,64k,1m \
  --runs 5

# Full comparison: all four throughput modes
networker-tester \
  --target http://host:8080/health \
  --modes download,upload,webdownload,webupload \
  --payload-sizes 64k,1m \
  --runs 3
```

### Everything in one run (all modes, all metrics)

The most comprehensive single invocation — captures every metric layer:

```bash
networker-tester \
  --target https://host:8443/health \
  --modes tcp,http1,http2,udp,download,upload,webdownload,webupload \
  --payload-sizes 64k,1m \
  --runs 3 \
  --retries 1 \
  --insecure \
  --excel \
  --output-dir ./output
```

This produces per-attempt data across all 9 probe types including:

| Report section | What you see |
|---------------|-------------|
| Timing Breakdown | Avg DNS/TCP/TLS/TTFB/Total per protocol |
| Throughput Results | MB/s per payload size (download + upload + webdownload + webupload) |
| TCP Stats | RTT, cwnd, ssthresh, retransmits, delivery rate, congestion algorithm |
| UDP Statistics | RTT min/avg/p95, jitter, loss% |
| TLS Details | Version, cipher suite, ALPN, cert subject/expiry |
| All Attempts | Every individual probe with all timing phases |
| Errors | Structured error table |

With `--excel` the workbook has 8 sheets (Summary, HTTP Timings, TCP Stats, TLS Details,
UDP Stats, Throughput, Server Timing, Errors).

### Retries and resilience

```bash
# Retry transient failures up to 2 times; retry_count is recorded in JSON
networker-tester \
  --target https://host:8443/health \
  --modes http1,http2 \
  --runs 10 \
  --retries 2 \
  --insecure
```

### Save to SQL Server

```bash
networker-tester \
  --target https://host:8443/health \
  --modes http1,http2,udp,download,upload \
  --payload-sizes 64k \
  --runs 3 \
  --insecure \
  --save-to-sql \
  --connection-string "Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=YourPass1!;TrustServerCertificate=true" \
  --output-dir ./output
```

Or set the connection string via environment variable:

```bash
export NETWORKER_SQL_CONN="Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=YourPass1!;TrustServerCertificate=true"
networker-tester --target ... --modes http1,http2 --save-to-sql
```

### HTTP/3 (QUIC)

```bash
cargo build --release --features http3

networker-tester \
  --target https://host:8443/health \
  --modes http3 \
  --insecure
```

---

## Multi-Target Comparison

Pass `--target` more than once to probe multiple endpoints in a single run and get
one combined HTML report.

### Example — local vs cloud endpoint

```bash
./networker-tester \
  --target http://127.0.0.1:8080/health \
  --target https://my-cloud-vm:8443/health \
  --modes tcp,http1,http2,http3,udp,download \
  --payload-sizes 1m \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

The HTML report (`output/report.html`) opens with:

1. **Multi-Target Summary** — one row per target, totals at a glance
2. **Cross-Target Protocol Comparison** — the primary metric for each protocol side-by-side,
   with % delta vs the first target (green = faster, red = slower)
3. **Per-target details** — each target's full report in a collapsible card

JSON files are named `run-{ts}-1.json` (local) and `run-{ts}-2.json` (cloud).
Excel workbooks follow the same numbering when `--excel` is passed.

### Via config file

```json
{
  "targets": [
    "http://127.0.0.1:8080/health",
    "https://my-cloud-vm:8443/health"
  ],
  "modes": ["http1", "http2", "download"],
  "payload_sizes": ["1m"],
  "runs": 5,
  "insecure": true
}
```

```bash
networker-tester --config multi.json
```

### Backward compatibility

Passing a single `--target` (or no `--target`) produces exactly the same single-target
report as before — the multi-target sections are only added when two or more targets are
detected.

---

## Page-Load Protocol Comparison (H1 / H2 / H3 / Browser)

The page-load probes simulate a browser fetching a real HTML page with N embedded assets.
All four modes target the same `/page` endpoint so the comparison is fair: only the
transport and connection model change.

| Mode | Connection model | What makes it realistic |
|------|-----------------|------------------------|
| `pageload` | Up to 6 parallel TCP connections | Matches H1.1 browser behavior (connection pool) |
| `pageload2` | Single TLS connection, all assets multiplexed | H2 stream multiplexing |
| `pageload3` | Single QUIC connection, all assets multiplexed | H3 / QUIC — no TCP head-of-line blocking |
| `browser` | Real headless Chromium via CDP | Actual browser events: Load, DOMContentLoaded, sub-resource negotiation |

### Minimum example — H1.1 vs H2 vs H3

```bash
# 1. Start the endpoint (HTTPS required for H2 and H3)
./networker-endpoint

# 2. Compare all three protocols — 20 assets × 10 KiB each (defaults), 5 runs
./networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3 \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

Terminal output includes a **Protocol Comparison** table once all runs complete:

```
──── Protocol Comparison ────

  pageload   ·  Total ms  ·  avg 124.3  min  98.2  max 161.7
  pageload2  ·  Total ms  ·  avg  89.1  min  76.4  max 105.3
  pageload3  ·  Total ms  ·  avg  82.6  min  69.1  max  99.2
```

### Customize asset count and size

```bash
# 50 assets × 50 KiB — heavier page, amplifies H1 head-of-line blocking
./networker-tester \
  --target https://host:8443/health \
  --modes pageload,pageload2,pageload3 \
  --page-assets 50 \
  --page-asset-size 50k \
  --runs 5 \
  --insecure
```

### Add real-browser metrics

Requires Chrome or Chromium installed. Build with the `browser` feature:

```bash
cargo build --release --features browser
```

Then add `browser` to `--modes`:

```bash
./networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3,browser \
  --page-assets 20 \
  --page-asset-size 10k \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

The `browser` probe drives real headless Chromium and captures events that synthetic probes
cannot replicate:

| Metric | Meaning |
|--------|---------|
| **Load time** | Navigation start → `load` event — what end users experience |
| **DOMContentLoaded** | DOM fully parsed, before images/stylesheets finish |
| **TTFB** | Time to first byte of the main document from the browser's perspective |
| **Resource count** | Total sub-resources loaded (HTML + all assets) |
| **Bytes transferred** | Actual bytes over the wire as seen by the browser |
| **Per-protocol counts** | e.g. `h2×18 h3×2` — which resources negotiated which protocol |

Chrome is auto-detected from common install paths. Override with an environment variable:

```bash
NETWORKER_CHROME_PATH=/usr/bin/chromium ./networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes browser --runs 3 --insecure
```

### Interpreting the results

| What to compare | What it tells you |
|-----------------|-------------------|
| `pageload` vs `pageload2` total time | Benefit of H2 multiplexing over H1.1 connection pools |
| `pageload2` vs `pageload3` total time | QUIC advantage (especially on lossy or high-latency links) |
| `pageload` connections_opened | Should be ≤ 6; more parallel TCP = more TLS handshake cost |
| `pageload` tls_overhead_ratio | TLS as % of total — high ratio → TLS cost dominates, connection reuse helps |
| `browser` Load event vs `pageload` total | Gap = browser render/parse/JS overhead on top of network |
| `browser` per-protocol counts | Mixed `h2×N h3×M` → protocol fallback or split CDN |

> **Note:** `pageload3` and `browser` require HTTPS. Pass `--insecure` when pointing at the
> self-signed certificate generated by `networker-endpoint`.

---

## Endpoint Reference

### HTTP routes

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | `{"status":"ok"}` with `X-Networker-Server-Version` header |
| `/echo` | POST | Echoes request body verbatim |
| `/echo` | GET | Returns received header info |
| `/download?bytes=N` | GET | Returns N zero bytes (max 100 MiB). Adds `Server-Timing: proc;dur=X` |
| `/upload` | POST | Accepts body; returns `{received_bytes}`. Adds `Server-Timing: recv;dur=X`; echoes `X-Networker-Request-Id` |
| `/delay?ms=N` | GET | Sleeps N ms (max 30 s) then responds |
| `/headers` | GET | Returns all received headers as JSON |
| `/status/:code` | GET | Returns the given HTTP status code |
| `/http-version` | GET | Returns negotiated HTTP version string |
| `/info` | GET | Server capabilities JSON |

### Server-Timing headers

Every response includes `X-Networker-Server-Timestamp` (UTC ISO-8601).
`/download` adds `Server-Timing: proc;dur=X` (body generation time).
`/upload` adds `Server-Timing: recv;dur=X` (body drain time) — used by the tester to
compute accurate upload throughput even when the server responds before the body is fully
drained.

### UDP echo

| Port | Protocol | Description |
|------|----------|-------------|
| `:9999` | UDP datagram | Reflects every packet back unchanged. Used by `--modes udp`. |

### Endpoint CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--config` / `-c` | — | Path to a JSON config file (see [Configuration File](#configuration-file)) |
| `--http-port` | `8080` | HTTP (plain) listening port |
| `--https-port` | `8443` | HTTPS (TLS) listening port |
| `--udp-port` | `9999` | UDP echo listening port |
| `--udp-throughput-port` | `9998` | UDP bulk throughput listening port |
| `--log-level` | — | `tracing` filter string. Overrides `RUST_LOG` |

### Logging

The endpoint uses [`tracing`](https://docs.rs/tracing) and logs to stdout.
Log verbosity is controlled by `--log-level` or the `RUST_LOG` environment variable:

```bash
# Default — INFO: version banner, listening addresses, one line per request/response
./networker-endpoint

# Via --log-level flag (persists in config file; see below)
./networker-endpoint --log-level info,tower_http=debug

# Quiet — only warnings and errors
RUST_LOG=warn ./networker-endpoint

# Verbose HTTP — keep endpoint logs at INFO, raise tower-http to DEBUG
# (shows request headers, response body size, full span details)
# IMPORTANT: a target-specific directive alone (e.g. "tower_http=debug") hides
# all other log targets including the endpoint's own startup lines. Always
# include a default level before the comma when using target overrides.
RUST_LOG=info,tower_http=debug ./networker-endpoint

# Debug everything
RUST_LOG=debug ./networker-endpoint
```

Example startup output (default `INFO`):
```
INFO networker_endpoint: networker-endpoint v0.3.2
INFO networker_endpoint: HTTP  → http://0.0.0.0:8080
INFO networker_endpoint: HTTPS → https://0.0.0.0:8443  (self-signed, use --insecure)
INFO networker_endpoint: UDP echo       → 0.0.0.0:9999
INFO networker_endpoint: UDP throughput → 0.0.0.0:9998
```

Example per-request output:
```
INFO request{method=GET uri=/download?bytes=65536 version=HTTP/1.1}: started processing request
INFO request{method=GET uri=/download?bytes=65536 version=HTTP/1.1}: finished processing request status=200 latency=8 ms
```

---

## Output Formats

### JSON artifact

One file per target per run. Naming:
- Single target: `run-YYYYMMDD-HHMMSS.json`
- Multiple targets: `run-YYYYMMDD-HHMMSS-1.json`, `run-YYYYMMDD-HHMMSS-2.json`, …

Normalised structure:

```
TestRun
  └─ attempts[]
       ├─ dns:    { query_name, resolved_ips, duration_ms }
       ├─ tcp:    { local_addr, remote_addr, connect_duration_ms, mss_bytes,
       │            rtt_estimate_ms, rtt_variance_ms, min_rtt_ms,
       │            snd_cwnd, snd_ssthresh, retransmits, total_retrans,
       │            rcv_space, segs_out, segs_in,
       │            delivery_rate_bps, congestion_algorithm }
       ├─ tls:    { protocol_version, cipher_suite, alpn_negotiated,
       │            cert_subject, cert_expiry, handshake_duration_ms }
       ├─ http:   { negotiated_version, status_code, ttfb_ms, total_duration_ms,
       │            payload_bytes, throughput_mbps, response_headers[] }
       ├─ udp:    { probe_count, loss_percent, rtt_min/avg/p95_ms, jitter_ms }
       ├─ server_timing: { recv_body_ms, processing_ms, clock_skew_ms,
       │                   server_timestamp, server_version }
       └─ error:  { category, message, detail }
```

### HTML report

Single file (with optional external CSS). For a **single target**, sections are:

1. **Run Summary** — target, modes, attempt count, duration, client/server versions
2. **Timing Breakdown** — per-protocol averages: DNS/TCP/TLS/TTFB/Total
3. **UDP Statistics** — RTT min/avg/p95, jitter, loss%
4. **Throughput Results** — payload sizes, MB/s, TTFB (download/upload/webdownload/webupload)
5. **All Attempts** — every individual probe with all timing phases and throughput
6. **TCP Stats** — kernel-level socket metrics per connection (see [TCP](#tcp) below)
7. **TLS Details** — negotiated version, cipher, ALPN, cert subject/expiry
8. **Errors** — structured error table

For **multiple targets**, the same file additionally includes:

1. **Multi-Target Summary** — one row per target with attempt count, duration, succeeded/failed
2. **Cross-Target Protocol Comparison** — side-by-side table of primary metrics per protocol,
   with % delta vs the first target highlighted green (faster) or red (slower)
3. **Per-target collapsible sections** — each target's full single-target report nested inside
   a `<details>` element (open by default when ≤ 2 targets)

### Excel workbook (`--excel`)

Eight worksheets: Summary · HTTP Timings · TCP Stats · TLS Details · UDP Stats ·
Throughput · Server Timing · Errors.

---

## SQL Server Setup

### Option A — Docker

```bash
docker run --name networker-sql \
  -e ACCEPT_EULA=Y -e SA_PASSWORD="YourPass1!" \
  -p 1433:1433 \
  -d mcr.microsoft.com/mssql/server:2022-latest
```

### Option B — Existing SQL Server

Ensure a login with `db_owner` on the target database.

### Apply schema migrations in order

```bash
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/02_StoredProcedures.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/03_SampleQueries.sql   # optional: sample queries
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/04_AddThroughput.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/05_ExtendedTcpStats.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/06_ServerTiming.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/07_MoreTcpStats.sql
```

All migration scripts are **idempotent** — safe to re-run.

### Connection string format

```
Server=<host>[,<port>];Database=NetworkDiagnostics;User Id=<user>;Password=<pass>;TrustServerCertificate=true
```

Examples:
```
# Local
Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true

# Named instance
Server=localhost\SQLEXPRESS;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true

# Azure SQL
Server=myserver.database.windows.net;Database=NetworkDiagnostics;User Id=user@myserver;Password=Pass!;Encrypt=true
```

---

## Metrics Captured

### DNS

| Field | Source |
|-------|--------|
| `query_name` | Input hostname |
| `resolved_ips` | DNS A/AAAA records |
| `duration_ms` | Elapsed from lookup call to result |
| `started_at` | UTC timestamp |

### TCP

All TCP fields are collected from a single `getsockopt` call per connection — no root, no BPF.

| Field | Source | Platform |
|-------|--------|----------|
| `local_addr` | `getsockname()` | All |
| `remote_addr` | Target IP:port | All |
| `connect_duration_ms` | `Instant` around `TcpStream::connect` | All |
| `mss_bytes` | `TCP_MAXSEG` via `getsockopt` | Linux, macOS |
| `rtt_estimate_ms` | `tcpi_rtt` (µs → ms) | Linux, macOS |
| `rtt_variance_ms` | `tcpi_rttvar` (µs → ms) | Linux, macOS |
| `min_rtt_ms` | `tcpi_min_rtt` (µs → ms) | Linux ≥ 4.9 |
| `snd_cwnd` | `tcpi_snd_cwnd` (segments) | Linux, macOS |
| `snd_ssthresh` | `tcpi_snd_ssthresh`; `None` when kernel sentinel (∞) | Linux, macOS |
| `retransmits` | `tcpi_retransmits` (current retransmit queue) | Linux, macOS |
| `total_retrans` | `tcpi_total_retrans` (lifetime count) | Linux |
| `rcv_space` | `tcpi_rcv_space` (receive window, bytes) | Linux |
| `segs_out` | `tcpi_segs_out` (segments sent) | Linux ≥ 4.2 |
| `segs_in` | `tcpi_segs_in` (segments received) | Linux ≥ 4.2 |
| `delivery_rate_bps` | `tcpi_delivery_rate` (bytes/sec) | Linux ≥ 4.9 |
| `congestion_algorithm` | `TCP_CONGESTION` getsockopt (e.g. "cubic", "bbr") | Linux, macOS |

> **Implementation note (Linux):** Fields are read from a raw `[u8; 232]` buffer with
> byte-offset guards on the `optlen` returned by `getsockopt`. This allows the binary to
> run on any kernel ≥ 3.x and silently omit fields that the running kernel does not report,
> without depending on libc struct definitions for newer fields.

### TLS

| Field | Source |
|-------|--------|
| `protocol_version` | `rustls::ClientConnection::protocol_version` |
| `cipher_suite` | `rustls::ClientConnection::negotiated_cipher_suite` |
| `alpn_negotiated` | `rustls::ClientConnection::alpn_protocol` |
| `cert_subject` / `cert_issuer` | `x509-parser` on peer leaf certificate |
| `cert_expiry` | `x509-parser` validity `not_after` |
| `handshake_duration_ms` | `Instant` around `TlsConnector::connect` |

### HTTP

| Field | Source |
|-------|--------|
| `negotiated_version` | ALPN negotiation / connection type |
| `status_code` | Response status |
| `ttfb_ms` | `Instant` from just-before-`send_request()` to first response byte |
| `total_duration_ms` | From request start to last byte of body received |
| `headers_size_bytes` | Sum of header name+value lengths |
| `body_size_bytes` | Body bytes consumed |
| `payload_bytes` | Bytes requested (download) or sent (upload) |
| `throughput_mbps` | `payload_bytes / transfer_window_ms` (download and upload modes) |

**Transfer window by direction:**
- Download: `total_duration_ms − ttfb_ms` (body-receive phase only)
- Upload: `max(server_recv_ms, ttfb_ms)` — prefers the server's `Server-Timing: recv;dur=X`
  when available; falls back to `ttfb_ms` (the client stopwatch that spans the wire transfer).

### UDP

| Field | Source |
|-------|--------|
| `rtt_min_ms` | Minimum round-trip across all probes |
| `rtt_avg_ms` | Mean round-trip |
| `rtt_p95_ms` | 95th-percentile round-trip |
| `jitter_ms` | Mean of successive absolute RTT differences |
| `loss_percent` | Probes with no echo response / total probes |
| `probe_rtts_ms` | Per-probe RTT array (`null` = lost) |

### Server timing (X-Networker-\* headers)

| Field | Source |
|-------|--------|
| `server_version` | `X-Networker-Server-Version` (every response) |
| `server_timestamp` | `X-Networker-Server-Timestamp` (every response) |
| `clock_skew_ms` | `(server_ts − client_send_at) − ttfb_ms/2` |
| `recv_body_ms` | `Server-Timing: recv;dur=X` (upload drain time) |
| `processing_ms` | `Server-Timing: proc;dur=X` (download generation time) |
| `request_id` | Echoed `X-Networker-Request-Id` |

---

## Running Tests

| Layer | Command | Requires |
|-------|---------|----------|
| **Unit** | `cargo test --workspace --lib` | Nothing — fully offline |
| **Integration** | `cargo test --test integration -p networker-tester -- --test-threads=1` | Nothing (endpoint is in-process) |
| **SQL integration** | see below | SQL Server + `NETWORKER_SQL_CONN` |

### Unit tests

```bash
cargo test --workspace --lib          # all crates (tester + endpoint)
cargo test -p networker-tester --lib  # tester only
cargo test -p networker-endpoint --lib
```

Covers: CLI parsing, payload-size suffixes, throughput formula (download/upload time windows),
RTT aggregation, HTML rendering, JSON round-trip, TLS ALPN config, UDP probe calculations.

### Integration tests

```bash
cargo test --test integration -p networker-tester -- --test-threads=1
```

> `--test-threads=1` is required: each test spawns its own in-process endpoint on random
> ports; concurrent tests may collide on port binding.

### SQL integration tests

```bash
docker run --name networker-sql \
  -e ACCEPT_EULA=Y -e SA_PASSWORD="YourPass1!" \
  -p 1433:1433 -d mcr.microsoft.com/mssql/server:2022-latest

sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/01_CreateDatabase.sql
sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/02_StoredProcedures.sql

export NETWORKER_SQL_CONN="Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=YourPass1!;TrustServerCertificate=true"
cargo test --workspace -- sql --include-ignored
```

### Full CI check

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --lib
cargo test --test integration -p networker-tester -- --test-threads=1
```

### HTTP/3 feature build

```bash
cargo build -p networker-tester --features http3
```

---

## Known Limitations

| Limitation | Reason | Workaround |
|-----------|--------|-----------|
| TCP kernel stats unavailable on Windows | Windows does not expose `TCP_INFO` equivalent via `getsockopt` in user mode | Run tester on Linux or macOS; or use a WSL2 Linux environment |
| `segs_out`, `segs_in` only on Linux ≥ 4.2 | Field was added in kernel 4.2 | Silently omitted on older kernels; check `optlen` guard |
| `delivery_rate_bps`, `min_rtt_ms` only on Linux ≥ 4.9 | Field was added in kernel 4.9 | Silently omitted on older kernels |
| MSS (`TCP_MAXSEG`) is 0 on Windows | Windows does not expose MSS via `getsockopt` | Omitted on Windows |
| HTTP/3 behind `--features http3` | `quinn` + `h3` are pre-1.0; quinn API is stable but h3 is evolving | Build with `--features http3` (both client and endpoint) |
| No redirect following | Redirects complicate timing attribution | Point `--target` at a URL that responds directly (e.g. `/health`) |
| Self-signed cert on endpoint | Required for dev | Pass `--insecure` to the tester |
| UDP "loss" counts out-of-order as lost | Simple seq-number check | Increase `--timeout` and `--udp-probes` for high-latency links |

---

## Design Decisions

### Why hyper 1.x with manual connection management?

`reqwest` abstracts away connection internals, making it impossible to inject timing hooks
at the DNS, TCP, and TLS phases. Using `hyper::client::conn::{http1,http2}` directly lets
us insert `Instant::now()` checkpoints around each phase with no async overhead.

### Why separate sub-result structs per protocol phase?

A single wide `RequestAttempt` table would have many NULLs (all TLS columns are NULL for
UDP probes). Separate structs keep the JSON/SQL dense and enable efficient filtering by
protocol. The foreign key chain is:
`TestRun → RequestAttempt → {DnsResult, TcpResult, TlsResult, HttpResult, UdpResult}`.

### Why raw byte-offset reads for `tcp_info` on Linux?

The kernel `tcp_info` struct has grown across releases. Reading into a `[u8; 232]` buffer
and gating each field on the `optlen` returned by `getsockopt` lets the binary run on any
kernel ≥ 3.x and silently omit fields added in 4.2 / 4.9 / 4.13 — without depending on
the libc crate's struct definition matching the running kernel.

### Why `max(server_recv_ms, ttfb_ms)` for upload throughput?

`ttfb_ms` is measured by a stopwatch that starts just before `send_request()` and stops
when the server's response headers arrive. Because `networker-endpoint` only responds
**after** draining the full request body, `ttfb_ms` captures the actual wire transfer time.
`max()` handles old-style endpoints that respond before draining (where `ttfb_ms ≈ 0` and
`Server-Timing: recv;dur=X` is the accurate value instead). `total_duration_ms` is
unsuitable because it adds response body receipt time — download noise.

### Why `NVARCHAR(36)` for IDs?

SQL Server's `UNIQUEIDENTIFIER` requires binary conversion in tiberius. UUID strings are
portable, readable in queries, and avoid driver version quirks. A covering index on `RunId`
keeps joins fast.

### Why `rustls` over `native-tls`?

Pure Rust, cross-platform, and gives programmatic access to negotiated protocol, cipher,
and ALPN via its `ClientConnection` API. `native-tls` delegates to the OS TLS stack and
returns far less metadata.
