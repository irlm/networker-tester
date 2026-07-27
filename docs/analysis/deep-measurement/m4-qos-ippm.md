# M4 — QoS / IPPM Deep Audit: Latency, Jitter, Loss, Responsiveness, Path

**Date:** 2026-07-27 · **Audited:** v0.28.81 (`crates/networker-tester`) ·
**Scope:** `runner/{udp,rpm,ping,path,pmtud}.rs`, `baseline.rs`,
`metrics.rs` (RTT aggregation, `UdpResult`/`RpmResult`/`NetworkBaseline`,
benchmark env/stability checks), `clock_sync.rs`, `networker-endpoint/src/udp_echo.rs`.
**Judged against:** IETF IPPM standards (RFC 2330 family) and modern
responsiveness practice (draft-ietf-ippm-responsiveness / Apple RPM,
Waveform/DSLReports bufferbloat grading).
**Prior audit:** `docs/analysis/measurement-gap-analysis-2026-07.md` (gaps #2/#4/#13
shipped as `rpm`/`ping`+`path`/`pmtud` since; this audit goes a level deeper into
*methodological conformance* of what shipped).

Scoring convention (per project standard): 0–100 =
**value 40 + trust 20 + effort-inverse 20 + fit 20**.

---

## 1. Current state and conformance assessment

### 1.1 Round-trip delay (udp / ping / websocket echo trains)

**What we do.** `runner/udp.rs` sends N (default 10) seq-stamped datagrams
back-to-back to the endpoint's verbatim echo server (`udp_echo.rs`); the next
probe fires when the previous echo arrives or its window (default 5000 ms)
expires. Echoes are matched by embedded sequence id against an
outstanding-probe table, so late/reordered/duplicated echoes are credited to
the probe that sent them (trust-audit V12 — genuinely correct and better than
most hobby implementations). `ping.rs` replicates the same loop over
unprivileged ICMP datagram sockets (Linux ping-socket / macOS SOCK_DGRAM /
Windows `IcmpSendEcho`), `websocket.rs` over an open WS connection. All three
aggregate through `aggregate_udp_rtts` (`metrics.rs:2670`).

**Conformance vs RFC 2681 (Type-P-Round-trip-Delay).** Broadly consistent: a
wire-format-stamped singleton stream, timed from the sender's own monotonic
`Instant`, lost-vs-received explicitly separated. Deviations:

1. **Non-uniform loss/waiting threshold.** RFC 2681 §2.5 / RFC 6673 §4.1.3
   want a *fixed* waiting time `Tmax` that decides "lost". Ours is variable
   per probe: probe *k*'s echo can be credited during *any later* probe's
   receive window (`recv_outstanding_echoes` credits any outstanding seq), but
   after the **last** probe's window closes the run ends with **no grace
   drain** (`udp.rs:121-141` — unlike `rpm.rs:383-387`, which has one). So
   probe 0 effectively gets up to ~N×timeout to arrive while probe N−1 gets
   exactly one timeout. Loss for the same path condition depends on the
   probe's position in the train. Minor in practice at 5 s windows; wrong in
   principle, and it makes the per-probe timeline unusable for strict RFC 3357
   pattern analysis (see §2.4).
2. **Sampling process is neither periodic nor Poisson.** RFC 2330 §11.1
   recommends Poisson sampling to avoid synchronization bias; RFC 3432 defines
   the periodic alternative. Back-to-back send-on-echo-arrival is
   *latency-conditioned* sampling: a slow echo delays the next probe, so the
   sample times are autocorrelated with the very quantity being measured
   (slow periods get *fewer* samples per unit time → distribution biased
   toward good states). The rpm loaded phase (100 ms cadence) is properly
   periodic; the plain udp/ping/websocket trains are not.
3. **Default n=10 with a reported p95.** `aggregate_udp_rtts` computes p95 by
   nearest-rank on any n; with n=10 that is literally the max. The codebase
   *elsewhere* refuses exactly this (`MIN_SAMPLES_P95 = 20`, `metrics.rs:2723`,
   enforced in `Stats` and the CPU-window sampler) — the UDP/ping/ws/rpm path
   is exempt from the project's own honesty gate. Same inconsistency in
   `baseline.rs::measure_baseline`, which reports `rtt_p95_ms` from **5**
   TCP-connect samples.
4. **Two different percentile definitions coexist.** `aggregate_udp_rtts` uses
   nearest-rank (`ceil(0.95·n)−1`); `baseline.rs::percentile` uses linear
   interpolation. p95 of the same data differs between a udp probe and the
   env/stability check. Cosmetic but sloppy for a measurement product.

**ICMP-specific nits (`ping.rs`):** the reply parser extracts the ICMP
identifier and then discards it (`_ident`, `ping.rs:572`). On Linux the kernel
demuxes per-socket so this is safe; on macOS SOCK_DGRAM ICMP the ident is
preserved and other traffic can be delivered — matching on seq alone can, in
principle, credit another process's echo reply whose seq happens to collide.
One `if ident != our_ident { continue; }` closes it. Also `seq` is truncated
to u16 on the wire while `probe_count` is u32 — trains >65 535 would alias
(theoretical; not clamped).

