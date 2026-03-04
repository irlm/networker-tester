# Testing Guide: H1 vs H2 vs H3 Protocol Comparison

This guide shows how to use `networker-tester` + `networker-endpoint` to run a fair,
reproducible comparison of HTTP/1.1, HTTP/2, and HTTP/3 across the metrics that matter
for a research-quality protocol evaluation:

| Metric | Why it matters |
|---|---|
| **TTFB** | Head-of-line blocking sensitivity; H/2 and H/3 should win on high-latency links |
| **Throughput** | Raw transfer rate; H/1.1 often competitive on single large transfers |
| **Goodput** | True end-to-end rate (incl. connection setup); penalizes QUIC's longer handshake |
| **CPU** | QUIC does TLS in userspace — should be measurably higher than H/1.1 or H/2 |
| **Client CSW** | Voluntary/involuntary context switches; reflects I/O concurrency differences |
| **Server CSW** | Server-side scheduling cost per request |
| **Page-load total** | Wall-clock impact of multiplexing; H/2 and H/3 win on multi-asset pages |

---

## Prerequisites

### 1. Build

```bash
cargo build --release
```

HTTP/3 is included by default. No extra flags needed.

### 2. Start the endpoint

```bash
# Both plain-HTTP (8080) and HTTPS/QUIC (8443) on localhost
./target/release/networker-endpoint
```

> **Windows note:** Replace `./target/release/` with `.\target\release\` and add `.exe`
> to binary names. Use backtick `` ` `` instead of `\` for line continuation in PowerShell.
> Examples in this guide use Unix syntax; PowerShell equivalents are shown where the syntax
> differs meaningfully (loops, `open`).

The endpoint serves:
- `/health` — tiny JSON response for latency probes
- `/download?bytes=N` — generates N bytes of body on the fly
- `/upload` — drains request body; reports receive time in `Server-Timing`
- `/page` + `/asset?size=N` — simulates HTML page + parallel asset fetches

---

## Test 1: Simple Request Latency (H1 vs H2 vs H3)

Measures the per-request overhead of each protocol on a tiny payload.

```bash
# HTTP/1.1 and HTTP/2 (plain HTTP and HTTPS)
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2 \
  --runs 20 \
  --insecure

# HTTP/3
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http3 \
  --runs 20 \
  --insecure
```

**What to look for in terminal output:**
```
✓ #1 [http1] 200 HTTP/1.1 DNS:0.3ms TCP:0.2ms TLS:4.1ms TTFB:0.5ms Total:4.8ms CPU:0.8ms CSW:2v/0i
✓ #1 [http2] 200 HTTP/2    DNS:0.3ms TCP:0.2ms TLS:3.9ms TTFB:0.4ms Total:4.6ms CPU:0.9ms CSW:3v/0i
✓ #1 [http3] 200 HTTP/3    QUIC:1.5ms TTFB:0.4ms Total:2.1ms CPU:2.4ms CSW:4v/1i
```

HTTP/3 has no separate DNS or TCP phase — QUIC is UDP-based and combines transport +
crypto into one handshake, shown as `QUIC:Xms`. HTTP/1.1 and HTTP/2 show `DNS:` and
`TCP:` separately because those are distinct phases before the `TLS:` handshake.

**Expected patterns:**
- `CPU` for HTTP/3 is notably higher — QUIC encryption runs in userspace (no kernel offload)
- `TTFB` should be similar on loopback; differences appear on real network links
- `CSW` higher for H/3 reflects more context switches from the async QUIC runtime

---

## Test 2: Download Throughput — Single Large Transfer

Measures sustained download rate with different payload sizes.

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes download \
  --payload-sizes 1m,10m,100m \
  --runs 5 \
  --insecure
```

Then repeat selecting a specific HTTP version by running against the plain-HTTP port
(HTTP/1.1 only) or forcing via TLS:

```bash
# Force HTTP/1.1 (plain — no TLS negotiation)
./target/release/networker-tester \
  --target http://127.0.0.1:8080/health \
  --modes download \
  --payload-sizes 1m,10m \
  --runs 5
```

**What to look for:**
```
✓ #1 [download] 10.0 MiB TLS:4.1ms TTFB:8.2ms Total:95.3ms Throughput:105.2 MB/s Goodput:98.1 MB/s CPU:12.4ms CSW:48v/3i sCSW:2v/0i
```

- **Goodput ≤ Throughput**: always, because goodput includes connection setup overhead
- **Goodput gap is larger for HTTP/3**: QUIC handshake is longer, hurting small transfers more
- **CPU higher for HTTP/3**: QUIC processes every packet in userspace
- **Server CSW** (`sCSW`) reflects server-side scheduling cost; typically low for download

---

