# Networker Probes Reference

Each probe mode runs a specified measurement sequence. Each mode also writes a
matched set of JSON fields to the `RequestAttempt` output. This reference tells
you what each probe measures, which fields it writes, and the example CLI
commands.

The `Protocol` enum in `crates/networker-tester/src/metrics.rs` gives the
canonical list of modes in [`shared/modes.json`](../shared/modes.json). This
document describes every mode in that list.

## Target requirements

Each mode needs one of these kinds of target:

- **Arbitrary URL** — the probe uses any HTTP(S)/UDP target that you give it
  (`tcp`, `dns`, `tls`, `tlsresume`, `native`, `curl`, `udp`, `http1`, `http2`, `http3`).
- **`networker-endpoint` target** — the probe needs the diagnostic server's routes
  (`/download`, `/upload`, `/page`, `/asset`, UDP throughput port)
  (`download`, `download1`–`download3`, `upload`, `upload1`–`upload3`, `webdownload`,
  `webupload`, `udpdownload`, `udpupload`, `pageload`, `pageload2`, `pageload3`).
- **LagHound SDK endpoint** — the probe uses a customer-embedded LagHound endpoint that emits
  `Server-Timing` (`sdkprobe`).
- **Chrome/Chromium required** — the probe needs a local Chrome install and `--features browser`
  (`browser`, `browser1`, `browser2`, `browser3`).

---

## Common Fields (all probes that reach HTTP)

| JSON field | Unit | Description |
|---|---|---|
| `dns.duration_ms` | ms | Recursive DNS resolution time |
| `tcp.connect_duration_ms` | ms | TCP 3-way handshake duration |
| `tls.handshake_duration_ms` | ms | TLS handshake (absent for plain HTTP) |
| `http.ttfb_ms` | ms | Time to first response byte |
| `http.total_duration_ms` | ms | Full HTTP round-trip (headers + body) |
| `http.status_code` | int | HTTP response status |
| `http.negotiated_version` | string | `HTTP/1.1`, `HTTP/2`, `HTTP/3` |
| `http.cpu_time_ms` | ms | Process CPU (user+sys) consumed this probe |
| `http.csw_voluntary` | count | Client voluntary context switches (Unix only) |
| `http.csw_involuntary` | count | Client involuntary context switches (Unix only) |
| `server_timing.recv_ms` | ms | Server body-drain time (upload only) |
| `server_timing.proc_ms` | ms | Server body-generation time (download only) |
| `server_timing.srv_csw_voluntary` | count | Server voluntary CSW (endpoint only) |
| `server_timing.srv_csw_involuntary` | count | Server involuntary CSW (endpoint only) |

---

## `tcp` — TCP Connect Only

This mode measures DNS resolution and the TCP 3-way handshake. It does no HTTP.

```bash
networker-tester --target http://example.com/health --modes tcp --runs 10
```

**Populated:** `dns`, `tcp`
**Terminal:** `DNS:0.5ms TCP:1.2ms`

---

## `http1` — HTTP/1.1

This mode measures the DNS → TCP → HTTP/1.1 request and response. It does not
need TLS for plain HTTP.

```bash
networker-tester --target http://example.com/health --modes http1 --runs 10
```

With TLS:
```bash
networker-tester --target https://example.com/health --modes http1 --runs 10
```

**Populated:** `dns`, `tcp`, `tls` (if HTTPS), `http` (all fields including `cpu_time_ms`, `csw_*`)
**Terminal:** `DNS:0.5ms TCP:1.2ms TLS:12.4ms TTFB:3.1ms Total:15.8ms CPU:2.3ms CSW:12v/3i`

---

## `http2` — HTTP/2

This mode measures the DNS → TCP → TLS (ALPN `h2`) → HTTP/2 request and
response. It needs TLS for ALPN negotiation.

```bash
networker-tester --target https://example.com/health --modes http2 --runs 10
```

**Populated:** same as `http1` plus `tls` always present
**Terminal:** same as `http1`. HPACK makes the CPU and CSW higher than h1 for large payloads.
**Note:** `http2` over plain HTTP fails with a TLS error.

---

## `tlsresume` — TLS session resumption

This mode makes two fresh TLS connections to the same HTTPS origin. The first
request seeds the resumption state. For TLS 1.3, this seed also lets the
`NewSessionTicket` arrive after the handshake. The second request succeeds only
when rustls reports a resumed handshake.