**Verdict:** RTT mechanics are solid and honestly implemented; the sampling
schedule and small-n tail statistics are below IPPM practice.

### 1.2 Jitter (`aggregate_udp_rtts`, `baseline.rs::average_jitter_ms`)

**What we do.** `jitter = mean(|RTTᵢ₊₁ − RTTᵢ|)` over received samples in
vector order, computed before sorting (trust-audit V2 fixed the
sorted-telescoping bug). Doc-comments in `udp.rs`/`metrics.rs`/`PingResult`
call this "RFC 3550-style arrival-order jitter".

**Conformance — this label is wrong on all three counts:**

1. **Not RFC 3550.** RFC 3550 §6.4.1/A.8 interarrival jitter is an
   exponentially weighted moving average with gain 1/16:
   `J(i) = J(i−1) + (|D(i−1,i)| − J(i−1))/16`, where `D` is the difference of
   *relative one-way transit times* of consecutively *received* packets. We
   compute an unweighted arithmetic mean, over *round-trip* times, with no
   smoothing. A VoIP engineer comparing our `jitter_ms` to an RTCP-reported
   jitter is comparing different estimators over different quantities.
2. **Not arrival order.** `probe_rtts` is indexed by sequence number and
   `filter_map` preserves index order — the pairing is **send order, skipping
   losses**. A reordered late echo (which the matcher correctly credits) is
   paired by sequence, not arrival. For in-order traffic the two coincide;
   precisely when reordering happens — when the distinction matters — the
   comment is false.
3. **What it actually is:** the mean absolute IPDV of RFC 3393 with the
   selection function "consecutive-by-sequence among received packets"
   (RFC 3393 §2.6 / §4.2 explicitly blesses this selection) — over RTT rather
   than one-way delay. That is a *legitimate, standard* metric. It just needs
   to be called "mean |IPDV| (RFC 3393), RTT-based", not RFC 3550.