## Test 3: Upload Throughput

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes upload \
  --payload-sizes 1m,10m \
  --runs 5 \
  --insecure
```

**Notes:**
- Upload `Throughput` uses `max(server_recv_ms, ttfb_ms)` to avoid near-zero readings
- `Server-Timing: recv;dur=X` reports server body-drain time; included in server CSW
- Goodput includes TLS overhead, which is more costly for H/3

---

## Test 4: Browser-Like Page Load (the key H1 vs H2 vs H3 test)

This is the most realistic comparison. A browser loads an HTML page then fetches N
parallel assets. HTTP/1.1 opens up to 6 TCP connections; HTTP/2 and H/3 multiplex all
assets over one connection.

```bash
# HTTP/1.1: up to 6 parallel TCP connections
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure

# HTTP/2: single connection, all assets multiplexed
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload2 \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure

# HTTP/3: single QUIC connection, all assets multiplexed
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload3 \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure
```

Or run all four in a single invocation (including the real-browser probe):

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3,browser \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure
```

> `browser` requires `cargo build --release --features browser` and a local Chrome/Chromium
> installation. Drop it from `--modes` if Chrome is not available — the run continues with
> the other three modes.

**What to look for:**

| Metric | H/1.1 | H/2 | H/3 |
|---|---|---|---|
| `total_ms` | Highest (HOL blocking × 6 conns) | Lower (full multiplex) | Similar to H/2 |
| `connections_opened` | Up to 6 | 1 | 1 |
| `tls_overhead_ratio` | Higher (6 TLS handshakes) | Lower (1 TLS handshake) | Similar to H/2 |
| `cpu_time_ms` | Lowest | Moderate | Highest (QUIC userspace) |
| `ttfb_ms` | Lowest | Low | Low |

**Key insight:** The `pageload` CPU + connection count tells the full story — H/2 wins on
connection overhead; H/3 wins on head-of-line blocking (at the packet level) but pays
more CPU.

---

## Test 5: Varied Asset Count (Simulating Different Page Complexity)

Run a sweep across asset counts to find the crossover point where H/2 multiplexing
starts to beat H/1.1's parallel connections:

```bash
# bash / zsh (macOS, Linux)
for N in 5 10 20 40 80; do
  echo "=== $N assets ==="
  ./target/release/networker-tester \
    --target https://127.0.0.1:8443/health \
    --modes pageload,pageload2 \
    --page-assets $N \
    --page-asset-size 10k \
    --runs 5 \
    --insecure
done
```

```powershell
# PowerShell (Windows)
foreach ($N in 5, 10, 20, 40, 80) {
    Write-Host "=== $N assets ==="
    .\target\release\networker-tester.exe `
        --target https://127.0.0.1:8443/health `
        --modes pageload,pageload2 `
        --page-assets $N `
        --page-asset-size 10k `
        --runs 5 `
        --insecure
}
```

Expect H/2 to break even or win at around 6-10 assets (the browser-connection-limit
threshold) and pull ahead at higher counts.

---

## Test 6: Varied Asset Size (Simulating Different Content Types)

Small assets expose connection overhead; large assets expose transfer efficiency:

```bash
# bash / zsh (macOS, Linux)
for SZ in 1k 10k 100k 1m; do
  echo "=== $SZ assets ==="
  ./target/release/networker-tester \
    --target https://127.0.0.1:8443/health \
    --modes pageload,pageload2,pageload3 \
    --page-assets 20 \
    --page-asset-size $SZ \
    --runs 5 \
    --insecure
done
```

```powershell
# PowerShell (Windows)
foreach ($SZ in '1k', '10k', '100k', '1m') {
    Write-Host "=== $SZ assets ==="
    .\target\release\networker-tester.exe `
        --target https://127.0.0.1:8443/health `
        --modes pageload,pageload2,pageload3 `
        --page-assets 20 `
        --page-asset-size $SZ `
        --runs 5 `
        --insecure
}
```

---

## Test 7: Multi-Target Comparison (Local vs Remote)

The most powerful use-case: run the same probe set against two (or more) endpoints in one
invocation and get a single HTML report with a side-by-side comparison table.

### Setup

Start a local endpoint:

```bash
./target/release/networker-endpoint
```

Deploy a remote endpoint via the installer:

```bash
bash install.sh --azure endpoint   # or --aws endpoint
```

The installer writes `networker-cloud.json` pointing at the remote VM's IP.

### Run — local vs cloud

```bash
./target/release/networker-tester \
  --target http://127.0.0.1:8080/health \
  --target https://<cloud-ip>:8443/health \
  --modes tcp,http1,http2,http3,udp,download,pageload,pageload2,pageload3 \
  --payload-sizes 1m \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

### Via config file (equivalent)

