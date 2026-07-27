# Deep Measurement Audit — Module 1: Transport (TCP / UDP / QUIC-L4 / kernel & NIC counters)

**Date:** 2026-07-27 · **Scope:** L4 measurement + kernel/NIC-level counters in
`crates/networker-tester`. Successor to (and strictly deeper than)
`docs/analysis/measurement-gap-analysis-2026-07.md` items #5/#13/#15/#17.

Method: full read of `runner/socket_info.rs`, `runner/{udp,udp_throughput,http3,
rpm,ping,path,pmtud,dualstack}.rs`, the TCP path of `runner/http.rs`,
`metrics.rs` transport result structs, `capture.rs`, `network_context.rs`;
verified externally against the Linux `tcp_info` UAPI layout, Apple xnu
`netinet/tcp.h`, quinn 0.11 docs, Microsoft `SIO_TCP_INFO` docs, and
draft-ietf-ippm-responsiveness. One claim (§A.6 finding C-1) was additionally
verified **live on this machine** with a raw `getsockopt` probe.

Scoring convention (house): 0–100 = value 40% + trust-impact 20% +
effort-inverse 20% + product fit 20%. Scores are per-component
(`v/t/e/f` out of 40/20/20/20) so they can be re-scored with deltas.

---

## A. What we measure today (precise inventory)

### A.1 TCP — connect-time and post-transfer kernel stats

* **Connect timing**: `runner/http.rs` times the TCP connect and builds
  `TcpResult` (`metrics.rs:1580`) with local/remote addr, `connect_duration_ms`,
  and a **connect-time** `SocketInfo` sample (`http.rs:330-357`).
* **Post-transfer sampling**: `SocketProbe` (`runner/socket_info.rs:132-183`)
  `dup(2)`s the fd before hyper/TLS take ownership and re-samples
  `getsockopt(TCP_INFO)` after the transfer — the point where cwnd/retrans/
  delivery-rate describe the transfer. Result lands in
  `HttpResult::socket_stats` (`metrics.rs:1968-1973`) for http1/http2 and the
  TCP throughput modes, and per-connection in
  `PageLoadResult::per_connection_socket_stats` (`metrics.rs:2340`).
* **Fields read on Linux** (`socket_info.rs:190-277`, offset-gated on the
  `optlen` the kernel returns, buffer = 232 bytes): `tcpi_retransmits` (u8 @2),
  `tcpi_rtt` @68, `tcpi_rttvar` @72, `tcpi_snd_ssthresh` @76, `tcpi_snd_cwnd`
  @80, `tcpi_rcv_space` @96, `tcpi_total_retrans` @100, `tcpi_segs_out` @136,
  `tcpi_segs_in` @140, `tcpi_min_rtt` @148, `tcpi_delivery_rate` @160 — plus
  `TCP_MAXSEG` and `TCP_CONGESTION` (algorithm name). That is **11 of the ~60
  fields** a modern kernel fills (see §B.2), and the 232-byte buffer truncates
  everything ≥ offset 232 even on kernels that report it.
* **Windows**: `SocketProbe::new` returns `None` (`socket_info.rs:156-159`) —
  zero TCP kernel telemetry on Windows testers.
* **macOS**: see finding **C-1** in §A.6 — the `TCP_CONNECTION_INFO` read is
  broken and silently returns nothing beyond MSS.

### A.2 UDP echo / RPM

* `runner/udp.rs`: N echo datagrams (`[seq u32][timestamp_us i64]` + padding),
  per-probe RTT via userspace `Instant` deltas, seq-matched crediting of
  late/reordered/duplicate echoes (trust-audit V12, `udp.rs:110-235`).
  Aggregates: min/avg/p95, RFC 3550-style arrival-order jitter, loss %
  (`UdpResult`, `metrics.rs:1993`).
* `runner/rpm.rs`: Apple-RPM-*style* two-phase probe — unloaded UDP echo
  baseline, then paced echoes (default 100 ms cadence, 5 s window) while a
  **single** back-to-back HTTP `/download` loop saturates the link
  (`rpm.rs:120-214`). Reports loaded/unloaded RTT distributions,
  `rpm = 60000/loaded_avg`, `bufferbloat_factor`, plus load-generator evidence
  (bytes, completions, throughput) and fails loudly when the load never moved
  a byte (`rpm.rs:193-247`). Note: not the IETF methodology — see §B.9.

### A.3 UDP bulk throughput

`runner/udp_throughput.rs`: NWKT control protocol; download counts unique seqs
and times first-data→CMD_DONE; upload derives loss **only** from the server's
CMD_REPORT byte count and excludes the report round-trip from the window
(trust-audit V3/V4). `UdpThroughputResult` (`metrics.rs:2009`): datagrams
sent/received, bytes_acked, loss %, transfer window, MB/s.
**Not captured:** socket receive-buffer size or local drop counters — download
"loss" cannot currently be split into path loss vs. local `SO_RCVBUF` overflow
(§B.6), and nothing records inter-arrival jitter or a loss-burst profile for
the transfer.