**What professionals additionally report** (RFC 5481): the *distribution* of
delay variation, in two standard forms — IPDV (consecutive-pair, what we
approximate) and **PDV** (delay minus the stream minimum — the "p99 minus
floor" number every SLA quotes). We already keep every per-probe RTT
(`probe_rtts_ms`), so PDV percentiles are pure arithmetic on data in hand.
A mean hides bimodality (a link that alternates 2 ms/40 ms and one with steady
21 ms jitter of ±2 ms can produce similar means; their PDV p95 differ 10×).

`baseline.rs::average_jitter_ms` (stability check) has the same estimator over
TCP-connect RTTs; same relabeling applies. The `max_jitter_ratio: 0.25` noise
gate (`BenchmarkNoiseThresholds`) is a reasonable engineering heuristic with
no standard counterpart — fine, as long as it isn't dressed as one.

### 1.3 Loss (udp/ping/websocket, env/stability checks)

**What we do.** `loss_percent = (sent − received)/sent × 100` over the train;
per-probe `Option<f64>` timeline persisted. Env/stability checks count failed
TCP connects as "packet_loss_percent" (it is actually *connection-establishment
failure rate* — a TCP connect can fail for SYN loss ×N retries, RST, or
listen-queue overflow; calling it packet loss overstates precision).

**Conformance vs RFC 6673 (round-trip loss) / RFC 7680 (one-way loss):**
directionally fine as a round-trip loss ratio; the non-uniform waiting
threshold from §1.1 applies. **What's missing entirely:**

- **Loss patterns (RFC 3357).** Loss distance and loss period — the
  burst-vs-random discrimination — is *the* diagnostic split: random 2% loss
  ≈ fine for TCP, bursty 2% (one 200 ms outage per train) destroys calls and
  gamers. We already store the exact per-probe timeline needed to compute
  both statistics; nothing computes them. Caveat: at n=10 the statistics are
  meaningless — this needs the longer periodic trains of §2.4.
- **Duplication (RFC 5560).** The matcher *detects* duplicate echoes and
  silently ignores them (`udp.rs:226`, `rpm.rs:425`, `ping.rs:587`).
  Duplication is a real pathology (misconfigured LAG/bridge loops, some LTE
  handovers) and we throw away the observation. One counter.
- **Reordering (RFC 4737).** Same story: the matcher was *built* to survive
  reordering, and then discards the fact that it happened. Recording arrival
  index alongside RTT yields Reordered-Ratio and Reordering-Extent
  (RFC 4737 §4/§5) for free. Reordering silently caps TCP throughput
  (dup-ACK fast retransmit) — it belongs next to loss in every report.
- **One-way loss direction.** With a verbatim echo server, forward and reverse
  loss are indistinguishable. STAMP's reflector sequence number (§2.2) splits
  them.

### 1.4 Responsiveness / latency-under-load (`rpm.rs`) vs draft-ietf-ippm-responsiveness

**What we do.** Phase 1: back-to-back UDP echo train → unloaded stats.
Phase 2: **one** sequential HTTP `/download` loop (32 MiB per transfer,
back-to-back on a single connection at a time) for a fixed 5 s window, with
UDP echo probes at 100 ms cadence; grace drain capped at
`min(udp timeout, 1000 ms)`. Reports `rpm = 60000 / loaded_avg_rtt_ms` and
`bufferbloat_factor = loaded_avg / unloaded_avg`. Load-failure detection
(refusing to report an idle link as "loaded") is a genuinely good honesty
guard most tools lack.

**Per-item conformance vs the draft** (draft-ietf-ippm-responsiveness-08;
parameters: ID=1 s intervals, INP/INC=1 connection ramp, MNP=16 max
connections, MAD=4-interval moving average, SDT=5% stability tolerance,
TMP=95% trimmed mean, MPS=100 probes/s, per-direction cap ~20 s):

| Spec element | Spec behavior | Ours | Conformance (0–100) |
|---|---|---|---|
| Working conditions: multiple load-generating connections | Start INP, add INC per interval up to MNP until goodput stabilizes | **Single connection**, sequential transfers, fixed count | **15** |
| Saturation detection | Declare saturation when stddev of last MAD goodput averages < SDT; only then trust the latency numbers | **None** — fixed 5 s window, saturated or not | **0** |
| Load duration | Ramp until stable, cap ~20 s/direction | Fixed 5 s | **30** |
| Upload direction | Separate upload working-conditions test | Download only | **0** |
| Foreign probes | HTTP GET on a **new** connection: measures TCP+TLS+HTTP handshake latency under load | UDP echo on a separate socket — network queueing only, no handshake components | **35** |
| Self probes | HTTP GET **multiplexed on the load-generating connection** (H2) — sees the loaded flow's own queue | **Absent** | **0** |
| Probe cadence | ≤ MPS/s, spread across intervals | 10/s periodic — fine | **85** |
| Aggregation | `Responsiveness = (60000/((TM(tcp_f)+TM(tls_f)+TM(http_f))/3) + 60000/TM(http_l)) / 2`, TM = 95% single-sided trimmed mean | `60000 / mean(loaded UDP RTT)` | **20** |
| Idle baseline reported alongside | Yes | Yes (unloaded phase) | **90** |

**Overall spec conformance: ~25/100.** Two of these are not pedantry but
first-order correctness problems:

- **The sparse-flow blind spot (worst one).** fq_codel and CAKE — the default
  qdiscs on most Linux routers, OpenWrt, and much CPE — give *sparse flows*
  (our 10/s UDP echo stream) queue-jump priority over the bulk TCP flow. On
  exactly the links that have flow-isolating AQM, our loaded UDP RTT ≈ idle
  RTT and `bufferbloat_factor ≈ 1.0` **while the load-bearing TCP connection
  itself sits behind hundreds of ms of queue**. This is precisely why the
  draft mandates *self probes on the load-generating connection*. Our current
  design systematically reports "no bufferbloat" on a large and growing class
  of real links. (Conversely on a dumb-FIFO bottleneck our numbers are fine.)
- **Loaded-tail censoring.** Loaded RTTs larger than the ≤1 s grace window are
  recorded as *lost*, and mid-window probes only get credited while the window
  is still open. Under severe bufferbloat (2–5 s queues are routinely observed
  on bad LTE/DOCSIS gear) the worst samples are exactly the ones dropped from
  the average → `loaded_rtt_avg` and `bufferbloat_factor` are biased **down**
  precisely when bufferbloat is worst; the evidence shows up only as inflated
  `loaded_loss_percent`, which the report doesn't connect to censoring.
- **Single-flow non-saturation.** One TCP connection cannot saturate high-BDP
  paths (window-limited) — under-load means under-measured queues. The
  32 MiB/transfer restart loop also re-enters slow-start every transfer,
  periodically *draining* the queue being measured.
- **The "RPM" number is not an RPM.** `60000/loaded_UDP_RTT` is 5–20×
  larger than a spec RPM for the same link (spec probes include TCP+TLS+HTTP
  handshake and HTTP-on-loaded-connection latency). Publishing it under
  Apple's trademark-adjacent name invites cross-tool comparison that will make
  every LagHound number look implausibly good — a trust liability, the exact
  thing the trust-audit series has been eliminating.

**Bufferbloat grading practice.** DSLReports established, and Waveform's test
popularized, grading on **absolute latency increase** (A+ <5 ms, A <30 ms,
B <60 ms, C <200 ms, D <400 ms, F ≥400 ms added). Our ratio
(`loaded/unloaded`) inverts perceived severity: 3 ms→9 ms (factor 3.0,
imperceptible) grades worse than 40 ms→110 ms (factor 2.75, ruins calls).
Report **added milliseconds** (loaded p95 − unloaded min is the robust choice)
as the headline, keep the ratio as secondary.

### 1.5 One-way delay (RFC 7679) and clock sync (`clock_sync.rs`)

We measure **no one-way anything** — every metric is round-trip against a
verbatim echo. `clock_sync.rs` is a clean, conformant one-shot SNTP (RFC 4330)
exchange: correct offset/delay algebra, kiss-of-death and mode validation,
bounded and best-effort. Limits: single sample against one pool server; offset
uncertainty is ±delay/2 (+ server dispersion) and is not reported — a 40 ms
RTT to pool.ntp.org means the offset is only good to ±20 ms, which the JSON
consumer cannot currently see. Path-asymmetry bias also means NTP offset is
the *wrong* instrument for measuring asymmetry itself (it assumes the symmetry
you're testing).

The professionally important observation: **one-way delay *variation* needs no
clock synchronization at all** — only short-term frequency stability
(RFC 3393 works per-direction on unsynchronized clocks). With reflector
timestamps (next section) we could say *which direction* the queue builds in
during the rpm loaded phase — a diagnostic neither Waveform nor Ookla gives —
without solving clock sync. Absolute OWD (RFC 7679) can then be layered on as
"OWD ± uncertainty" using the SNTP offset with its ±delay/2 bound stated.

### 1.6 The echo protocol vs TWAMP-Light / STAMP — we own both ends

`udp_echo.rs` echoes bytes verbatim: no receive timestamp, no transmit
timestamp, no reflector sequence number, no TTL/DSCP reflection. This is the
single structural ceiling on the whole QoS module: every deviation in
§1.2–1.5 that says "indistinguishable" or "round-trip only" traces to it.

TWAMP-Light (RFC 5357 Appendix I) and its standardized successor **STAMP
(RFC 8762)** define exactly the missing reflector: unauthenticated-mode UDP
packets carrying sender timestamp + sequence, reflected with **reflector
receive timestamp (T2), reflector transmit timestamp (T3), reflector sequence
number, and received TTL**; RFC 8972 adds TLVs (notably Class-of-Service,
which reflects the DSCP/ECN byte as actually received). That one packet format
upgrade yields, with arithmetic we already have:

- RTT with reflector processing time removed (T3−T2 subtracted) — cleaner
  numbers on a busy endpoint;
- per-direction delay *variation* and queue-direction attribution (no sync
  needed, §1.5);
- **directional loss**: sender-seq gaps seen by the reflector = forward loss;
  reflector-seq gaps seen by us = reverse loss (RFC 8762 §4.2);
- reflected TTL → reverse-path length change detection;
- with RFC 8972 CoS TLV: DSCP bleaching and **ECN mangling/CE-mark
  observation** on the forward path (§2.7).

STAMP unauthenticated packets are ≥44 bytes with a fixed layout — a weekend
of Rust on each side, and interoperable with every carrier-grade tester
(Juniper/Nokia/Cisco all speak TWAMP-Light/STAMP), which is a credibility
asset for a measurement product.

### 1.7 Path measurement (`path.rs`) vs Paris-traceroute methodology

**What we do.** Linux: UDP probes, TTL 1..=max, **destination port =
33434+ttl−1**, ICMP errors read via `IP_RECVERR`/`MSG_ERRQUEUE` — full per-hop
trace, unprivileged, with honest `*` hops and an all-silent trace reported as
no-information. macOS/Windows: honest degradation to hop-count estimate, hops
never fabricated. The honesty engineering is exemplary.

**The classic artifact, present:** varying the destination port per TTL means
**every hop is probed on a different flow**. Per-flow ECMP load balancers
(ubiquitous in transit and any cloud fabric) hash the 5-tuple; consecutive
TTLs therefore ride different physical paths, producing the false links,
phantom loops, and inconsistent RTT-vs-hop curves that Paris traceroute
(Augustin et al., IMC 2006) was invented to eliminate. The fix is unusually
cheap *for us specifically*: classic traceroute needed the per-TTL port to
demultiplex responses, but we probe **sequentially** and match ICMP errors
temporally on one socket — the varying port buys us nothing. Holding
src/dst port constant (one `connect()`, drop the `wrapping_add`) makes the
probe flow-stable, Paris-consistent, and *simpler*. Remaining honest caveat to
document: per-**packet** balancers can still scramble anything.

**Other gaps vs professional practice:** one probe per hop (standard is ≥3 →
per-hop loss% and min/avg RTT; a single silent probe renders `*` for a hop
that merely rate-limits ICMP); no per-hop RTT statistics; sequential-only
(30 hops × 1 s timeout = 30 s worst case; batched TTL windows cut this ~10×);
no MDA-style multipath enumeration (Veitch et al.; route metrics formalized in
RFC 9198) — deliberately acceptable to skip (§3).

### 1.8 PMTUD (`pmtud.rs`) vs RFC 4821 / RFC 8899

Genuinely good. The design — DF probes, **positive delivery confirmation**
(echo or port-unreachable) as the primary signal, ICMP frag-needed only
tightening the bound, black-hole = honest `None` with the reason — is
precisely the *packetization-layer* philosophy of PLPMTUD (RFC 4821) and
DPLPMTUD (RFC 8899): never depend on ICMP delivery for correctness, treat it
as an accelerator. Stale-error draining before each send, local-EMSGSIZE vs
wire-ICMP evidence classes, and `lower_bound_only` are above the bar of most
commercial tools. Deviations worth noting, none severe: binary search vs
RFC 8899 §5.3's recommended candidate table (probe common MTUs 1500/1492/
1460/1400/1280 first — fewer probes on the 99% case, and the result lands on
recognizable values); no periodic re-validation (RFC 8899 PMTU_RAISE_TIMER) —
irrelevant for a one-shot probe; the ICMP-quoted next-hop MTU is trusted as a
search bound without validation against the floor (RFC 8899 §4.6.1 requires
sanity-checking quoted MTUs ≥ minimum; a malicious/buggy ICMP with
`ee_info=68` would pin `hi` below the v4 floor — one `max(floor)` clamp).

### 1.9 Baseline & benchmark environment checks (`baseline.rs`)

TCP-connect RTT ×5 (baseline) / ×12@50 ms (stability) with network-type
classification and CPU-contention bracketing — a sound *noise gate* for
benchmark publication, and honest about the CPU window (`MIN_CPU_WINDOW_MS`).
Non-conformances already covered: p95-from-5-samples (§1.1.3), connect-failure
labeled packet loss (§1.3), jitter label (§1.2). One more: each sample opens a
new ephemeral-port connection → each RTT sample may ride a different ECMP path;
fine for a noise gate, but it means baseline "jitter" conflates path diversity
with queueing — worth one sentence in docs.

---

## 2. Professional gaps — what / why / implementation path / score

### 2.1 Responsiveness-spec conformance for `rpm` (working conditions + self-probes)

- **What:** (a) multi-connection load: start 1, add 1/interval (1 s) up to 8–16,
  Reuse `run_download_probe`'s machinery with ranged/parallel requests;
  (b) goodput-stability saturation detection (stddev of last 4 interval
  averages < 5% → saturated) with `saturated: bool` in `RpmResult` — refuse to
  headline bufferbloat numbers from an unsaturated run (same honesty class as
  the existing `load_ok` guard); (c) **HTTP self-probes**: small GET
  multiplexed on a load connection (reqwest/hyper H2 stream or a second
  request on the H1 connection pool) — kills the fq_codel blind spot (§1.4);
  (d) HTTP foreign probes (new-connection GET, we already time
  dns/tcp/tls/ttfb per attempt — it's `run_probe` with a 1-byte object);
  (e) upload-direction phase; (f) spec aggregation (95% trimmed means,
  foreign/self averaging) reported as `rpm_spec`, keeping today's UDP-derived
  number renamed `udp_loaded_rtt` (never "RPM").
- **Why:** §1.4 — current mode under-reports bufferbloat on AQM links
  (wrong answer, not just nonstandard), and the RPM number is incomparable
  with the ecosystem while carrying its name.
- **Path here:** all building blocks exist (`throughput.rs`, `run_probe`
  phase timings, paced echo engine). New fields are additive to `RpmResult`;
  modes.json untouched (same `rpm` mode). Effort: the largest item in this
  report (~1–2 weeks incl. tests), sliceable — (c)+(b) alone remove the two
  worst distortions.
- **Score: 88** (value 36/40 — headline product metric for "LagHound";
  trust 19/20 — fixes a wrong-answer class; effort 14/20; fit 19/20).

### 2.2 STAMP (RFC 8762) sender + reflector — we own both endpoints

- **What:** unauthenticated-mode STAMP reflector in `networker-endpoint`
  (new UDP port beside :9999; stateful mode for reflector seq), sender mode in
  the tester (`stamp` mode or an upgrade path inside `udp`: detect a STAMP
  reflector, fall back to verbatim echo). Yields: processing-time-corrected
  RTT, **directional loss**, per-direction delay variation
  (queue-direction attribution in the rpm loaded phase), reflected TTL;
  RFC 8972 CoS TLV later for DSCP/ECN reflection (§2.7).
- **Why:** §1.6 — one packet format removes the structural ceiling on five
  metrics at once, and speaks the same protocol as carrier test gear
  (interop = credibility for a measurement vendor).
- **Path here:** fixed 44+-byte layouts, both sides in-repo, integration test
  = in-process reflector (the existing test pattern). NTP-format timestamps —
  reuse `clock_sync.rs` conversion helpers. Wire it into rpm's loaded phase
  for direction-resolved bufferbloat. Follow the new-protocol checklist
  (CLAUDE.md) if shipped as a new mode.
- **Score: 84** (value 32/40; trust 18/20 — direction attribution +
  processing-time removal; effort 16/20 — small fixed formats, we control
  both ends; fit 18/20).

### 2.3 Loaded-tail censoring fix + absolute bufferbloat grading

- **What:** (a) extend the rpm grace drain to cover the worst plausible queue
  (e.g. `max(4 s, 4×unloaded_avg)`) *or* track outstanding probes past the
  window and report `censored_count` explicitly; (b) headline
  `added_latency_ms = loaded_p95 − unloaded_min` next to the factor;
  (c) optional letter grade on the DSLReports/Waveform scale (A+ <5 ms …
  F ≥400 ms) for instant recognizability.
- **Why:** §1.4 — today's numbers are biased *optimistic* precisely on the
  worst links (a trust-audit-class defect: the reported number is wrong, not
  merely incomplete), and the ratio metric inverts perceived severity.
- **Path here:** ~30 lines in `rpm.rs` + fields in `RpmResult` + summary/report
  strings. Days, not weeks.
- **Score: 82** (value 28/40; trust 20/20; effort 19/20; fit 15/20).

### 2.4 Loss-pattern / burst analysis (RFC 3357) + long periodic trains

- **What:** a `--udp-probes`-scaled periodic train option (e.g. 100–600 probes
  at 20–50 ms fixed cadence per RFC 3432, decoupling send cadence from echo
  arrival — the paced engine in `rpm.rs::echo_rtts` already does this);
  compute loss distance & loss period (RFC 3357), report
  `burst_loss: {periods, max_period_len, loss_distance_min}` and a
  bursty-vs-random verdict. Fix §1.1.1's non-uniform threshold as a
  by-product (fixed per-probe Tmax, credit via the outstanding-table as now,
  final drain of Tmax).
- **Why:** §1.3 — burst-vs-random is the single most actionable loss
  diagnostic and we already persist the exact timeline; only train length and
  arithmetic are missing.
- **Path here:** analysis is pure post-processing in `metrics.rs` (+ tests
  with synthetic timelines); the paced sender is code motion from rpm to udp.
- **Score: 76** (value 28/40; trust 15/20; effort 17/20; fit 16/20).

### 2.5 Reordering (RFC 4737) + duplication (RFC 5560) counters

- **What:** record arrival order in the echo matchers (one `arrival_idx`
  alongside the RTT credit; one `duplicates += 1` where duplicates are
  currently ignored), then report Reordered-Ratio, Reordering-Extent
  (RFC 4737 §4.2/§5.4) and duplicate fraction (RFC 5560) in
  `UdpResult`/`PingResult`/`WebSocketResult`.
- **Why:** §1.3 — the matcher already *survives* both pathologies and then
  discards the evidence; reordering silently caps TCP throughput and nothing
  else in the product can currently explain that failure mode.
- **Path here:** ~50 lines across three matchers + additive fields + report
  rows. The cheapest real metric in this report.
- **Score: 74** (value 24/40; trust 16/20; effort 19/20; fit 15/20).

### 2.6 Delay-variation reporting done right (RFC 3393/5481; correct labels)

- **What:** (a) relabel current `jitter_ms` as mean |IPDV| (RFC 3393) —
  doc/comment change plus report strings, JSON field name kept; (b) add PDV
  percentiles (RFC 5481 §4.2: delay − min, report p95/p99 when n ≥ the
  existing `MIN_SAMPLES_P95/P99` gates — finally applying the project's own
  gate to the echo-train path); (c) optionally an actual RFC 3550 EWMA-1/16
  value labeled as such for VoIP-tool comparability; (d) use one percentile
  implementation everywhere.
- **Why:** §1.2 — a measurement product mislabeling its estimator is a trust
  problem out of proportion to the code involved; PDV percentiles are what
  SLAs and VoIP planners actually consume.
- **Path here:** `metrics.rs` only + tests; no wire changes (additive fields).
- **Score: 72** (value 20/40; trust 19/20; effort 19/20; fit 14/20).

### 2.7 ECN echo / readiness observation

- **What:** set ECT(0) on probe datagrams (`IP_TOS`/`IPV6_TCLASS`); with the
  STAMP CoS TLV (RFC 8972) reflecting the received DSCP/ECN byte, report:
  ECT survived / bleached / CE-marked on the forward path. CE marks during the
  rpm loaded phase = AQM present and reacting (explains *why* added latency is
  low); bleaching = middlebox interference. Later: L4S readiness (ECT(1),
  RFC 9331) as the ecosystem moves.
- **Why:** ECN behavior is the modern differentiator between "no bufferbloat
  because AQM" and "no bufferbloat because idle"; almost no consumer tool
  reports it.
- **Path here:** depends on §2.2 (needs reflection of the received byte);
  setsockopt + one TLV parse on top of it.
- **Score: 64** (value 22/40; trust 10/20; effort 16/20; fit 16/20).

### 2.8 Paris-consistent `path` mode (+ multi-probe hops)

- **What:** constant 5-tuple (drop the per-TTL port increment — see §1.7: our
  sequential temporal matching never needed it), 3 probes per TTL with
  per-hop loss% and min/avg RTT, batched TTL windows for speed, and a
  documented per-packet-LB caveat.
- **Why:** §1.7 — the current port-varying probing is the textbook ECMP
  artifact generator; under load balancing our hop lists can contain
  interleaved routers from different paths presented as one path.
- **Path here:** `path.rs` Linux impl: delete the port arithmetic, loop
  probes per TTL, extend `PathHop` additively (`loss_percent`, `rtt_min_ms`);
  portable impl unchanged.
- **Score: 68** (value 22/40; trust 16/20; effort 16/20; fit 14/20).

### 2.9 One-way delay with stated uncertainty (RFC 7679, coarse)

- **What:** on top of §2.2 timestamps + the existing SNTP offset: report
  `owd_forward_ms`/`owd_reverse_ms` **with** `±uncertainty_ms = ntp_delay/2`
  propagated, and refuse the split when uncertainty > asymmetry observed
  (honesty gate). Also surface the SNTP uncertainty in `ClockSync` regardless.
- **Why:** absolute OWD is what distinguishes "40 ms out, 5 ms back" from
  symmetric 45/2; with ±20 ms pool-NTP uncertainty it's only meaningful for
  coarse asymmetry — say so in the schema rather than not shipping it.
- **Path here:** arithmetic + additive fields once §2.2 lands; multi-sample
  SNTP (best-of-4, min-delay filter per RFC 5905 practice) halves the
  uncertainty for one round of effort.
- **Score: 58** (value 18/40; trust 14/20; effort 14/20; fit 12/20).

### 2.10 Small honesty patches (bundle)

Fixes falling out of §1 that don't merit separate scores, bundled: apply
`MIN_SAMPLES_P95` gating to `aggregate_udp_rtts` and baseline p95 (make the
field `Option`, additive); rename env-check `packet_loss_percent` →
document as connect-failure rate; ICMP ident check on macOS; clamp
ICMP-quoted MTU to the address-family floor in `pmtud.rs`
(RFC 8899 §4.6.1); add the missing final grace drain in `udp.rs`.
- **Score: 70** (value 14/40; trust 20/20; effort 20/20; fit 16/20 — an
  afternoon of diffs, all in the "reported numbers must be true" class).

---

## 3. Considered and REJECTED

| Candidate | Why rejected |
|---|---|
| **Full TWAMP (RFC 5357 control protocol)** | TWAMP-Control (TCP negotiation, session management, modes) exists for multi-vendor session brokering. We own both endpoints and already have a control plane; STAMP unauthenticated mode gives the entire measurement payload with none of the protocol machinery. Revisit only if third-party TWAMP responders become a target market. |
| **Poisson sampling (RFC 2330 §11.1)** | Periodic streams (RFC 3432) are the right choice for this product: they match the constant-rate traffic (VoIP/gaming) whose experience we predict, they're what the responsiveness draft uses, and they make loss-pattern metrics interpretable. The anti-synchronization argument matters for continuous monitors, not short active trains. Decision should be *documented*, not implemented. |
| **MDA multipath enumeration (Paris-MDA, RFC 9198 context)** | Statistically rigorous ECMP enumeration costs O(100s) probes per hop, needs per-flow probe steering that fights our errqueue-serialized unprivileged design, and produces a topology product, not a QoS number. Flow-*consistency* (§2.8) removes the measurement artifact; enumeration adds mostly cartography. |
| **Raw-socket ICMP traceroute / privileged modes** | Project constraint (unprivileged-first) is correct and already produced honest degradation tiers; a privileged mode forks the test matrix for marginal macOS/Windows hop data. |
| **Kernel/NIC timestamping (`SO_TIMESTAMPING`, PTP)** | Sub-microsecond timestamping is for lab gear; our quantities of interest (queueing at ms scale) are far above userspace-`Instant` noise (~10s of µs). Enormous platform-specific surface for zero decision-changing precision. |
| **True RFC 3550 EWMA as the *headline* jitter** | The 1/16 EWMA is an RTP-receiver state variable — order-dependent, warm-up-sensitive, and unstable on 10-sample trains. Right as a *secondary* comparability number (§2.6c), wrong as the primary statistic; mean |IPDV| + PDV percentiles dominate it for reporting. |
| **ICMP timestamp messages (type 13/14) for OWD** | Widely filtered, 1 ms resolution, frequently lies (middleboxes) — strictly worse than the STAMP path we can own. |
| **Continuous background monitoring mode (smokeping-style)** | Product-scope, not methodology; the run-oriented architecture (control-plane-scheduled runs) already covers recurring measurement. |
| **IPDV/PDV of the *unloaded vs loaded* difference distributions (fancy two-sample stats, K-S tests)** | Overkill for the report audience; added_latency_ms + PDV percentiles carry the decision weight without a statistics lecture in the UI. |

---

## 4. Top-5 shortlist

| # | Item | Score | One-line rationale |
|---|---|---|---|
| 1 | **§2.1 Responsiveness-spec conformance** (multi-connection ramp, saturation gate, HTTP self-probes, upload; rename UDP-derived "RPM") | **88** | The headline metric is currently blind on AQM links and incomparable under the name it carries. |
| 2 | **§2.2 STAMP sender + reflector** (RFC 8762; we own both ends) | **84** | One packet format unlocks directional loss, per-direction jitter, processing-corrected RTT, and later ECN — the structural fix behind five gaps. |
| 3 | **§2.3 Loaded-tail censoring fix + added-latency-ms grading** | **82** | Cheapest wrong-answer fix in the module: worst links currently report *optimistic* bufferbloat. |
| 4 | **§2.4 Loss patterns + long periodic trains** (RFC 3357/3432) | **76** | Burst-vs-random is the diagnostic split; the per-probe timeline is already persisted. |
| 5 | **§2.5 Reordering + duplication counters** (RFC 4737/5560) | **74** | The matchers already detect both and throw the evidence away; ~50 lines. |

(§2.10's honesty bundle scores 70 but is an afternoon — do it opportunistically
alongside #3.)

---

## 5. Sources

**IETF / IPPM:**
[RFC 2330](https://www.rfc-editor.org/rfc/rfc2330) (IPPM framework, sampling §11.1) ·
[RFC 3432](https://www.rfc-editor.org/rfc/rfc3432) (periodic streams) ·
[RFC 2681](https://www.rfc-editor.org/rfc/rfc2681) (round-trip delay) ·
[RFC 7679](https://www.rfc-editor.org/rfc/rfc7679) (one-way delay) ·
[RFC 7680](https://www.rfc-editor.org/rfc/rfc7680) (one-way loss) ·
[RFC 6673](https://www.rfc-editor.org/rfc/rfc6673) (round-trip loss) ·
[RFC 3393](https://www.rfc-editor.org/rfc/rfc3393) (IPDV) ·
[RFC 5481](https://www.rfc-editor.org/rfc/rfc5481) (delay-variation applicability: IPDV vs PDV) ·
[RFC 3550 §6.4.1/A.8](https://www.rfc-editor.org/rfc/rfc3550) (RTP interarrival jitter, EWMA-1/16) ·
[RFC 3357](https://www.rfc-editor.org/rfc/rfc3357) (loss distance/period) ·
[RFC 4737](https://www.rfc-editor.org/rfc/rfc4737) (reordering metrics) ·
[RFC 5560](https://www.rfc-editor.org/rfc/rfc5560) (duplication metric) ·
[RFC 5357](https://www.rfc-editor.org/rfc/rfc5357.html) (TWAMP; Appendix I TWAMP-Light) ·
[RFC 8762](https://www.rfc-editor.org/rfc/rfc8762) (STAMP) ·
[RFC 8972](https://www.rfc-editor.org/rfc/rfc8972) (STAMP optional extensions / CoS TLV) ·
[RFC 4330](https://www.rfc-editor.org/rfc/rfc4330) (SNTP) ·
[RFC 4821](https://www.rfc-editor.org/rfc/rfc4821) / [RFC 8899](https://www.rfc-editor.org/rfc/rfc8899) (PLPMTUD / DPLPMTUD) ·
[RFC 9198](https://www.rfc-editor.org/rfc/rfc9198) (route assessment) ·
[RFC 3168](https://www.rfc-editor.org/rfc/rfc3168) / [RFC 9331](https://www.rfc-editor.org/rfc/rfc9331) (ECN / L4S).

**Responsiveness & bufferbloat practice:**
[draft-ietf-ippm-responsiveness-08](https://datatracker.ietf.org/doc/html/draft-ietf-ippm-responsiveness) and the
[working-group source](https://github.com/network-quality/draft-ietf-ippm-responsiveness/blob/main/draft-ietf-ippm-responsiveness.md)
(parameters ID/MAD/SDT/TMP/MNP/INP/INC/MPS, foreign/self probes, trimmed-mean
aggregation formula) ·
Waveform/DSLReports bufferbloat grading scale
([bufferbloat.net test list](https://www.bufferbloat.net/projects/bloat/wiki/Tests_for_Bufferbloat/),
[grading rubric reference](https://github.com/kenarkerim/bufferbloat-test-tools),
[2026 test comparison](https://www.mysupportdetails.com/web/dslreports-shutdown-alternative-bufferbloat-test-2026/)).

**Path measurement:** Augustin et al., *Avoiding traceroute anomalies with
Paris traceroute*, IMC 2006 (flow-consistent probing; MDA follow-up work).

*Code anchors cited inline (file:line) refer to v0.28.81.*
