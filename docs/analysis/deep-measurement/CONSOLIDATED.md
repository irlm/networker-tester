# Deep Measurement Analysis — Consolidated Report (2026-07-27)

Five domain-specialist audits of the measurement engine against **external
professional practice** (IETF IPPM RFCs, RIPE-Atlas/SSLLabs/WebPageTest-class
tooling, kernel observability, benchmarking statistics literature), with live
research and library-source verification. Module reports (scored 0-100, house
convention, each with an explicit considered-and-rejected section):

| Module | File | Lens |
|---|---|---|
| M1 | `m1-transport.md` | TCP/UDP/QUIC + kernel/NIC counters |
| M2 | `m2-dns-tls.md` | DNS + TLS/PKI (RIPE-Atlas / SSLLabs class) |
| M3 | `m3-http-web.md` | HTTP + page-load + browser (WebPageTest class) |
| M4 | `m4-qos-ippm.md` | Latency/jitter/loss/responsiveness/path vs IETF IPPM |
| M5 | `m5-statistics-validity.md` | Statistics soundness + validity envelope |

## Executive verdict

The 2026-07-24 audit was **code-complete but not standards-complete**. This
pass found what it structurally could not: **correctness and conformance
defects in numbers we already ship**, plus a class of high-value capabilities
that are uniquely cheap for us because we control both ends of the wire.
Totals: **~57 scored gaps**, **42 explicitly-rejected items** (with reasons),
and **10 defects in shipped measurements** — three of P0 severity.

---

## 1. Defects in shipped numbers (fix before any new capability)

| # | Defect | Module | Severity |
|---|---|---|---|
| P0-1 | **Bootstrap RNG broken**: `lcg_state % n` on a raw LCG → for power-of-two sample counts every "resample" is a permutation → **CI width 0, SE 0** → adaptive stop fires at n=2/4/8/16, `relative_margin_of_error=0` passes the publication blocker, comparison verdicts forced on noise. Three duplicated copies. | M5 A3/G1 (91) | **P0** — publication-quality machinery is currently vacuous at common n |
| P0-2 | **macOS TCP kernel stats are dead code**: wrong sockopt constant (0x24 vs xnu 0x106; verified failing live) — every macOS RTT/cwnd/retrans field has always been None while docs claim coverage. **Latent trap**: naive fix arms ms-vs-µs (1000× low srtt), bytes-vs-segments cwnd, and a wrong struct offset. | M1 C-1..C-3 (74) | **P0** — silent no-data + a confidently-wrong-data trap behind it |
| P0-3 | **`dns_ms` cache contamination**: hickory's in-process LRU (8192 entries) never disabled → attempts 2..N measure a hashmap (~µs) labeled "system (…:53)". | M2 D1 (88) | **P0** — affects every resolving mode |
| P1-4 | **`rpm` flatters the worst links**, twice: sparse UDP probes get queue-jump priority on fq_codel/CAKE (factor≈1.0 exactly where bufferbloat lives) + loaded RTTs >1s censored as loss (optimistic bias). | M4 §2.1/2.3, M5 G4 | P1 — headline metric, wrong-answer class |
| P1-5 | **`path` mode ECMP artifact**: dest port varies per TTL → each hop measured on a different network path (pre-Paris-traceroute). Port variation is vestigial → fix is trivial. | M4 (part of 88) | P1 |
| P1-6 | **Regression detection is a dead stub the UI claims is live** ("automatically flagged when p50 +10%…" — that policy exists nowhere; zero callers). | M5 G2 (83) | P1 — trust in the product's own claims |
| P1-7 | **Phase contamination**: only the benchmark artifact excludes warmup/cooldown; console/HTML/Excel compute stats over raw attempts → human surfaces disagree with the artifact. | M5 G3 (76) | P1 |
| P2-8 | **M3 trust micro-set**: pageload counts 404s as "fetched"; asset-timing misalignment on failure; browser `transferred_bytes` contradicts its definition; two TTFB semantics share one name. | M3 G12 (74) | P2 |
| P2-9 | **`jitter_ms` mislabeled**: actually mean-\|IPDV\| (RFC 3393-ish), not RFC 3550; rename/document + add PDV percentiles (data already persisted). | M4 §1 | P2 |
| P2-10 | **`ocsp_stapled` aging false-negative**: Let's Encrypt ended OCSP (2025) → field increasingly reads false innocently; needs UI annotation. Also: env-check "p95" from n=5 is a max; `content_encoding` dormant (no Accept-Encoding sent). | M2, M5, M3 | P2 |