```bash
networker-tester --target https://www.microsoft.com/ --modes tlsresume --runs 3
```

**Populated:** `dns`, `tcp`, `tls`
**Terminal:** `cold=full:Xms warm=resumed:Yms resumed=true cold_http=200 warm_http=200`
**Notes:**
- The mode uses a real HTTP/1.1 request on both connections. The HTTP status can be non-2xx,
  but the transport result is still useful.
- Use this mode for HTTPS targets. The metric is the warm handshake duration. The extra TLS
  fields show the cold and warm handshake kind and the HTTP status codes.

---

## `http3` — HTTP/3 over QUIC

This mode measures the UDP-based QUIC handshake and then the HTTP/3 request and
response. The QUIC handshake is the equivalent of TCP plus TLS. The default
build includes this mode.

```bash
networker-tester --target https://example.com/health --modes http3 --runs 10 --insecure
```

**Populated:** `tls` (QUIC handshake, labeled `QUIC:` in terminal), `http` — `dns` and `tcp` are `None`
because QUIC combines transport + crypto into a single UDP-based handshake.
**Terminal:** `QUIC:Xms TTFB:Xms Total:Xms CPU:Xms CSW:Xv/Xi` — no `DNS:` or `TCP:` shown.
**Note:** `QUIC:Xms` is the full 1-RTT handshake, which includes TLS 1.3. The CPU is higher
than H/1.1 or H/2 because the encryption runs in userspace per UDP datagram, not in the
kernel TCP stack.

---

## `download` — Bulk HTTP Download (endpoint only)

This mode measures the end-to-end download throughput from the
`networker-endpoint` `/download` route. It **rewrites** the URL path
automatically: `/health` → `/download?bytes=N`.

```bash
networker-tester --target http://127.0.0.1:8080/health \
  --modes download --payload-sizes 64k,1m,10m --runs 5
```

**Populated:** all `http` fields plus `http.throughput_mbps`, `http.goodput_mbps`, `http.cpu_time_ms`, `http.csw_*`, `server_timing.proc_ms`, `server_timing.srv_csw_*`
**Terminal:**
```
✓ #1 [download] 10.0 MiB TLS:12.4ms TTFB:8.2ms Total:95.3ms Throughput:105.22 MB/s Goodput:98.1 MB/s CPU:2.3ms CSW:12v/3i sCSW:4v/1i
```

**Throughput:** `payload_bytes / (total_duration_ms − ttfb_ms)` — body receive phase only
**Goodput:** `payload_bytes / (dns_ms + tcp_ms + tls_ms + total_duration_ms)` — full delivery

---

## `upload` — Bulk HTTP Upload (endpoint only)

This mode measures the end-to-end upload throughput to the `networker-endpoint`
`/upload` route. It **rewrites** the URL path automatically.

```bash
networker-tester --target http://127.0.0.1:8080/health \
  --modes upload --payload-sizes 64k,1m,10m --runs 5
```

**Populated:** same as `download` but `server_timing.recv_ms` replaces `proc_ms`
**Terminal:** same format as `download`
**Throughput formula:** `max(server_recv_ms, ttfb_ms)` — whichever is larger avoids
near-zero readings when the server responds before fully draining the body.

---

## `webdownload` — Labeled Download Probe

This mode uses the built-in `networker-endpoint` route `GET /download?bytes=N`,
the same as `download`. The **protocol label** in the output and report is the
only difference (`webdownload` against `download`). Use this label to make
side-by-side comparison groups in a report.

```bash
networker-tester --target https://host:8443/health \
  --modes webdownload --payload-sizes 1m --runs 3 --insecure
```

**Populated:** same as `download`
**Note:** the mode rewrites the URL to `/download`. It does not fetch an arbitrary URL as-is.

---

## `webupload` — Labeled Upload Probe

This mode uses the built-in `networker-endpoint` route `POST /upload`, the same
as `upload`. The **protocol label** in the output and report is the only
difference (`webupload` against `upload`).

```bash
networker-tester --target https://host:8443/health \
  --modes webupload --payload-sizes 1m --runs 3 --insecure
```

**Populated:** same as `upload`

---

## `download1` / `download2` / `download3` — Protocol-Pinned Download (endpoint only)

