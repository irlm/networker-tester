# Protocol Comparison: H1 vs H2 vs H3

This guide walks through a fair, reproducible comparison of HTTP/1.1, HTTP/2, and HTTP/3 using `networker-tester` + `networker-endpoint`.

---

## Background: What Changes Between Protocols

| Aspect | HTTP/1.1 | HTTP/2 | HTTP/3 |
|--------|----------|--------|--------|
| Transport | TCP | TCP | QUIC (UDP) |
| Multiplexing | None (pipelining unreliable) | Full (streams over one TCP conn) | Full (streams over one QUIC conn) |
| Head-of-line blocking | TCP + HTTP level | TCP level only | None (independent QUIC streams) |
| TLS | Kernel-offloaded | Kernel-offloaded | Userspace (per-datagram encryption) |
| Connections for page loads | Up to 6 per host | 1 | 1 |
| Handshake | DNS + TCP + TLS (3 phases) | DNS + TCP + TLS (3 phases) | QUIC (combined, shown as `QUIC:Xms`) |

---

## Setup

### 1. Start the endpoint

```bash
# HTTP :8080 and HTTPS/QUIC :8443
networker-endpoint
```

HTTP/3 is enabled by default. UDP port 8443 must not be firewalled.

### 2. Build (if from source)

```bash
cargo build --release
# HTTP/3 is included in the default build — no extra flags needed
```

---

## Test 1: Request Latency (tiny payload)

Measures per-request overhead on a tiny `/health` response.

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,http3 \
  --runs 20 \
  --insecure
```

**Sample output:**
```
✓ #1 [http1] 200 HTTP/1.1 DNS:0.3ms TCP:0.2ms TLS:4.1ms TTFB:0.5ms Total:4.8ms CPU:0.8ms CSW:2v/0i
✓ #1 [http2] 200 HTTP/2   DNS:0.3ms TCP:0.2ms TLS:3.9ms TTFB:0.4ms Total:4.6ms CPU:0.9ms CSW:3v/0i
✓ #1 [http3] 200 HTTP/3   QUIC:1.5ms TTFB:0.4ms Total:2.1ms CPU:2.4ms CSW:4v/1i
```

**What to look for:**
- `CPU` for HTTP/3 is 2-5x higher — QUIC encryption runs in userspace
- HTTP/3 shows `QUIC:Xms` instead of `DNS:` / `TCP:` / `TLS:` because QUIC combines all three phases
- `TTFB` is similar on loopback; differences appear over real network links
- `CSW` (context switches) is higher for H3 due to async QUIC runtime overhead

---

## Test 2: Download Throughput

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes download \
  --payload-sizes 1m,10m,100m \
  --runs 5 \
  --insecure
```

**Sample output:**
```
✓ #1 [download] 10.0 MiB TLS:4.1ms TTFB:8.2ms Total:95.3ms Throughput:105.2 MB/s Goodput:98.1 MB/s CPU:12.4ms CSW:48v/3i sCSW:2v/0i
```

**Key metrics:**
- **Throughput** = `payload_bytes / (total_duration_ms - ttfb_ms)` — body transfer phase only
- **Goodput** = `payload_bytes / (dns + tcp + tls + total_duration_ms)` — full delivery including setup
- **Goodput < Throughput** always, because setup overhead is included
- **The goodput gap is larger for H3** — the QUIC handshake is longer, which hurts small transfers more

To isolate HTTP versions, force H1.1 by targeting the plain port:

```bash
# Force HTTP/1.1 (no TLS negotiation)
networker-tester \
  --target http://127.0.0.1:8080/health \
  --modes download \
  --payload-sizes 1m,10m \
  --runs 5
```

---

## Test 3: Upload Throughput

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes upload \
  --payload-sizes 1m,10m \
  --runs 5 \
  --insecure
```

---

## Test 4: Page-Load Comparison (the key test)

This is the most realistic comparison. A simulated browser loads an HTML page then fetches N parallel assets. HTTP/1.1 opens up to 6 TCP connections; HTTP/2 and H/3 multiplex all assets over a single connection.

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3 \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure \
  --output-dir ./output
```

Or run all three plus real-browser in one invocation:

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes pageload,pageload2,pageload3,browser \
  --page-assets 30 \
  --page-asset-size 50k \
  --runs 10 \
  --insecure \
  --output-dir ./output
