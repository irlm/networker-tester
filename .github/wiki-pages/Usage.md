# Usage

---

## Basic Usage

```bash
# Start the endpoint (HTTP :8080, HTTPS :8443, UDP echo :9999)
networker-endpoint

# Run a probe against it
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,http3 \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

Open `output/report.html` in any browser.

---

## All Probe Modes

| Mode | Transport | What it measures | Notes |
|------|-----------|-----------------|-------|
| `tcp` | TCP | DNS + TCP connect time only | No HTTP |
| `http1` | HTTP/1.1 | DNS, TCP, TLS, TTFB, Total | Works over plain HTTP or HTTPS |
| `http2` | HTTP/2 | DNS, TCP, TLS (ALPN h2), TTFB, Total | Requires HTTPS |
| `http3` | QUIC | QUIC handshake, TTFB, Total | UDP-based; no separate DNS/TCP phases |
| `dns` | DNS | Standalone DNS resolution with resolved IPs | No TCP |
| `tls` | TCP + TLS | Full cert chain, cipher suite, ALPN, expiry | No HTTP body |
| `udp` | UDP | RTT min/avg/p95, jitter, loss% | UDP echo server |
| `udpdownload` | UDP bulk | NWKT protocol throughput, server to client | Requires `--payload-sizes` |
| `udpupload` | UDP bulk | NWKT protocol throughput, client to server | Requires `--payload-sizes` |
| `download` | HTTP | Throughput MB/s, server to client | URL rewritten to `/download?bytes=N`; requires `--payload-sizes` |
| `upload` | HTTP | Throughput MB/s, client to server | URL rewritten to `/upload`; requires `--payload-sizes` |
| `webdownload` | HTTP | Download throughput from any URL (no rewrite) | Use for external URLs |
| `webupload` | HTTP | Upload throughput to any URL (no rewrite) | Requires `--payload-sizes` |
| `pageload` | HTTP/1.1 | Multi-asset page load, up to 6 parallel connections | Requires HTTPS target |
| `pageload2` | HTTP/2 | Multi-asset page load, single connection multiplexed | Requires HTTPS target |
| `pageload3` | HTTP/3 | Multi-asset page load, QUIC multiplexed | Requires HTTPS target |
| `browser` | Chromium | Real headless browser: Load/DCL/TTFB/bytes/protocols | Requires `--features browser` + Chrome |

### Aggregate shortcuts

- `pageload` by itself runs `pageload` (H1.1) + `pageload2` (H2) + `pageload3` (H3)
- `browser` by itself runs `browser1` (H1.1) + `browser2` (H2) + `browser3` (H3)

---

## Common Examples

### Latency comparison across protocols

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,http3 \
  --runs 20 \
  --insecure
```

### Download throughput

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes download \
  --payload-sizes 1m,10m,100m \
  --runs 5 \
  --insecure
```

### Upload throughput

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes upload \
  --payload-sizes 1m,10m \
  --runs 5 \
  --insecure
```

### Page-load comparison (H1 vs H2 vs H3)

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3 \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure
```

### Everything in one run

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes tcp,http1,http2,http3,udp,download,upload,pageload,pageload2,pageload3 \
  --payload-sizes 1m \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

### Multi-target comparison (local vs cloud)

```bash
networker-tester \
  --target http://127.0.0.1:8080/health \
  --target https://<cloud-ip>:8443/health \
  --modes tcp,http1,http2,http3,udp,download \
  --payload-sizes 1m \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

---

## All Flags

### Targeting

| Flag | Default | Description |
|------|---------|-------------|
| `--target URL` | `http://localhost:8080/health` | URL to probe. Repeat for multi-target: `--target URL1 --target URL2` |
| `--modes MODE,...` | `http1,http2,udp` | Comma-separated probe modes |
| `--runs N` | `3` | Repetitions per mode |
| `--concurrency N` | `1` | Concurrent requests per run |
| `--timeout N` | `30` | Per-request timeout in seconds |
| `--retries N` | `0` | Retry failed probes up to N times |

### Payload and throughput

| Flag | Default | Description |
|------|---------|-------------|
| `--payload-sizes LIST` | — | Sizes for download/upload (e.g. `64k,1m,10m`). Each size = separate probe. |

Size suffixes: `k` = KiB, `m` = MiB, `g` = GiB.

### Page-load tuning

| Flag | Default | Description |
|------|---------|-------------|
| `--page-assets N` | `20` | Number of assets per page-load simulation |
| `--page-asset-size SZ` | `10k` | Size of each simulated asset |