These modes work the same as `download`, but they force the HTTP version. They
do not negotiate it. `download1` uses HTTP/1.1, `download2` uses HTTP/2, and
`download3` uses QUIC/HTTP3. Use these modes to compare the sustained download
throughput across protocol versions against the same `networker-endpoint`
`/download` route.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes download1,download2,download3 --payload-sizes 10m --runs 5 --insecure
```

**Requires:** `networker-endpoint`.
**Populated:** same as `download`; `http.negotiated_version` reflects the forced version.

---

## `upload1` / `upload2` / `upload3` — Protocol-Pinned Upload (endpoint only)

These modes work the same as `upload`, but they force the HTTP version. `upload1`
uses HTTP/1.1, `upload2` uses HTTP/2, and `upload3` uses QUIC/HTTP3. They run
against the `networker-endpoint` `/upload` route.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes upload1,upload2,upload3 --payload-sizes 10m --runs 5 --insecure
```

**Requires:** `networker-endpoint`.
**Populated:** same as `upload`; `http.negotiated_version` reflects the forced version.

---

## `udp` — UDP Echo

This mode measures the round-trip time for UDP packets to a UDP echo server.

```bash
networker-tester --target udp://example.com --modes udp \
  --udp-port 9999 --udp-probes 20 --runs 3
```

**Populated:** `udp` (min/mean/max/jitter RTT, loss %)
**Terminal:** `UDP RTT min/mean/max/jitter Loss`

---

## `udpdownload` / `udpupload` — UDP Bulk Throughput

These modes measure the bulk UDP throughput with the custom NWKT protocol on the
endpoint's UDP port (default 9998). They capture the datagram count, the loss,
and the effective throughput.

```bash
networker-tester --target http://127.0.0.1:8080/health \
  --modes udpdownload,udpupload --payload-sizes 1m --runs 3
```

**Populated:** `udp_throughput` (bytes_sent/received, datagrams, loss_percent, throughput_mbps)

---

## `dns` — Standalone DNS Resolution

This mode resolves the target hostname and records the results. It does not open
a TCP connection.

```bash
networker-tester --target http://example.com/health --modes dns --runs 5
```

**Populated:** `dns` (duration_ms, resolved IPs)
**Terminal:** `DNS:0.5ms → 93.184.216.34`

---

## `tls` — Standalone TLS Handshake

This mode does the DNS → TCP → TLS sequence only. It captures the full
certificate chain (subject, issuer, SANs, expiry), the cipher suite, the TLS
version, and the ALPN.

```bash
networker-tester --target https://example.com --modes tls --runs 5
```

**Populated:** `dns`, `tcp`, `tls` (all cert chain fields, cipher suite, TLS version)
**Terminal:** shows cert expiry, cipher, and ALPN

---

## `pageload` — HTTP/1.1 Multi-Asset Page Load

This mode simulates a browser page load. It fetches a root HTML page. It then
downloads N parallel assets over a maximum of 6 concurrent HTTP/1.1 connections.
This maximum matches the browser connection limits.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes pageload --page-assets 20 --page-asset-size 10k --runs 5 --insecure
```

**Populated:** `page_load` (asset_count, assets_fetched, total_bytes, total_ms, ttfb_ms, connections_opened, tls_overhead_ratio, cpu_time_ms)
**Terminal:** page load summary with asset count and timing
**Note:** Requires `networker-endpoint` (uses `/page` + `/asset` routes).

---

## `pageload2` — HTTP/2 Multiplexed Page Load

This mode works like `pageload`, but it uses one TLS connection with HTTP/2
multiplexing. All N assets are in flight at the same time. This mode shows the
H/2 multiplexing advantage.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes pageload2 --page-assets 20 --page-asset-size 10k --runs 5 --insecure
```

**Populated:** same as `pageload`
**Note:** TLS required for ALPN `h2`.

---

## `pageload3` — HTTP/3 Multiplexed Page Load