```

> `browser` requires `cargo build --release --features browser` and Chrome/Chromium installed.

**Expected results:**

| Metric | H1.1 (`pageload`) | H2 (`pageload2`) | H3 (`pageload3`) |
|--------|-------------------|-------------------|-------------------|
| `total_ms` | Highest (HOL blocking + 6 conns) | Lower (full multiplex) | Similar to H/2 |
| `connections_opened` | Up to 6 | 1 | 1 |
| `tls_overhead_ratio` | Higher (6 TLS handshakes) | Lower (1 TLS handshake) | Similar to H/2 |
| `cpu_time_ms` | Lowest | Moderate | Highest (QUIC userspace) |
| `ttfb_ms` | Lowest on loopback | Low | Low |

---

## Test 5: Asset Count Sweep (finding the H2 crossover)

H2 multiplexing pulls ahead of H1.1 connection pools at around 6-10 assets — the browser connection limit. Run a sweep to find the crossover in your environment:

```bash
# macOS / Linux
for N in 5 10 20 40 80; do
  echo "=== $N assets ==="
  networker-tester \
    --target https://127.0.0.1:8443/health \
    --modes pageload,pageload2 \
    --page-assets $N \
    --page-asset-size 10k \
    --runs 5 \
    --insecure
done
```

```powershell
# Windows PowerShell
foreach ($N in 5, 10, 20, 40, 80) {
    Write-Host "=== $N assets ==="
    networker-tester.exe `
        --target https://127.0.0.1:8443/health `
        --modes pageload,pageload2 `
        --page-assets $N `
        --page-asset-size 10k `
        --runs 5 `
        --insecure
}
```

---

## Test 6: Asset Size Sweep

Small assets expose connection overhead; large assets expose transfer efficiency:

```bash
for SZ in 1k 10k 100k 1m; do
  echo "=== $SZ ==="
  networker-tester \
    --target https://127.0.0.1:8443/health \
    --modes pageload,pageload2,pageload3 \
    --page-assets 20 \
    --page-asset-size $SZ \
    --runs 5 \
    --insecure
done
```

---

## Interpreting CPU and Context Switch Results

### CPU (`cpu_time_ms`)

Measures process CPU (user + system) consumed during that one probe using `ProcessTime`. Reflects both cryptographic cost and runtime overhead.

**Typical pattern on loopback:**
- `http1`: lowest — kernel handles TCP; one TLS record layer
- `http2`: slightly higher — HPACK header compression adds a small cost
- `http3`: noticeably higher (2-5x) — QUIC encrypts/decrypts every UDP packet in userspace

**On a real network with large payloads:** the gap grows further because H3 processes more packets (MTU-limited datagrams vs. larger TCP segments).

### Context switches (`CSW`)

Measured via `getrusage` delta around the probe (Unix only).

- **Voluntary** (`v`): the process yielded the CPU (e.g., waiting for I/O). Higher for async I/O-heavy protocols.
- **Involuntary** (`i`): the kernel preempted the process. Higher under CPU pressure.

Server CSW (`sCSW`) is reported by the endpoint via `Server-Timing: csw-v;dur=N, csw-i;dur=N`.

---

## What to Compare in the HTML Report

| What to compare | What it tells you |
|-----------------|-------------------|
| `pageload` vs `pageload2` total time | Benefit of H2 multiplexing over H1.1 connection pools |
| `pageload2` vs `pageload3` total time | QUIC advantage (especially on lossy or high-latency links) |
| `connections_opened` in `pageload` | Should be ≤ 6; more connections = more TLS handshake cost |
| `tls_overhead_ratio` in `pageload` | TLS as % of total time — high ratio means connection reuse helps |
| `browser` Load event vs `pageload` total | Gap = browser render/parse/JS overhead on top of network |
| `browser` per-protocol counts | `h2×18 h3×2` means mixed protocol negotiation |
| `http1`/`http2`/`http3` total_ms delta (multi-target) | Extra RTT added by WAN vs loopback |
| `download` throughput delta (multi-target) | Available bandwidth to the remote endpoint |

---

## Generating a Full Comparison Report

```bash
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,http3,udp,download,upload,pageload,pageload2,pageload3 \
  --payload-sizes 1m \
  --page-assets 20 \
  --page-asset-size 10k \
  --runs 10 \
  --insecure \
  --output-dir ./output

open output/report.html        # macOS
xdg-open output/report.html   # Linux
```