```json
{
  "targets": [
    "http://127.0.0.1:8080/health",
    "https://<cloud-ip>:8443/health"
  ],
  "modes": ["tcp", "http1", "http2", "http3", "udp", "download",
            "pageload", "pageload2", "pageload3"],
  "payload_sizes": ["1m"],
  "runs": 5,
  "insecure": true
}
```

### What to look for in the HTML report

The report opens with a **Multi-Target Summary** table (one row per target) and a
**Cross-Target Protocol Comparison** table. The comparison shows the primary metric for
each protocol (e.g. `total_duration_ms` for HTTP, `throughput_mbps` for download) with
the % delta vs the first (local) target:

| What to compare | What it tells you |
|-----------------|-------------------|
| `http1`/`http2`/`http3` total_ms delta | Extra RTT added by WAN vs loopback |
| `download` throughput delta | Available bandwidth to the remote endpoint |
| `pageload`/`pageload2`/`pageload3` total_ms delta | Real page-load impact of WAN latency |
| `tcp` connect_ms delta | Raw TCP setup cost over the network link |
| `udp` rtt_avg_ms delta | UDP path latency (incl. NAT/firewall path difference) |

**Typical patterns:**
- WAN HTTP total_ms 5–50× higher than loopback due to RTT
- Download throughput limited to available uplink/downlink of both sides
- H3 may show larger delta than H2 on high-latency links due to QUIC handshake overhead
- `%diff` in green = remote is faster (unusual for loopback baseline); red = slower (expected)

---

## Exporting Results

### JSON (default — every run)

Output is logged to `stdout` as structured JSON by default when `--json` is passed:

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2 \
  --runs 20 \
  --insecure \
  --json > results.json
```

### HTML Report

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,download,pageload,pageload2 \
  --payload-sizes 1m \
  --page-assets 20 \
  --runs 5 \
  --insecure \
  --html
```

```bash
open output/report.html        # macOS
xdg-open output/report.html   # Linux
```
```powershell
Invoke-Item output\report.html  # Windows
```

The HTML report includes a **Protocol Comparison** table and a **Throughput Results** table
with Goodput, CPU, Client CSW, and Server CSW columns.

### Excel Spreadsheet

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,download,pageload,pageload2 \
  --payload-sizes 1m \
  --page-assets 20 \
  --runs 5 \
  --insecure \
  --excel
```

```bash
open output/report.xlsx        # macOS
xdg-open output/report.xlsx   # Linux
```
```powershell
Invoke-Item output\report.xlsx  # Windows
```

---

## Interpreting CPU and CSW Results

### CPU time (`cpu_time_ms`)

Measures process CPU (user + system) consumed **during that one probe** using
`cpu_time::ProcessTime`. It reflects both the cryptographic cost and the runtime overhead
of each protocol stack.

**Typical pattern on loopback (no bandwidth constraint):**
- `http1`: lowest — kernel handles TCP; rustls handles one TLS record layer
- `http2`: slightly higher — HPACK compression adds a small cost
- `http3`: noticeably higher (2-5×) — QUIC encrypts/decrypts every UDP packet in userspace

**On a real network with large payloads:**
The gap grows further because H/3 processes more packets (MTU-limited datagrams vs TCP
segments that can be larger).

### Context switches (`csw_voluntary` / `csw_involuntary`)

Measured via `getrusage(RUSAGE_SELF)` delta around the probe (Unix only).

- **Voluntary** (`csw_voluntary`): the process yielded the CPU (e.g., waiting for I/O).
  Higher for async I/O-heavy protocols.
- **Involuntary** (`csw_involuntary`): the kernel preempted the process (e.g., time-slice
  expired or higher-priority task). Higher under CPU pressure.

Server CSW (`sCSW`) is reported by the endpoint via `Server-Timing: csw-v;dur=N, csw-i;dur=N`.

---

## Quick Reference: All Flags Used Above

| Flag | Default | Purpose |
|---|---|---|
| `--target URL` | required | Base URL of the server. Repeat for multi-target comparison: `--target URL1 --target URL2` |
| `--modes MODE,...` | `http1` | Comma-separated probe modes |
| `--runs N` | `3` | Probes per mode per payload size |
| `--insecure` | false | Skip TLS certificate verification |
| `--payload-sizes LIST` | none | Sizes for download/upload (e.g., `64k,1m`) |
| `--page-assets N` | `20` | Assets per page-load simulation |
| `--page-asset-size SZ` | `10k` | Size of each simulated asset |
| `--html` | false | Write HTML report to `output/report.html` |
| `--excel` | false | Write Excel report to `output/report.xlsx` |
| `--json` | false | Write JSON results to stdout |
| `--retries N` | `0` | Retry failed probes |
| `--no-default-features` | off | Exclude HTTP/3 for a minimal build |

See `networker-tester --help` for the full list.
