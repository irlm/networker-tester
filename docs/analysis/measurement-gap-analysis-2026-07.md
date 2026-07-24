# Measurement Gap Analysis — 2026-07-24

Deep audit of the Rust measurement engine (`crates/networker-tester` +
`crates/networker-endpoint`, SDK endpoints, orchestrator, and the C# persistence
layer): **what we measure today, what we're missing, and a 0-100 priority score
for each gap.**

Method: two independent code sweeps (probe engine + supporting surfaces),
reconciled. File:line evidence lives in the sweep notes; key anchors cited here.

---

## 1. What we measure today (inventory)

### Per-phase timings (all HTTP-family modes)
`dns_ms → tcp_ms → tls_ms → http_handshake_ms → ttfb_ms → total_duration_ms`
(`metrics.rs:1002-1042`), plus per-mode primary metrics (connect / resolve /
handshake / RTT / throughput / load).

### Beyond timings — already strong
| Area | What's captured | Where |
|---|---|---|
| **TCP kernel stats** | MSS, smoothed RTT + variance, retransmits (current + lifetime), cwnd, ssthresh, rcv_space, segs in/out, **congestion algorithm**, delivery rate, min-RTT | `socket_info.rs:33-60` (tcp probe) |
| **TLS** | version, cipher, ALPN, **full cert chain** (subject/issuer/expiry/SANs), resumption kind (full/full-hrr/resumed), TLS1.3 ticket count, backend (rustls vs OS) | `metrics.rs:948-1000` |
| **Server-Timing split** | proc/recv/total/auth + LagHound `app;dur` → network-vs-server split with anomaly flag, clock-skew estimate, **server context switches** | `metrics.rs:890-946` |
| **Client cost** | process CPU time, voluntary/involuntary context switches (Unix) | `metrics.rs:1027-1035` |
| **UDP** | RFC-3550-style jitter (arrival order), loss %, per-probe RTTs, p95 | `metrics.rs:1044-1057` |
| **Throughput** | throughput + end-to-end goodput, upload byte verification (`X-Networker-Received-Bytes`) | `throughput.rs:15-127` |
| **Page load** | per-asset timings, connections opened, TLS setup/overhead ratio, per-connection TLS, CPU | `metrics.rs:1087-1126` |
| **Browser** | load/DOMContentLoaded/TTFB, resource count/bytes, per-protocol resource mix | `metrics.rs:1129-1146` |
| **Packet capture** | real tshark pcapng + retransmissions, dup-ACKs, resets, transport shares, target confidence | `capture.rs:44-74` |
| **Environment** | client+server HostInfo (CPU/RAM/OS/hostname/region), pre-run RTT baseline + network classification (loopback/LAN/internet), benchmark env+stability checks | `metrics.rs:16-113` |
| **Statistics** | min/max/mean/p50/stddev; p95 gated ≥20 samples, p99 ≥100; 100%-loss UDP excluded from RTT | `metrics.rs:1505-1655` |

**Verdict:** the engine itself is genuinely deep. The two systemic problems are
(a) **we discard much of it after measurement**, and (b) a set of classic
network-diagnostic dimensions are absent entirely.

---

## 2. The gap list, scored

Score = weighted 0-100: **user value 40%** · **measurement-trust impact 20%** ·
**effort (inverse) 20%** · **product/scaffolding fit 20%**. Per project
convention, numeric 0-100 with deltas on re-score.

### P0 — score ≥ 85

| # | Gap | Score | Why / evidence |
|---|---|---|---|
| 1 | **Persist what we already measure.** `Networker.Contracts/ProbeRunResult.cs` maps only dns/tcp/tls/http phase timings; everything else the tester emits is **silently dropped** at the C# boundary: server_timing (network-vs-server split), cert chain, TCP kernel stats, CPU/CSW, goodput, baseline, packet-capture summary, udp_throughput detail, page_load, browser fields. The DB stores only the orchestration envelope. | **95** | Highest value-per-effort in the codebase: the measurement is already paid for; the product just can't show it. Unlocks: cert-expiry alerts, congestion-algorithm visibility, network-vs-server split in Run detail, CPU-per-protocol comparisons — with zero new probe code. |
| 2 | **Latency under load / bufferbloat (“responsiveness”, RPM-style).** No probe loads the link while measuring latency. We have both halves (UDP echo RTT + throughput saturation) but never run them together. | **88** | The single most user-felt network defect (video calls stutter while downloading). Apple-RPM-style “working latency” is an industry-recognized metric, brand-perfect for **LagHound**, and mostly composes existing probes into one new mode (`rpm`/`loaded`). |
| 3 | **QUIC/TLS 1.3 0-RTT measurement.** Endpoint advertises early data (`http3_server.rs:111`) but the tester never reports whether 0-RTT was used or its latency win. H3 is a headline feature; resumption depth exists for TCP-TLS but not QUIC. | **85** | Completes the protocol-comparison story we already sell (h1/h2/h3 head-to-head). Moderate effort in the quinn/h3 path. |

### P1 — score 70-84