### UDP

| Flag | Default | Description |
|------|---------|-------------|
| `--udp-port N` | `9999` | UDP echo server port |
| `--udp-probes N` | `10` | Probe packets per run |

### Connection options

| Flag | Default | Description |
|------|---------|-------------|
| `--insecure` | — | Skip TLS certificate verification (for self-signed endpoint certs) |
| `--ipv4-only` | — | Restrict to IPv4 |
| `--ipv6-only` | — | Restrict to IPv6 |
| `--no-proxy` | — | Bypass system proxy |
| `--connection-reuse` | — | Reuse a single TCP connection across requests |

### Output

| Flag | Default | Description |
|------|---------|-------------|
| `--output-dir PATH` | `./output` | Directory for reports |
| `--html` | — | Write HTML report to `output/report.html` |
| `--excel` | — | Write Excel workbook to `output/report.xlsx` (8 sheets) |
| `--json` | — | Write structured JSON to stdout |
| `--verbose` / `-v` | — | Enable debug logging |

---

## Output Formats

### Terminal output

Every probe prints one line as it completes:

```
✓ #1 [http2] 200 HTTP/2  DNS:0.3ms TCP:0.2ms TLS:3.9ms TTFB:0.4ms Total:4.6ms CPU:0.9ms CSW:3v/0i
✓ #1 [http3] 200 HTTP/3  QUIC:1.5ms TTFB:0.4ms Total:2.1ms CPU:2.4ms CSW:4v/1i
✓ #1 [download] 10.0 MiB TLS:4.1ms TTFB:8.2ms Total:95.3ms Throughput:105.2 MB/s Goodput:98.1 MB/s
```

HTTP/3 shows `QUIC:Xms` instead of separate `DNS:` / `TCP:` / `TLS:` phases, because QUIC combines transport and crypto into a single UDP-based handshake.

### HTML report (`--html`)

Sections for a single target:
1. Run Summary — target, modes, attempt count, duration
2. Timing Breakdown — per-protocol averages: DNS/TCP/TLS/TTFB/Total
3. Protocol Comparison — side-by-side table with % delta
4. Throughput Results — MB/s per payload size
5. All Attempts — every individual probe
6. TCP Stats — kernel socket metrics
7. TLS Details — cipher suite, certificate info
8. UDP Statistics — RTT, jitter, loss

For multiple targets, a **Cross-Target Comparison** table appears at the top with % delta vs the first (baseline) target.

### Excel workbook (`--excel`)

Eight worksheets: Summary, HTTP Timings, TCP Stats, TLS Details, UDP Stats, Throughput, Server Timing, Errors.

### JSON (`--json`)

Structured per-attempt output. Naming:
- Single target: `run-YYYYMMDD-HHMMSS.json`
- Multiple targets: `run-YYYYMMDD-HHMMSS-1.json`, `run-YYYYMMDD-HHMMSS-2.json`, ...

---

## Configuration File

Both binaries accept a JSON config file via `--config` / `-c`. CLI flags override config file values.

```json
{
  "targets": [
    "http://127.0.0.1:8080/health",
    "https://remote:8443/health"
  ],
  "modes": ["http1", "http2", "http3", "udp"],
  "runs": 5,
  "payload_sizes": ["1m"],
  "insecure": true,
  "output_dir": "./output",
  "excel": false
}
```

```bash
networker-tester --config my-config.json
# Override a single key at run time:
networker-tester --config my-config.json --runs 10
```

### Endpoint config

```json
{
  "http_port": 8080,
  "https_port": 8443,
  "udp_port": 9999,
  "log_level": "info"
}
```

```bash
networker-endpoint --config endpoint.json
```

---

## Endpoint Routes

| Route | Method | Description |
|-------|--------|-------------|
| `/` | GET | HTML landing page with server info |
| `/health` | GET | `{"status":"ok"}` with version header |
| `/download?bytes=N` | GET | Returns N bytes; adds `Server-Timing: proc;dur=X` |
| `/upload` | POST | Drains body; adds `Server-Timing: recv;dur=X` |
| `/page` | GET | JSON manifest for pageload probes |
| `/browser-page` | GET | HTML page with img tags for browser probes |
| `/echo` | POST | Echoes request body |
| `/delay?ms=N` | GET | Sleeps N ms then responds |
| `/headers` | GET | Returns all received headers as JSON |
