# Testing Guide: H1 vs H2 vs H3 Protocol Comparison

This guide shows how to use `networker-tester` + `networker-endpoint` to run a fair,
reproducible comparison of HTTP/1.1, HTTP/2, and HTTP/3 across the metrics that matter
for a research-quality protocol evaluation:

| Metric | Why it matters |
|---|---|
| **TTFB** | Head-of-line blocking sensitivity; H/2 and H/3 should win on high-latency links |
| **Throughput** | Raw transfer rate; H/1.1 often competitive on single large transfers |
| **Goodput** | True end-to-end rate (incl. connection setup); penalises QUIC's longer handshake |
| **CPU** | QUIC does TLS in userspace — should be measurably higher than H/1.1 or H/2 |
| **Client CSW** | Voluntary/involuntary context switches; reflects I/O concurrency differences |
| **Server CSW** | Server-side scheduling cost per request |
| **Page-load total** | Wall-clock impact of multiplexing; H/2 and H/3 win on multi-asset pages |

---

## Prerequisites

### 1. Build

```bash
# HTTP/1.1 and HTTP/2 — always available
cargo build --release

# HTTP/3 — compile in QUIC support
cargo build --release --features http3
```

### 2. Start the endpoint

```bash
# Both plain-HTTP (8080) and HTTPS/QUIC (8443) on localhost
./target/release/networker-endpoint
```

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

# HTTP/3 (requires --features http3 build)
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http3 \
  --runs 20 \
  --insecure
```

**What to look for in terminal output:**
```
✓ #1 [http1] 200 HTTP/1.1 DNS:0.3ms TCP:0.2ms TLS:4.1ms TTFB:0.5ms Total:4.8ms CPU:0.8ms CSW:2v/0i
✓ #1 [http2] 200 HTTP/2 DNS:0.3ms TCP:0.2ms TLS:3.9ms TTFB:0.4ms Total:4.6ms CPU:0.9ms CSW:3v/0i
✓ #1 [http3] 200 HTTP/3 DNS:0.3ms TCP:0.3ms TLS:3.7ms TTFB:0.4ms Total:4.5ms CPU:2.4ms CSW:4v/1i
```

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

# HTTP/3: single QUIC connection, all assets multiplexed (--features http3 build)
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload3 \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure
```

Or run all three in a single invocation (requires `--features http3`):

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3 \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure
```

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

Expect H/2 to break even or win at around 6-10 assets (the browser-connection-limit
threshold) and pull ahead at higher counts.

---

## Test 6: Varied Asset Size (Simulating Different Content Types)

Small assets expose connection overhead; large assets expose transfer efficiency:

```bash
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

open output/report.html
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

open output/report.xlsx
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
| `--target URL` | required | Base URL of the server |
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
| `--features http3` | off | Enable HTTP/3 (compile-time flag) |

See `networker-tester --help` for the full list.