| # | Gap | Score | Why |
|---|---|---|---|
| 4 | **ICMP ping + traceroute/path mode.** No hop count, per-hop latency/loss, or ICMP reachability at all. | **80** | The most-asked-for network diagnostic that “a network tester” is expected to have. Caveat: raw sockets need privileges (CAP_NET_RAW / UDP-probe traceroute fallback) — design for unprivileged mode first. |
| 5 | **TCP kernel stats for HTTP-family sockets.** `socket_info.rs` depth (cwnd, retrans, delivery-rate, min-RTT, CC algo) is captured **only by the bare `tcp` probe**, not the sockets under http1/2, throughput, pageload. | **76** | Turns every throughput anomaly into an explainable one (retrans? cwnd-limited? BBR-vs-cubic?). Plumbing, not research — the extractor exists. |
| 6 | **DNS depth.** Single A-then-AAAA resolve only: no CNAME-chain visibility, no per-record-type timing, no DoH/DoT comparison, no DNSSEC validation status, no per-server comparison. | **75** | DNS is a first-class probe today, so users expect diagnosis, not just one number. Hickory supports most of this. |
| 7 | **Certificate/OCSP depth.** No OCSP stapling status, revocation check, key size/type, signature algorithm, or CT-log presence. Chain+expiry already captured. | **73** | Natural extension of an existing strength; ops teams alert on these. Low-moderate effort in the rustls verifier path. |
| 8 | **Dual-stack IPv4-vs-IPv6 comparison (happy-eyeballs).** `ipv4_only`/`ipv6_only` flags exist but no mode probes both and compares. | **72** | One new composite mode reusing existing probes; growing operational relevance. |
| 9 | **Content-encoding measurement.** Compression (gzip/br/zstd) not measured: no compressed-vs-decompressed ratio, no negotiated-encoding report. | **70** | Cheap (headers + byte counts already captured); directly explains “why is my page slow”. |

### P2 — score 50-69

| # | Gap | Score | Why |
|---|---|---|---|
| 10 | **WebSocket probe mode** (handshake/upgrade time, message RTT, sustained echo jitter). | **66** | New protocol surface; valuable for app teams; moderate effort (tungstenite) + endpoint route. |
| 11 | **Source network context.** Interface type (WiFi/ethernet), link MTU, VPN detection, default gateway, local topology. | **64** | Explains variance between runs from the same tester; OS-specific plumbing. |
| 12 | **Geo/ISP/ASN enrichment** of source + target. | **60** | Fleet-level insight (“slow from ISP X”); needs an external DB/service — policy decision (offline MaxMind vs API). |
| 13 | **Path-MTU discovery probe** (DF-bit sweep / PMTUD). | **58** | Classic black-hole diagnosis; niche but decisive when it hits; raw-socket caveats like #4. |
| 14 | **Security-header audit** (HSTS, CSP, HTTP→HTTPS redirect behavior) as a report dimension. | **56** | Headers already captured — this is analysis + report UI, adjacent to core latency mission. |
| 15 | **System load during run** (load avg / CPU-steal on tester + endpoint, per-run not static). | **55** | Guards measurement trust on busy VMs; we already flag CPU time per probe. |
| 16 | **Clock-sync validation** (NTP query vs the heuristic `clock_skew_ms`). | **52** | Hardens the network-vs-server split; current estimate is fine for most uses. |
| 17 | **Real-time retransmission classification** (timeout vs fast-retransmit vs SACK, without pcap). | **50** | Mostly covered by #5 (`total_retrans` deltas); full classification needs eBPF/pcap — heavy. |

### P3 — score < 50 (recorded, not recommended now)

| Gap | Score |
|---|---|
| Browser depth: resource hints (preload/prefetch), service-worker detection, per-request waterfall priority | **45** |
| Per-probe memory profiling (allocations during request) | **38** |
| IPv6 NDP / extension-header analysis | **35** |
| DANE/TLSA validation | **30** |
| Cookie/3rd-party-cookie policy reporting | **28** |

---

## 3. Recommended execution order

1. **#1 persistence gap** — split into (a) widen `ProbeRunResult` + Run-detail UI
   for server_timing/cert/CPU fields, (b) TCP-stats + goodput, (c) baseline +
   capture summary. Ship incrementally; zero probe-side risk.
2. **#2 `rpm`/loaded-latency mode** — new Protocol variant (full checklist in
   CLAUDE.md: metrics.rs, dispatch, summary, modes.json + `requires:
   networker-endpoint`, drift guards, integration test).
3. **#3 0-RTT** then **#5 TCP stats plumbing** (both deepen existing modes, no
   new infra), then **#4 ping/path** (needs a privileges design first).

Everything scored here should exist in the product eventually down to ~60;
below that, revisit at next re-score.

---

*Sources: two-agent code sweep 2026-07-24 over crates/networker-tester (30
modes), crates/networker-endpoint, sdk/, benchmarks/orchestrator,
src/Networker.Contracts + Networker.Data, dashboard run views. Prod v0.28.74.*