This mode works like `pageload2`, but it runs over QUIC. The default build
includes this mode.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes pageload3 --page-assets 20 --page-asset-size 10k --runs 5 --insecure
```

**Populated:** same as `pageload`
**Note:** a firewall must not block UDP. Use `--insecure` for self-signed certificates.

---

## `native` — System TLS Stack

This mode works like `http1`, but it uses the platform's native TLS library, not
rustls. The native library is Secure Transport on macOS, SChannel on Windows,
and OpenSSL on Linux. This mode needs `--features native`.

```bash
networker-tester --target https://example.com/health --modes native --runs 5
```

**Populated:** same as `http1`; `tls.tls_backend = "native-tls"`

---

## `curl` — System curl Binary

This mode runs the system `curl` binary. It reads the `--write-out` timing
fields from curl. Use this mode as a ground-truth baseline.

```bash
networker-tester --target https://example.com/health --modes curl --runs 5
```

**Populated:** `http` fields from curl's timing output; `tls.tls_backend = "curl"`

---

## `sdkprobe` — LagHound SDK Endpoint (server-time split)

This mode probes a **customer-embedded LagHound SDK endpoint**. It does not probe
the `networker-endpoint` diagnostic server or an arbitrary URL. It splits the
total time into DNS, TCP, TLS, network transfer, and server processing. It uses
the endpoint's `Server-Timing` header for this split.

```bash
networker-tester --target https://app.example.com/__laghound --modes sdkprobe --runs 10
```

**Requires:** a target that speaks the LagHound SDK contract (emits `Server-Timing`).
**Populated:** `dns`, `tcp`, `tls`, `http`, `server_timing` (server processing split out
from network transfer).

See [`sdk/`](sdk/README.md) and the [Application Network Performance report](reports-app-network.md).

---

## `browser` — Real Headless Chromium (CDP)

This mode drives a real headless Chromium instance through the Chrome DevTools
Protocol (chromiumoxide). It measures the true page-load performance that no
synthetic probe can copy. This mode needs `--features browser` at compile time
and a local Chrome/Chromium installation.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes browser --runs 3 --insecure
```

**Populated:** `browser` — `load_ms`, `dom_content_loaded_ms`, `ttfb_ms`, `resource_count`,
`transferred_bytes`, `protocol` (main-document ALPN), `resource_protocols` (per-protocol
resource counts, e.g. `[("h2", 18), ("h3", 2)]`)
**Terminal:** `[browser] proto=h2 TTFB:Xms DCL:Xms Load:Xms res=21 bytes=...`
**Note:** the mode rewrites the URL to `/page`, so you can compare the results
directly with `pageload` / `pageload2` / `pageload3`. The Chrome binary search
order is: `NETWORKER_CHROME_PATH` env var → system paths (`/usr/bin/google-chrome`,
etc. on Linux; `/Applications/Google Chrome.app/…` on macOS). When the mode finds
no Chrome binary, the probe returns a skipped `RequestAttempt`. It does not crash
the run.

---

## `browser1` / `browser2` / `browser3` — Protocol-Pinned Headless Chrome

These modes work like `browser`, but they force the transport that the real
Chrome uses:

- `browser1` — the mode disables HTTP/2, so Chrome uses HTTP/1.1.
- `browser2` — the mode disables QUIC, so Chrome uses HTTP/2.
- `browser3` — the mode forces QUIC with an origin flag and SPKI cert pinning, so Chrome uses HTTP/3.

The `--modes browser` CLI shorthand expands to `browser1,browser2,browser3`.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes browser1,browser2,browser3 --runs 3 --insecure
```

**Requires:** Chrome/Chromium + `--features browser`.
**Populated:** same as `browser`.

---

## URL Page-Load Diagnostic (CLI workflow)

This is not a classic `--modes ...` probe. It is a higher-level workflow. The
`--url-test-*` flags drive it. Use it for real website diagnostics, not for
synthetic endpoint comparison.

```bash
networker-tester \
  --url-test-url https://example.com \
  --url-test-capture-har \
  --url-test-capture-pcap \
  --output-dir ./output
```

**Captures:**
- browser-style page timings
- observed protocol for the primary load
- per-resource timing snapshot
- per-origin summary
- connection summary
- optional protocol validation probes
- optional HAR artifact
- optional PCAP artifact + structured packet summary

**Outputs:** JSON artifact always, plus optional HAR/PCAP artifacts when supported.
**Dashboard API:** `GET /api/projects/{projectId}/url-tests`, `GET /api/projects/{projectId}/url-tests/{run_id}`, `GET /api/projects/{projectId}/url-tests/{run_id}/sections`

---

## GeoIP / ASN Enrichment (optional, offline)

The tester can add geo/ISP/ASN context to each run for both sides of the path.
It adds `client_geo` (the client's egress IP) and `target_geo` (the first
resolved target IP) to the run JSON. It also adds a line to the summary and to
the HTML host cards. This enrichment is **strictly offline**: the lookups use
local MaxMind `.mmdb` files that you supply. The tester never downloads a
database. It never calls a runtime geolocation or "what's my IP" API.

```bash
# Get the free GeoLite2 databases (requires a free MaxMind account):
#   https://www.maxmind.com/en/geolite2/signup
# Download GeoLite2-City.mmdb and/or GeoLite2-ASN.mmdb, then:

export NETWORKER_GEOIP_CITY_DB=/var/lib/geoip/GeoLite2-City.mmdb
export NETWORKER_GEOIP_ASN_DB=/var/lib/geoip/GeoLite2-ASN.mmdb
networker-tester --target https://example.com/health --modes http1 --runs 3

# Or as flags:
networker-tester --target https://example.com/health --modes http1 \
  --geoip-city-db /var/lib/geoip/GeoLite2-City.mmdb \
  --geoip-asn-db  /var/lib/geoip/GeoLite2-ASN.mmdb
```

Behavior:

- **Absent or unreadable database → no enrichment.** The fields are simply
  omitted from the JSON; it is never an error (one debug log line).
- `client_geo` is only emitted when the local egress interface toward the
  target carries a **public** IP (typical for cloud VMs). Behind NAT the
  egress address is private/CGNAT and the client side stays unenriched —
  by design, since discovering the NAT'd public IP would require an external
  service call.
- `target_geo` uses the first resolved target IP (honoring
  `--ipv4-only`/`--ipv6-only`).
- Each `GeoInfo` carries `db_date` (the build date of the `.mmdb` used) so
  consumers can judge staleness. Keep your databases fresh — MaxMind updates
  GeoLite2 twice weekly.

**Populated:** `client_geo.{country,city,asn,as_org,db_date}`,
`target_geo.{country,city,asn,as_org,db_date}` (all optional, additive —
schema stays 1.0).

## Tester CPU Trust Envelope (`cpu_usage`)

Every run reports the tester's own CPU usage so consumers can judge whether
the measurements were taken on a quiet host. A whole-run mean alone can hide
a contention burst, so the tester also samples the CPU counters once per
second for the duration of the run:

```json
"cpu_usage": {
  "mean_busy_percent": 12.4,     // whole-run two-snapshot delta
  "max_busy_percent": 96.0,      // highest 1 s sample
  "p95_busy_percent": 41.2,      // only when sample_count >= 20 (20 s+ run)
  "mean_steal_percent": 0.3,     // Linux only — /proc/stat field 8
  "max_steal_percent": 2.1,      // Linux only
  "sample_count": 34,
  "sample_interval_ms": 1000
}
```

Sources and per-platform honesty (a field is `None`/omitted when the platform
cannot measure it — never fabricated):

| Platform | Busy source | Steal |
|----------|-------------|-------|
| Linux    | `/proc/stat` aggregate `cpu` line (idle incl. iowait) | field 8 (`steal`) — counted as **busy**, never idle |
| macOS    | `host_statistics(HOST_CPU_LOAD_INFO)` | none (no mach concept/API) |
| Windows  | `GetSystemTimes` (kernel time includes idle; `busy = 1 - idle/(kernel+user)`) | none (no API) |

Trust guards:

- **Min window**: any delta window under 500 ms (or under 20 elapsed ticks
  where 10 ms tick granularity applies) yields `None`, never fake precision.
- **p95 gating**: `p95_busy_percent` follows the same sample-size philosophy
  as attempt statistics (`MIN_SAMPLES_P95 = 20`) — at 1 s cadence a run
  shorter than ~20 s reports no p95.
- `client_load_after.cpu_busy_percent` is kept for compatibility and equals
  `cpu_usage.mean_busy_percent` (the whole-run mean).

The run summary prints `⚠ tester CPU-contended — measurements may be noisy`
when max/p95 busy exceeds 80% or any steal exceeds 1.5%.

### Benchmark publication gates

In benchmark mode the environment-check phase also brackets its RTT probes
with CPU snapshots (`benchmark_environment_check.cpu_busy_percent` /
`cpu_steal_percent`; subject to the same 500 ms min-window guard — widen
`--benchmark-environment-check-samples` / `--benchmark-environment-check-interval-ms`
if the default 5 × 50 ms window is too short on your path). A contended
tester then blocks publication-ready claims exactly like jitter/loss:

| Flag | Default | Blocks publication when |
|------|---------|-------------------------|
| `--benchmark-max-cpu-busy-percent`  | 85 | environment-check tester CPU busy ≥ threshold |
| `--benchmark-max-cpu-steal-percent` | 5  | environment-check tester CPU steal ≥ threshold (Linux only) |