## 2. Unified capability priorities (deduped across modules)

**Tier 1 — structural, high-value (80+):**
- **QUIC transport stats** — `quinn::Connection::stats()` never called; h3 probes carry zero transport facts (M1 90 / M3 88 — independent agreement)
- **Full Linux `tcp_info`** — ~50 unread fields incl. the busy/rwnd/sndbuf-limited attribution triad, `bytes_retrans`, `delivered_ce` (ECN/L4S) (M1 88)
- **Responsiveness-spec conformance program** — multi-connection ramp + saturation detection + HTTP self-probes + upload + honest naming (M4 88 / M1 76 / M5 G4)
- **STAMP (RFC 8762) sender+reflector** — we own both ends; 44-byte format unlocks directional loss, per-direction jitter *without clock sync*, processing-corrected RTT (M4 84)
- **Core Web Vitals via CDP** — LCP/CLS/FCP/TBT, verified feasible with chromiumoxide (M3 82)
- **CDP waterfall + wire bytes** — data already arrives in subscribed events (M3 80)
- **UDP local-drop vs path-loss split** — `SO_RXQ_OVFL`; today rcvbuf overflow reports as network loss (M1 80)

**Tier 2 — high-value depth (70-79):** SVCB/HTTPS type-65 capture (75), multi-connection throughput à la ndt7-vs-Ookla (76), RFC 3357 burst-loss patterns (76), DNS TTLs (74, parsed-and-discarded), reordering+duplication counters (74, detected-and-discarded), key-exchange group (74, one rustls call), h3-DNS unification (71), median±CI on user surfaces (71, post-RNG-fix).

**Tier 3 (60-69):** DoH/DoT/DoQ comparison (68), chain trust-path diagnosis (69), RIPE-class DNS response metadata (70→here for effort), PQC probing (64, needs a ring-policy owner decision), netstat/snmp system-delta validity capture, SNTP burst, tokio scheduler-delay sentinel.

**Explicitly rejected across modules (42 items)** — eBPF/raw-socket/kernel-timestamping (violate unprivileged-first), qlog (quinn lacks it), full TWAMP-Control, MDA multipath, HdrHistogram, BCa bootstrap, INP/Speed-Index in lab, server push (dead), OCSP live-fetch timing (ecosystem moved on), DANE, Poisson sampling, per-row p-values, and more — each with reasons in its module report.

## 3. Cross-module themes

1. **"Already in hand, thrown away"** — the most common gap shape: quinn stats (never read), CDP waterfall (subscribed, unread), DNS TTLs (parsed, discarded), reordering/duplication evidence (detected, discarded), per-probe timelines (persisted, unanalyzed). Cheapest value in the codebase.
2. **The both-ends superpower** — STAMP, responsiveness self-probes, directional loss: standards-grade capabilities that are uniquely cheap because tester and endpoint are ours.
3. **Label honesty** — "RPM", "jitter", "Apple-style", macOS coverage claims: several names promise conformance the implementation doesn't deliver. Renaming/annotating is part of measurement trust.
4. **The statistics layer lagged the probe layer** — probes got trust-audited repeatedly (V-series); the stats machinery (RNG, CI, gates) never did, and it held the biggest P0.

## 4. Proposed execution order

1. **Wave T (trust)** — all §1 defects in one integrated pass: RNG fix (+dedupe 3 copies + n=8 regression test), macOS TCP fix *with units*, DNS cache defeat/label, h3-DNS unify, path 5-tuple, rpm censoring surface + labels, M3 micro-set, phase filtering, jitter rename, OCSP annotation. Mostly small diffs; ends the confidently-wrong class.
2. **Wave S (symmetry)** — QUIC stats + full tcp_info + UDP drop-split: every transport number becomes attributed, h1/h2/h3 becomes symmetric.
3. **Wave R (responsiveness)** — the conformance program + STAMP (the two "own-both-ends" builds).
4. **Wave W (web)** — CWV + waterfall + multi-connection throughput.
5. **Tier-2/3 batch** — the depth list, ordered by score.

*Sources: five module reports, each with RFC/tool/API citations verified
against live sources and vendored dependency code. Prod at v0.28.81.*