### A.4 QUIC / HTTP-3

`runner/http3.rs` (quinn 0.11 + h3): QUIC handshake time, TTFB, body bytes,
CPU + context-switch cost, DNS phase, and a follow-up connection measuring TLS
1.3 session resumption + 0-RTT acceptance (`http3.rs:275-352`,
`TlsResult::quic_*`/`zero_rtt_*`, `metrics.rs:1832-1849`).
**Transport telemetry: none.** The `quinn::Connection` is moved into
`QuinnH3Connection` (`http3.rs:482`) and `Connection::stats()` is never called;
`HttpResult.socket_stats` is explicitly `None` for H3 ("TCP kernel stats do not
apply", `http3.rs:719-720`). So for QUIC we currently report **no** RTT
estimate, cwnd, lost packets, congestion events, datagram counts, or path MTU —
the exact fields we sample for TCP. TLS fields are also placeholders
(`cipher_suite: "QUIC-embedded"`, empty cert chain, `http3.rs:651-676`).

### A.5 Network-layer probes, capture, and source context (adjacent, in-scope edges)

* `runner/ping.rs`: unprivileged ICMP datagram sockets (Linux
  `ping_group_range`, macOS out-of-the-box, Windows `IcmpSendEcho`), reply TTL
  when observable, honest platform degradation (`ping.rs:1-24`).
* `runner/path.rs`: TTL-stepped UDP + Linux `IP_RECVERR` error queue for full
  unprivileged per-hop traces; macOS/Windows degrade to hop-count estimate with
  `hops: []`, never fabricated (`path.rs:1-22`).
* `runner/pmtud.rs`: DF/`IP_DONTFRAG` binary search with per-platform `method`
  provenance, ICMP next-hop MTU on Linux, `local_mtu` contrast
  (`PmtudResult`, `metrics.rs:2266`).
* `runner/dualstack.rs`: v4-vs-v6 legs + RFC 8305 happy-eyeballs verdict
  (`metrics.rs:2194`).
* `capture.rs`: optional tshark pcapng + summary — retransmissions, dup-ACKs,
  resets, transport shares, target-attribution confidence
  (`capture.rs:44-74`). No awareness of GRO/LRO offload state, which skews
  exactly these counters (§B.11).
* `network_context.rs`: default interface/kind/MTU/gateway/egress IP/VPN
  heuristic — static context, no interface *counters* (no drops/errors deltas).

### A.6 Correctness findings surfaced by this audit (fix before extending)

These are in-scope "what we measure today" facts, not gaps:

* **C-1 (P0, macOS TCP stats are dead code).** `socket_info.rs:355` defines
  `TCP_CONNECTION_INFO_OPT = 0x24`. In xnu, `TCP_CONNECTION_INFO` is **0x106**
  (Apple `netinet/tcp.h`; also Apple Developer docs for
  `tcp_connection_info`). Verified live on this Darwin 25.5 machine: a
  `getsockopt(IPPROTO_TCP, 0x24, …)` on a connected socket fails with
  `ENOPROTOOPT` ("Protocol not available"). Consequence: **every**
  `TCP_CONNECTION_INFO` field on macOS (RTT, rttvar, cwnd, ssthresh,
  retransmits) has always been silently `None`; only `TCP_MAXSEG` works. The
  module-header table ("macOS ✓ RTT, cwnd, retrans", `socket_info.rs:8`) and
  the `SocketProbe` test both overclaim — the test passes on MSS alone
  (`socket_info.rs:526-535`).
* **C-2 (P1, garbage-prone).** `TCP_CONGESTION_MACOS = 0x20`
  (`socket_info.rs:359`) is xnu's `TCP_CONNECTIONTIMEOUT`, not a
  congestion-algorithm option (xnu has no `TCP_CONGESTION` getsockopt at all).
  The code reads an `int` and interprets it as a UTF-8 string; a nonzero
  timeout would be reported as a 1-byte garbage "algorithm". Should return
  `None` on macOS, honestly.
* **C-3 (latent unit bugs, armed the moment C-1 is fixed).** In xnu's
  `tcp_connection_info`: `tcpi_srtt`/`tcpi_rttvar` are in **milliseconds**
  (Linux: µs) — `socket_info.rs:415-424` divides by 1000, which would
  under-report macOS RTT 1000×; `tcpi_snd_cwnd`/`tcpi_snd_ssthresh` are in
  **bytes** (Linux: segments) — mapping them into the same `snd_cwnd` field
  makes cross-platform comparisons wrong by ~MSS×. Also the struct in
  `socket_info.rs:334-352` puts `tcpi_txretransmitpackets` at offset 52; in
  xnu that offset is the TFO bitfield — the real `tcpi_txretransmitpackets` is
  a `u64` at offset 104, after the six 8-aligned `tx/rx` u64 counters.
* **C-4 (doc nits).** `tcpi_min_rtt`/`tcpi_notsent_bytes` were added in Linux
  4.6 (not 4.9 as the header table at `socket_info.rs:28` says; 4.9 added
  `tcpi_delivery_rate`). The `SocketStats.snd_cwnd` doc "segments" is
  Linux-only truth.

---

## B. What the professional standard measures that we don't

Reference points used: `ss -i` (iproute2) exposes essentially the full
`struct tcp_info` + `TCP_CC_INFO`; RFC 4898 (TCP Extended Statistics MIB) is
the design ancestor of the modern `tcp_info` fields; RFC 6349 (TCP throughput
testing) defines retransmission-ratio and buffer-delay metrics; RFC 9002
(QUIC loss/CC) defines what a QUIC stack knows; qlog/qvis is the QUIC
observability practice; draft-ietf-ippm-responsiveness is the RPM standard;
RFC 3168 + RFC 9330–9332 for ECN/L4S; RFC 8899 for DPLPMTUD.

### B.1 — QUIC transport stats from `quinn::Connection::stats()` — **Score 90** (v36/t17/e18/f19)

* **What:** quinn 0.11 exposes `ConnectionStats { udp_tx, udp_rx, frame_tx,
  frame_rx, path }`; `PathStats` carries `rtt`, `cwnd`, `congestion_events`,
  `lost_packets`, `lost_bytes`, `sent_packets`, `sent_plpmtud_probes`,
  `lost_plpmtud_probes`, `black_holes_detected`, `current_mtu` (docs.rs
  `quinn::PathStats`). `UdpStats` gives datagrams/bytes/syscall counts both
  directions; `FrameStats` gives per-frame-type counts (ACK, PING,
  RESET_STREAM…).
* **Why it matters:** we currently sell an h1/h2/h3 head-to-head and can
  explain a slow H1/H2 run down to cwnd and retransmissions, but an anomalous
  H3 run is a black box (`socket_stats: None`). `lost_packets` +
  `congestion_events` is the QUIC analogue of `total_retrans`;
  `current_mtu` is a per-connection DPLPMTUD (RFC 8899) verdict that
  cross-checks the `pmtud` probe; `rtt` is the QUIC analogue of `tcpi_rtt`.
  This is the single largest observability asymmetry in the engine, and it is
  userspace state we already own — no privileges, no platform variance.
* **How, in this codebase:** in `runner/http3.rs`, clone the
  `quinn::Connection` handle before `QuinnH3Connection::new(conn)`
  (`Connection` is cheaply clonable) and call `.stats()` after the body drain
  at `http3.rs:609-627`; add an additive `QuicStats` struct to `metrics.rs`
  (sibling of `SocketStats`) carried in `HttpResult` (or `TlsResult` for the
  resumption connection). Same pattern for pageload3
  (`runner/pageload.rs:1358+`). Windows/macOS/Linux identical.
* **Platform coverage:** all three, identical (userspace QUIC).

### B.2 — Full modern Linux `tcp_info` field set — **Score 88** (v34/t18/e18/f18)

* **What (already inside the 232-byte buffer we read, currently ignored):**
  * `tcpi_ca_state` @1 — Open/Disorder/CWR/Recovery/Loss at sample time.
  * `tcpi_options` @5 — bitflags: TIMESTAMPS, SACK, WSCALE, **ECN, ECN_SEEN**,
    **SYN_DATA (TCP Fast Open used)**. Three report-grade facts in one byte.
  * byte @7 — `tcpi_delivery_rate_app_limited` bit: whether the
    `delivery_rate` we already report was application-limited (without it the
    delivery-rate number is not trustworthy as a path-capacity signal).
  * `tcpi_rto` @8, `tcpi_snd_mss`/`tcpi_rcv_mss` @16/20,
    `tcpi_unacked/sacked/lost/retrans` @24–36 (in-flight loss picture),
    `tcpi_pmtu` @60 (kernel's path-MTU for this connection — free cross-check
    of the `pmtud` probe), `tcpi_rcv_ssthresh` @64, `tcpi_advmss` @84,
    `tcpi_reordering` @88, `tcpi_rcv_rtt` @92 (receiver-side RTT estimate).
  * `tcpi_pacing_rate`/`tcpi_max_pacing_rate` @104/112 (what `ss -i` shows as
    `pacing_rate`), `tcpi_bytes_acked`/`tcpi_bytes_received` @120/128
    (RFC 4898 `tcpEStatsAppHCThruOctetsAcked/Received`),
    `tcpi_notsent_bytes` @144, `tcpi_data_segs_in/out` @152/156.
  * `tcpi_busy_time` @168, `tcpi_rwnd_limited` @176, `tcpi_sndbuf_limited`
    @184 (Linux 4.10) — **the throughput-attribution triad**: µs the transfer
    was limited by receiver window vs. by our own send buffer vs. busy. This
    turns "throughput was 40 MB/s" into "…and 62% of the transfer was
    rwnd-limited ⇒ the bottleneck was the receiver, not the path". No other
    single change buys more explanatory power per byte.
  * `tcpi_delivered`/`tcpi_delivered_ce` @192/196 (4.18) — CE-marked delivery
    count = **real ECN/L4S signal** (RFC 3168 / RFC 9330).
  * `tcpi_bytes_sent`/`tcpi_bytes_retrans` @200/208, `tcpi_dsack_dups` @216,
    `tcpi_reord_seen` @220 (4.19) — enables RFC 6349's *Retransmitted Bytes
    Ratio* exactly, and `dsack_dups`+`reord_seen` distinguish **spurious**
    retransmission (reordering, RACK-TLP false alarms — RFC 8985 world) from
    genuine loss. `total_retrans` alone cannot make that distinction.
  * `tcpi_rcv_ooopack` @224, `tcpi_snd_wnd` @228 (5.4).
* **What needs a bigger buffer (grow 232 → 256):** `tcpi_rcv_wnd` @232 +
  `tcpi_rehash` @236 (6.1), `tcpi_total_rto`/`_recoveries`/`_time` @240+
  (6.7) — RTO-episode counts, the Windows `TimeoutEpisodes` analogue.
* **How:** pure extension of the existing `u32_at!/u64_at!` offset-gated
  pattern in `socket_info.rs:221-260`; widen `SocketInfo`/`SocketStats`
  additively (serde-defaulted, `schema_version` stays 1.0). Zero new syscalls.
* **Platform coverage:** Linux only (the fleet's prod testers) — macOS/Windows
  handled by B.5/B.10. Honest `None` elsewhere, as today.

### B.3 = C-1..C-3 — Fix and deepen macOS `TCP_CONNECTION_INFO` — **Score 74** (v24/t20/e14/f16)

* **What:** use `0x106`, correct struct layout (TFO bitfield @52, u64 counters
  @56+), correct units (srtt/rttvar **ms**, cwnd/ssthresh **bytes**), then read
  what Apple actually gives: `tcpi_txpackets/txbytes/txretransmitbytes/
  rxpackets/rxbytes/rxoutoforderbytes/txretransmitpackets` (u64s),
  `tcpi_flags` (LOSSRECOVERY, REORDERING_DETECTED), `tcpi_options`
  (TCPCI_OPT_ECN), `tcpi_rttcur` (last RTT vs. smoothed), `tcpi_snd_wnd`,
  `tcpi_rcv_wnd`, TFO bits. Report cwnd in a unit-honest way (either convert
  bytes→segments via MSS with a `cwnd_unit` note, or add `snd_cwnd_bytes`).
* **Why:** trust — today macOS silently reports nothing while docs claim
  otherwise, and the moment anyone "fixes" the constant without the unit fix
  we'd publish RTTs 1000× too small. `rxoutoforderbytes` and
  `txretransmitbytes` give macOS loss/reordering parity with Linux.
* **Trust-impact is scored max** because it corrects a wrongness class, but
  value is capped by fleet reality: prod testers are Linux/Windows VMs; macOS
  is mostly dev machines.
* **Cite:** apple-oss-distributions/xnu `bsd/netinet/tcp.h`
  (`TCP_CONNECTION_INFO 0x106`, field comments state ms/bytes units); Apple
  Developer docs `kernel/tcp_connection_info`.

### B.4 — `TCP_CC_INFO` (BBR internals) — **Score 62** (v22/t12/e16/f12)

* **What:** Linux `getsockopt(TCP_CC_INFO)` (option 26, kernel ≥ 4.1) returns
  `union tcp_cc_info`; when `TCP_CONGESTION == "bbr"`, `struct tcp_bbr_info`
  gives `bbr_bw` (estimated bottleneck bandwidth, the number BBR actually
  paces to), `bbr_min_rtt` (µs), `bbr_pacing_gain`, `bbr_cwnd_gain` — what
  `ss -i` renders as `bbr:(bw:…,mrtt:…)`.
* **Why:** we already report the CC algorithm name; when it is BBR,
  `bbr_bw` is a direct path-capacity estimate independent of our transfer
  size — a second opinion on `delivery_rate`. Low cost, but only meaningful on
  BBR-configured hosts, hence the modest value score.
* **How:** one more offset-gated getsockopt in `linux_socket_info`, fields
  populated only when the algorithm is bbr. Linux only.

### B.5 — System-wide kernel counter deltas around the run — **Score 68** (v25/t16/e13/f14)

* **What:** snapshot before/after each run and report deltas of:
  Linux `/proc/net/snmp` (`Tcp: RetransSegs, InErrs, InCsumErrors`,
  `Udp: InErrors, RcvbufErrors, SndbufErrors`) and `/proc/net/netstat`
  (`TcpExt: TCPOFOQueue, ListenDrops, TCPTimeouts, TCPLostRetransmit,
  PruneCalled, TCPRcvCollapsed, TCPBacklogDrop…`) — the `netstat -s`/`nstat`
  field set; macOS: parse `netstat -s` text (same idiom as
  `network_context.rs` uses for `route`/`ifconfig`); Windows:
  `GetTcpStatisticsEx`/`GetUdpStatisticsEx` (iphlpapi, unprivileged).
* **Why:** measurement *context*, in the same spirit as the existing HostInfo/
  CPU/CSW evidence: a run on a VM where `TcpExt.TCPTimeouts` jumped by 500
  system-wide (noisy neighbor, another tenant process) is not comparable to a
  clean run. `Udp.RcvbufErrors` deltas also back up B.6 when per-socket drop
  data is unavailable. Whole-machine counters are a confound detector, not a
  per-probe metric — report them under `HostInfo`-level context, clearly
  labeled machine-wide.
* **Effort:** file parse + delta; the only design work is where it hangs in
  the JSON contract (suggest `environment.kernel_counters_delta`).

### B.6 — Per-socket UDP drop visibility + buffer observations — **Score 80** (v28/t20/e15/f17)

* **What:** for `udp`, `udpdownload`, and the rpm loaded phase:
  1. Linux `SO_RXQ_OVFL` (socket(7), 2.6.33+): a cmsg on every `recvmsg`
     carrying the cumulative count of datagrams the kernel dropped because the
     socket buffer was full; and/or `getsockopt(SO_MEMINFO)` (socket(7),
     4.14+) whose `SK_MEMINFO_DROPS` slot gives the same counter plus the
     autotuned `sk_rcvbuf`/`sk_sndbuf`.
  2. Report the effective `SO_RCVBUF` (`getsockopt` after bind) in
     `UdpThroughputResult`, and optionally raise it (bounded by
     `net.core.rmem_max`) for download mode.
* **Why (trust, directly):** `udp_throughput.rs` computes download
  `loss_percent` from missing seqs (`udp_throughput.rs:114-121`) with a
  default ~208 KiB rcvbuf; at even a few hundred Mbit/s a scheduling hiccup
  overflows it, and the resulting **local** drops are indistinguishable from
  path loss in the report today. Splitting `loss_percent` into
  `lost_in_socket` vs `lost_on_path` (or at minimum flagging
  `socket_drops > 0`) is the difference between a demo number and a
  professional one — same honesty class as trust-audit V3/V4.
* **Platform:** Linux full (per-socket); macOS none per-socket (fall back to
  the B.5 `netstat -s` "dropped due to full socket-buffers" delta, labeled
  machine-wide); Windows none per-socket (B.5 `GetUdpStatisticsEx.dwInErrors`
  delta). `None` stays honest where unobservable.

### B.7 — Kernel receive timestamps for UDP/ICMP RTT and jitter — **Score 72** (v26/t17/e12/f17)

* **What:** timestamp echo arrival in the kernel instead of after the tokio
  wakeup: Linux `SO_TIMESTAMPNS`/`SO_TIMESTAMPING(SOF_TIMESTAMPING_RX_SOFTWARE)`
  cmsgs; macOS `SO_TIMESTAMP`; Windows `SIO_TIMESTAMPING` (Win10 2004+, UDP).
  TX software timestamps (send-side) exist on Linux only
  (`SOF_TIMESTAMPING_TX_SOFTWARE` via `MSG_ERRQUEUE`).
* **Why:** the RTT/jitter numbers in `udp.rs`/`rpm.rs` include scheduler and
  runtime wakeup latency (`sent_at.elapsed()` at `udp.rs:224`). Normally sub-ms
  noise — but the **rpm loaded phase runs the load generator in the same
  process** (`rpm.rs:127-164`), so the probe's own CPU load inflates the
  "loaded RTT" it is reporting: self-interference on exactly the headline
  metric. RX kernel timestamps remove the receive half of that error on all
  platforms (and both halves on Linux); the delta between kernel and userspace
  timestamps is itself a reportable "runtime-noise" evidence figure, matching
  the house style of proving measurement conditions (CPU/CSW already do this).
* **How:** `setsockopt` + `libc::recvmsg` with cmsg parsing behind a small
  helper; tokio interop via `AsyncFd` or `try_io`. Moderate, contained.

### B.8 — ECN / L4S path measurement — **Score 60** (v24/t10/e10/f16)

* **What:** three tiers.
  (1) *Passive TCP observation* — free with B.2: `tcpi_options` ECN/ECN_SEEN
  bits + `tcpi_delivered_ce` (negotiation is governed by the host's
  `net.ipv4.tcp_ecn`; Linux has no per-socket enable — report, don't control.
  macOS *does* have per-socket `TCP_ENABLE_ECN 0x104`).
  (2) *Active UDP marking check* — send echo probes with ECT(0)/ECT(1) set via
  `IP_TOS`/`IPV6_TCLASS`, read the received TOS via `IP_RECVTOS` cmsg, and have
  the endpoint's echo reflect the TOS byte it *received* inside the payload
  (small `networker-endpoint` change): detects ECN bleaching/remarking per
  direction — RFC 3168 §5, and ECT(1) treatment is the L4S classifier
  (RFC 9331).
  (3) *QUIC* — quinn negotiates/validates ECN internally but `PathStats` does
  not expose ECN counters in 0.11; report tier-2 results for the UDP path
  instead.
* **Why:** L4S (RFC 9330–9332) rollout is the industry's active
  latency-under-load story; "does this path bleach ECN?" is a differentiating
  diagnostic almost no general-purpose tester reports. Scored honestly
  moderate: user demand today is thin, tier 2 needs an endpoint protocol
  addition, and Windows lacks clean TOS-read APIs (tier 2 becomes
  Linux/macOS-first).

### B.9 — Align `rpm` with draft-ietf-ippm-responsiveness — **Score 76** (v32/t14/e12/f18)

* **What the standard does that we don't** (draft-ietf-ippm-responsiveness-08):
  * load via **up to 16 parallel** HTTP/2 (or H3) connections, ramped until a
    **working-conditions stability detector** says the bottleneck is saturated
    (goodput plateau + responsiveness stability), rather than one back-to-back
    download flow with no saturation proof;
  * probe latency **both** on separate connections **and on the load-bearing
    connections themselves** (the "foreign vs self" split — self-probes see
    the queue *inside* the loaded flow's buffers, including HOL and
    server-side queueing);
  * RPM computed from the aggregate of those HTTP-level round-trips, so
    numbers are comparable with Apple `networkQuality`, Cloudflare and Ookla
    implementations.
* **Why:** "RPM" is a branded, comparable industry number; ours shares the
  name but not the method (UDP echo under single-flow load), so a LagHound RPM
  and an Apple RPM for the same link will disagree and *should not be compared*
  — a trust liability once users notice. Keeping our UDP-echo variant (it
  isolates the network without server HTTP cost) **and** adding
  draft-compliant load/probing gives both comparability and diagnosis.
* **How:** the pieces exist — parallel `run_download_probe` tasks; saturation
  detector over per-second goodput (moving-average plateau per the draft);
  self-probes = timed small GETs multiplexed on the loaded H2 connections
  (hyper h2 handles are already in `throughput.rs`); sample B.2's
  `tcpi_notsent_bytes`/cwnd on load connections as saturation evidence.
* **Effort:** the algorithmic detector + new fields make this the most
  design-heavy item in the list; hence 12/20 effort despite full reuse.

### B.10 — Windows TCP telemetry via `SIO_TCP_INFO` — **Score 70** (v28/t14/e11/f17)

* **What:** `WSAIoctl(SIO_TCP_INFO)` (mstcpip.h, unprivileged) returns
  `TCP_INFO_v0/v1`: state, Mss, RttUs, MinRttUs, Cwnd (bytes), SndWnd, RcvWnd,
  RcvBuf, BytesOut/In, BytesReordered, **BytesRetrans**, **DupAcksIn**,
  **TimeoutEpisodes**, SynRetrans, and in v1 also Snd/RcvLimTime-style
  transmission-limit accounting (`SndLimTransRwin/Cwnd/Snd` + time/bytes) —
  i.e. the Windows equivalents of both our current field set *and* B.2's
  limited-time triad (Microsoft Learn: `TCP_INFO_v1`, `SIO_TCP_INFO`).
* **Why:** Windows testers currently ship **zero** TCP kernel telemetry
  (`SocketProbe` → `None`), yet the CI/product treats Windows as first-class.
  This closes the largest per-platform hole in `SocketStats`.
* **How/honesty:** connect-time sampling is easy (call on the raw
  `SOCKET` from `AsRawSocket` before hyper takes the stream). Post-transfer
  sampling is the hard part: there is no `dup(2)` equivalent with clean
  semantics for this pattern (`WSADuplicateSocketW` targets cross-process
  sharing), so phase 1 should ship connect-time-only stats on Windows with the
  existing `TcpResult` fields, documented as such — that is already far more
  than today. Effort scored accordingly.

### B.11 — NIC/interface counters + offload state as capture context — **Score 55** (v20/t13/e10/f12)

* **What:** per-run deltas of `/sys/class/net/<egress-if>/statistics/*`
  (`rx_dropped`, `rx_errors`, `rx_missed_errors`, `tx_dropped` — unprivileged,
  the interface is already identified by `network_context.rs`); offload state
  via `ethtool` ioctls (`ETHTOOL_GFEATURES`/`ETHTOOL_GRINGPARAM`, get-side
  unprivileged): GRO/GSO/TSO/**LRO** on/off and ring sizes; vendor `ethtool -S`
  stats (`ETHTOOL_GSTATS`) where the driver exposes them (virtio:
  `rx_queue_*_drops`).
* **Why:** two distinct payoffs. (1) NIC/ring drops explain variance on
  saturated links that no socket-level counter shows. (2) **Offload state is a
  correctness caveat for `capture.rs`**: with GRO/LRO active, tshark sees
  coalesced super-frames, so the retransmission/dup-ACK counts and packet
  totals in `PacketCaptureSummary` (`capture.rs:59-66`) are computed on a
  *distorted* view of the wire; professional practice is to record offload
  state alongside any capture-derived numbers (or capture with offloads noted
  as a warning). Add to `PacketCaptureSummary.warnings`.
* **Platform:** Linux full; macOS partial (`netstat -id`); Windows
  `GetIfEntry2`. Scored lowest of the accepted set: context, not measurement,
  and driver-dependent naming.

---

## C. Considered and REJECTED (with reasons)

| Item | Why rejected (for now) |
|---|---|
| **qlog emission for QUIC** (IETF qlog schema, qvis tooling) | quinn 0.11 has no qlog support (unlike quiche/quic-go); implementing it means forking quinn or swapping stacks. `ConnectionStats` (B.1) captures the summary-level value at ~2% of the cost. Revisit if upstream lands qlog. |
| **eBPF-based per-packet TCP tracing** (tcp tracepoints, BCC `tcpretrans`/`tcprtt`) | Requires root/CAP_BPF + kernel-version sensitivity across fleet VMs; duplicates what B.2's `bytes_retrans`/`dsack_dups` and the existing pcap path already answer at summary level. Violates the engine's unprivileged-first design (established by ping/path/pmtud). |
| **Raw-socket handcrafted TCP** (SYN-only RTT, custom TFO probing) | CAP_NET_RAW. The unprivileged connect() timing + `tcpi_options` SYN_DATA bit (B.2) covers the observable value. |
| **Hardware NIC timestamps / PTP** (`SOF_TIMESTAMPING_RX_HARDWARE`) | Needs NIC+driver PHC support; absent on the cloud/virtio VMs the fleet runs on. Software kernel timestamps (B.7) capture most of the accuracy win. |
| **`TCP_REPAIR` introspection** | CAP_NET_ADMIN; checkpoint/restore tool, not measurement. |
| **qdisc/fq pacing introspection via netlink** | Root for most qdisc stats; `tcpi_pacing_rate` (B.2) reports the socket-level truth without it. |
| **Windows `GetPerTcpConnectionEStats`** | Needs `SetPerTcpConnectionEStats` pre-enable and admin rights for several stat classes; `SIO_TCP_INFO` (B.10) is the sanctioned unprivileged replacement. |
| **OWAMP/TWAMP implementation** (RFC 4656 / RFC 5357) | One-way latency needs synchronized clocks we don't have (the existing `clock_skew_ms` is a heuristic, not NTP-grade); two-way value is already covered by udp/ping echo. TWAMP-light reflector compatibility is a product/interop feature, not a measurement gap — park until a customer asks for standards-interop. |
| **Multipath TCP observability** (`MPTCP_INFO`) | Not deployed on fleet paths or the endpoint; niche. Revisit if Apple-ecosystem targets matter (MPTCP is default for some Apple services). |
| **UDP GSO/GRO (`UDP_SEGMENT`) in the throughput sender** | A sender-efficiency optimization, not an observability gap; only worth it if profiling shows the udp sender CPU-bound before line rate. Tracked as perf, not measurement. |
| **ICMP timestamp / record-route options** | Filtered nearly everywhere on the public internet; would report `None` almost always — noise, not signal. |
| **conntrack/NAT-table introspection** | Root/netlink perms; NAT presence is already inferable cheaply (egress IP vs observed public IP) if ever needed — different module (context). |
| **Per-probe socket memory profiling via repeated `SO_MEMINFO` polling** | Sampling *during* transfer perturbs and complicates; single post-transfer snapshot (B.6) plus `sndbuf_limited` (B.2) answers the question. |

---

## D. Top-5 ranked shortlist

| # | Item | Score | One-line justification |
|---|---|---|---|
| 1 | **B.1 quinn `ConnectionStats` for H3/QUIC** | **90** | Biggest asymmetry: QUIC probes currently report zero transport facts; the data is free, cross-platform, and already in-process. |
| 2 | **B.2 full Linux `tcp_info`** (incl. limited-time triad, `bytes_retrans`, `delivered_ce`, options bits, app-limited flag; buffer → 256) | **88** | Same syscall we already make; converts every throughput number into an attributed one (rwnd vs sndbuf vs path) per RFC 4898/6349 practice. |
| 3 | **B.6 UDP socket-drop split + rcvbuf reporting** | **80** | Direct trust fix: today local rcvbuf overflow is reported as network loss in `udpdownload`/`udp`/`rpm`. |
| 4 | **B.9 IPPM-responsiveness alignment of `rpm`** | **76** | Makes the headline LagHound metric comparable with Apple/Cloudflare RPM and adds saturation proof; keep the UDP-echo variant as the diagnostic complement. |
| 5 | **B.3 macOS `TCP_CONNECTION_INFO` fix (C-1..C-3)** | **74** | Correctness before features: constant `0x24`→`0x106` (verified failing live), ms-vs-µs and bytes-vs-segments units, real retransmit/OOO counters. |

Fix findings **C-1/C-2** (and pre-emptively C-3) regardless of ranking — they
are wrongness, not backlog. B.10 (Windows `SIO_TCP_INFO`, 70) is the next item
after the five above and pairs naturally with B.2 as "SocketStats parity per
platform".

---

### Source references

* Linux `tcp_info` UAPI: `include/uapi/linux/tcp.h` (fields/offsets as listed;
  additions: 4.1 bytes_acked, 4.2 segs, 4.6 min_rtt/notsent, 4.9
  delivery_rate, 4.10 busy/rwnd/sndbuf_limited, 4.18 delivered_ce, 4.19
  bytes_retrans/dsack_dups/reord_seen, 5.4 rcv_ooopack/snd_wnd, 6.1
  rcv_wnd/rehash, 6.7 total_rto*). Rendered by `ss -i` (iproute2).
* RFC 4898 (TCP Extended Statistics MIB); RFC 6349 (TCP throughput testing:
  retransmitted-bytes ratio, buffer delay); RFC 2018 (SACK); RFC 8985
  (RACK-TLP, spurious-retransmit context); RFC 9293 (TCP).
* Apple xnu `bsd/netinet/tcp.h` (apple-oss-distributions):
  `TCP_CONNECTION_INFO 0x106`, `struct tcp_connection_info` field comments
  (srtt/rttvar in ms; cwnd/ssthresh/wnd in bytes; TFO bitfield; u64 tx/rx
  counters), `TCP_ENABLE_ECN 0x104`, `TCP_CONNECTIONTIMEOUT 0x20`; Apple
  Developer docs `kernel/tcp_connection_info`. Live `ENOPROTOOPT` check for
  option `0x24` performed on this machine (Darwin 25.5), 2026-07-27.
* quinn 0.11: docs.rs `quinn::ConnectionStats`, `quinn::PathStats`
  (rtt/cwnd/congestion_events/lost_packets/lost_bytes/sent_packets/
  plpmtud probes/black_holes_detected/current_mtu), `MtuDiscoveryConfig`
  (DPLPMTUD per RFC 8899); RFC 9000/9002 (QUIC transport, loss & CC).
* Windows: Microsoft Learn `SIO_TCP_INFO`, `TCP_INFO_v0`/`TCP_INFO_v1`
  (mstcpip.h); `GetTcpStatisticsEx`/`GetUdpStatisticsEx`; `SIO_TIMESTAMPING`.
* Responsiveness: draft-ietf-ippm-responsiveness-08 (RPM methodology: parallel
  load connections, working-conditions saturation, self/foreign probes);
  Apple `networkQuality`.
* ECN/L4S: RFC 3168; RFC 9330 (L4S architecture), RFC 9331 (ECT(1)/Prague),
  RFC 9332 (DualQ).
* Sockets/counters: socket(7) (`SO_RXQ_OVFL`, `SO_MEMINFO`,
  `SO_TIMESTAMPNS`/`SO_TIMESTAMPING`), proc(5)/`nstat(8)`
  (`/proc/net/snmp`, `/proc/net/netstat`), ethtool netlink/ioctl
  (`ETHTOOL_GFEATURES`, `ETHTOOL_GSTATS`, `ETHTOOL_GRINGPARAM`),
  `/sys/class/net/<if>/statistics`.
* Prior internal audit: `docs/analysis/measurement-gap-analysis-2026-07.md`.
