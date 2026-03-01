# Networker Probes Reference

Each probe mode runs a specific measurement sequence and populates a corresponding set of
JSON fields in the `RequestAttempt` output. This reference describes what each probe
measures, which fields it populates, and example CLI commands.

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

Measures: DNS resolution + TCP 3-way handshake. No HTTP.

```bash
networker-tester --target http://example.com/health --modes tcp --runs 10
```

**Populated:** `dns`, `tcp`
**Terminal:** `DNS:0.5ms TCP:1.2ms`

---

## `http1` — HTTP/1.1

Measures: DNS → TCP → HTTP/1.1 request/response. No TLS required (plain HTTP).

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

Measures: DNS → TCP → TLS (ALPN `h2`) → HTTP/2 request/response. TLS is required for
ALPN negotiation.

```bash
networker-tester --target https://example.com/health --modes http2 --runs 10
```

**Populated:** same as `http1` plus `tls` always present
**Terminal:** same as `http1` (CPU and CSW will be higher than h1 for large payloads due to HPACK)
**Note:** Attempting `http2` over plain HTTP will fail with a TLS error.

---

## `http3` — HTTP/3 over QUIC

Measures: UDP-based QUIC handshake (combines TCP+TLS equivalent) → HTTP/3
request/response. Included in the default build.

```bash
networker-tester --target https://example.com/health --modes http3 --runs 10 --insecure
```

**Populated:** `dns`, `tcp` (QUIC), `tls` (QUIC handshake), `http`
**Terminal:** same format; expect higher `CPU` than h1/h2 (QUIC encryption in userspace)
**Note:** QUIC bypasses kernel TCP stack — CPU cost reflects full userspace TLS per-packet.

---

## `download` — Bulk HTTP Download (endpoint only)

Measures end-to-end download throughput from the `networker-endpoint` `/download` route.
URL path is **rewritten** automatically: `/health` → `/download?bytes=N`.

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

Measures end-to-end upload throughput to the `networker-endpoint` `/upload` route. URL
path is **rewritten** automatically.

```bash
networker-tester --target http://127.0.0.1:8080/health \
  --modes upload --payload-sizes 64k,1m,10m --runs 5
```

**Populated:** same as `download` but `server_timing.recv_ms` replaces `proc_ms`
**Terminal:** same format as `download`
**Throughput formula:** `max(server_recv_ms, ttfb_ms)` — whichever is larger avoids
near-zero readings when the server responds before fully draining the body.

---

## `webdownload` — Arbitrary URL Download

Like `download` but uses the target URL **as-is** (no path rewriting). Works against any
HTTP server, not just `networker-endpoint`. Payload size is the actual response body.

```bash
networker-tester --target https://example.com/big-file.bin --modes webdownload --runs 3
```

**Populated:** same as `download` (server CSW will be absent unless using networker-endpoint)
**Note:** `--payload-sizes` is ignored — payload = actual response body length.

---

## `webupload` — Arbitrary URL Upload

Like `upload` but posts to the target URL as-is.

```bash
networker-tester --target https://example.com/upload \
  --modes webupload --payload-sizes 1m --runs 3
```

**Populated:** same as `upload`

---

## `udp` — UDP Echo

Measures round-trip time for UDP packets to a UDP echo server.

```bash
networker-tester --target udp://example.com --modes udp \
  --udp-port 9999 --udp-probes 20 --runs 3
```

**Populated:** `udp` (min/mean/max/jitter RTT, loss %)
**Terminal:** `UDP RTT min/mean/max/jitter Loss`

---

## `udpdownload` / `udpupload` — UDP Bulk Throughput

Measures bulk UDP throughput using the custom NWKT protocol on the endpoint's UDP port
(default 9998). Captures datagram count, loss, and effective throughput.

```bash
networker-tester --target http://127.0.0.1:8080/health \
  --modes udpdownload,udpupload --payload-sizes 1m --runs 3
```

**Populated:** `udp_throughput` (bytes_sent/received, datagrams, loss_percent, throughput_mbps)

---

## `dns` — Standalone DNS Resolution

Resolves the target hostname and records the results without opening a TCP connection.

```bash
networker-tester --target http://example.com/health --modes dns --runs 5
```

**Populated:** `dns` (duration_ms, resolved IPs)
**Terminal:** `DNS:0.5ms → 93.184.216.34`

---

## `tls` — Standalone TLS Handshake

Performs DNS → TCP → TLS only. Captures the full certificate chain (subject, issuer,
SANs, expiry), cipher suite, TLS version, and ALPN.

```bash
networker-tester --target https://example.com --modes tls --runs 5
```

**Populated:** `dns`, `tcp`, `tls` (all cert chain fields, cipher suite, TLS version)
**Terminal:** shows cert expiry, cipher, and ALPN

---

## `pageload` — HTTP/1.1 Multi-Asset Page Load

Simulates a browser page load: fetches a root HTML page and then downloads N parallel
assets over up to 6 concurrent HTTP/1.1 connections (matching browser connection limits).

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes pageload --page-assets 20 --page-asset-size 10k --runs 5 --insecure
```

**Populated:** `page_load` (asset_count, assets_fetched, total_bytes, total_ms, ttfb_ms, connections_opened, tls_overhead_ratio, cpu_time_ms)
**Terminal:** page load summary with asset count and timing
**Note:** Requires `networker-endpoint` (uses `/page` + `/asset` routes).

---

## `pageload2` — HTTP/2 Multiplexed Page Load

Like `pageload` but uses a single TLS connection with HTTP/2 multiplexing — all N assets
in-flight simultaneously. Demonstrates H/2 multiplexing advantage.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes pageload2 --page-assets 20 --page-asset-size 10k --runs 5 --insecure
```

**Populated:** same as `pageload`
**Note:** TLS required for ALPN `h2`.

---

## `pageload3` — HTTP/3 Multiplexed Page Load

Like `pageload2` but over QUIC. Included in the default build.

```bash
networker-tester --target https://127.0.0.1:8443/health \
  --modes pageload3 --page-assets 20 --page-asset-size 10k --runs 5 --insecure
```

**Populated:** same as `pageload`
**Note:** UDP must not be firewalled; `--insecure` needed for self-signed certs.

---

## `native` — System TLS Stack

Like `http1` but uses the platform's native TLS library (Secure Transport on macOS,
SChannel on Windows, OpenSSL on Linux) instead of rustls. Requires `--features native`.

```bash
networker-tester --target https://example.com/health --modes native --runs 5
```

**Populated:** same as `http1`; `tls.tls_backend = "native-tls"`

---

## `curl` — System curl Binary

Runs the system `curl` binary and parses its `--write-out` timing fields. Useful as a
ground-truth baseline.

```bash
networker-tester --target https://example.com/health --modes curl --runs 5
```

**Populated:** `http` fields from curl's timing output; `tls.tls_backend = "curl"`
