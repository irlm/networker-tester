# M5 — Statistics & Measurement Validity — Deep Audit (2026-07-27)

Module audit of the measurement engine's **statistical treatment (A)**, the
**measurement-validity envelope (B)**, and the **analysis layer (C)** — what we
compute, whether it is sound, what silently corrupts it, and what we fail to
derive from data we already collect. Goes deeper than
`docs/analysis/measurement-gap-analysis-2026-07.md` (which inventoried *what*
is measured; this audits *how honestly* it is summarized).

Method: full read of `metrics.rs` (Stats/CpuUsage/benchmark envelope),
`benchmark.rs`, `baseline.rs`, `clock_sync.rs`, `network_context.rs`,
`summary.rs`, `runner/rpm.rs`, `runner/udp.rs`, `output/json.rs` (benchmark
contract), orchestrator `reporter/{stats,comparison}.rs`, `types.rs`,
`collector.rs`, C# `RegressionAnalyzer.cs`, dashboard
`BenchmarkRegressionsPage.tsx`; cross-checked against benchmarking literature
(Tene, Kalibera & Jones, IETF IPPM responsiveness draft, percentile-bootstrap
coverage results). Scores follow the project rubric: **value 40 · trust 20 ·
effort-inverse 20 · fit 20**, numeric 0–100.

---

## Part A — Current state: conformance assessment per mechanism

### A0 Summary scorecard

| # | Mechanism | Where | Soundness |
|---|-----------|-------|-----------|
| A1 | Core `Stats` (min/max/mean/p50/p95@20/p99@100/stddev) | `metrics.rs:2720-2787` | **78** |
| A2 | Percentile/stddev implementation consistency | 4 implementations | **55** |
| A3 | Bootstrap CI engine (`DeterministicRng`) | `benchmark.rs`, `output/json.rs`, orchestrator `reporter/stats.rs` | **25 — P0 defect** |
| A4 | Adaptive sampling (`target_relative_error`, pilot) | `benchmark.rs:83-346`, `target_runner.rs:532-719` | **60** |
| A5 | Warmup / phase model | `target_runner.rs`, `output/json.rs:410-509` | **55** |
| A6 | Environment / stability noise gates | `baseline.rs`, `output/json.rs:861-1012` | **62** |
| A7 | Data-quality verdict (CV tiers, publication blockers) | `output/json.rs:1044-1096`, `reporter/stats.rs:73-85` | **58** |
| A8 | A/B comparison layer (CI overlap, Cohen's d) | `reporter/comparison.rs`, `reporter/stats.rs:221-253` | **60** |
| A9 | `rpm` latency-under-load probe | `runner/rpm.rs` | **68** |
| A10 | Cadence semantics / coordinated omission | `runner/udp.rs`, `runner/rpm.rs`, dispatch loop | **70** |
| A11 | Trust envelope (CPU, clock, network context, load) | `metrics.rs:555-870`, `clock_sync.rs`, `network_context.rs` | **82** |
| A12 | Analysis layer (trend/regression/correlation) | `RegressionAnalyzer.cs`, dashboard | **20 — P0 trust gap** |

Overall statistical maturity: **≈60/100** — a genuinely sophisticated
scaffold (bootstrap CIs, Tukey fences, MAD, skew/kurtosis, publication
blockers, phase model, CPU-trust gating) sitting on **one broken primitive**
(A3), **several honesty inconsistencies** (A2, A5, A6), and a **dead analysis
layer** (A12).

---

### A1 Core `Stats` — 78

`compute_stats` (`metrics.rs:2748-2775`): min/max/mean/p50, p95 gated at
`MIN_SAMPLES_P95 = 20`, p99 at `MIN_SAMPLES_P99 = 100`, `None` below.

**Sound:**
- The gate philosophy ("an interpolated tail percentile at small n is the max
  wearing a costume") is correct and rare in tooling of this class. Rendering
  suppressed percentiles as `—` (`summary.rs:228-230`) is honest.
- 100 %-loss UDP/ping/websocket attempts excluded from the RTT distribution
  via `primary_metric_value` (`metrics.rs:2858-2918`) — correct
  "no-samples ≠ 0.0" handling (trust audit V11).

**Flaws:**
1. **The gates are display-honesty gates, not inference gates, and are ~10×
   lenient by literature standards.** At n=20, the p95 estimate rests on ~1
   observation above it. Practitioner guidance for *claiming* a tail
   percentile is ~10 samples beyond the quantile → n ≥ 200 for p95, n ≥ 1000
   for p99 (see e.g. [StatsTest on P50/P95/P99 comparison sample
   sizes](https://www.statstest.com/percentiles-latency-comparing-p50-p95-correctly):
   "P99 comparisons require roughly 50× the data of mean comparisons"). The
   current thresholds are fine for *display* but the report never
   distinguishes "displayable" from "defensible". No CI accompanies any
   user-facing percentile (the CI machinery exists but is benchmark-only).
2. **Population stddev (÷n)** here vs **sample stddev (÷(n−1))** in
   `output/json.rs:721-726` and orchestrator `reporter/stats.rs:139-147`. At
   the default `--runs 3` that is a 22 % discrepancy in reported σ for the
   same data depending on which surface you read.
3. Mean-first presentation: for right-skewed latency, professional practice
   (SPEC-style run rules, Kalibera & Jones) leads with the median; the console
   table prints Mean before p50 and per-phase headline rows use plain means
   (`summary.rs:186-196`). The benchmark artifact correctly uses the median as
   primary estimator; the diagnostic surfaces don't state one.

### A2 Percentile/stddev implementation consistency — 55

Four percentile implementations coexist:

| Impl | Method | Used by |
|---|---|---|
| `metrics.rs:2777` `percentile_from_sorted` | linear interpolation (Hyndman-Fan R-7) | Stats, CpuUsage |
| `benchmark.rs:392` | R-7 (duplicate) | bootstrap intervals |
| `baseline.rs:100` | R-7 (duplicate) | baseline/env/stability RTT |
| `metrics.rs:2704` `aggregate_udp_rtts` | **nearest-rank (ceil)** | udp/ping/rpm/websocket p95 |

The nearest-rank p95 and the interpolated p95 differ at small n (at n=10,
nearest-rank returns the max; R-7 returns an interpolation between 9th and
10th). Two different "p95" definitions appear in the *same run report* for
different probes, unlabeled. Not a correctness bug per se — both are valid
estimators — but a consistency/trust defect, compounded by the ÷n vs ÷(n−1)
stddev split (A1.2). Three identical copies of `DeterministicRng` and two of
`percentile_from_sorted`/`median_from_sorted` also mean a fix must be applied
in several places (see A3).

### A3 Bootstrap CI engine — 25 — **P0 statistical defect**

All confidence intervals in the product — adaptive-stop error bounds
(`benchmark.rs:359-390`), the benchmark artifact's `ci95_lower/upper` and
`relative_margin_of_error` (`output/json.rs:783-788`), and the orchestrator's
`MetricSummary.ci95_*` used for comparison verdicts
(`reporter/stats.rs:87-118`) — come from a percentile bootstrap driven by this
generator:

```rust
fn next_u64(&mut self) -> u64 {
    self.state = self.state
        .wrapping_mul(6364136223846793005)   // Knuth MMIX LCG, mod 2^64
        .wrapping_add(1442695040888963407);
    self.state
}
fn next_index(&mut self, upper: usize) -> usize {
    (self.next_u64() as usize) % upper       // ← low bits, modulo
}
```

A raw power-of-two-modulus LCG has provably periodic low bits: bit k of the
state sequence has period 2^k (Hull–Dobell; a ≡ 1 mod 4, c odd → full period
mod 2^m for every m). `% upper` therefore:

- **For `upper` = any power of two (2, 4, 8, 16, 32…): the index sequence is
  an LCG mod `upper` with full period `upper`, i.e. each consecutive block of
  `upper` draws visits every index exactly once.** Each bootstrap "resample"
  of length n draws exactly n indices → every resample is a **permutation of
  the original data**, not a with-replacement resample. Every one of the
  1,024/2,048 resample medians equals the sample median. Consequences:
  - `bootstrap_median_interval` returns **CI width 0 and SE 0** whenever the
    per-case successful-sample count is a power of two;
  - the adaptive stop (`median_error_bounds` → `benchmark_adaptive_status`,
    `benchmark.rs:242-261`) declares `AccuracyTargetReached` **the moment a
    case reaches 2, 4, 8, or 16 samples**, with a zero-width interval claiming
    perfect precision — the sequential procedure stops at the first
    power-of-two count it encounters after `min_samples`;
  - the artifact publishes `relative_margin_of_error = 0`, which sails through
    the `> 0.05` publication blocker (`output/json.rs:1068-1071`) — a
    **publication-ready verdict earned by an RNG artifact**;
  - orchestrator comparisons degenerate: with zero-width CIs on both sides,
    `confidence_intervals_overlap` is true only when the two medians are
    *exactly* equal, so the "same within 5%" verdict
    (`reporter/comparison.rs:249-251`) is unreachable and every noise-level
    difference at 4 or 8 repeats is verdicted "faster"/"slower".
- **For any even `upper`:** because state parity strictly alternates
  (a, c odd), resample indices strictly alternate even/odd — each resample
  contains exactly n/2 draws from each parity class. That is stratified, not
  multinomial, resampling: bootstrap variance is systematically underestimated
  for *all* even sample counts, not just powers of two.
- For odd `upper` the lattice structure still degrades resample independence,
  plus ordinary modulo bias.

Even with a correct RNG, two further caveats apply and should be recorded in
the methodology string: (a) the percentile bootstrap **undercovers at small
n** — the bootstrap distribution of a median of n < ~15 values is supported on
a handful of points, so a "95 %" interval is optimistic (see
[Rousselet/garstats on the percentile
bootstrap](https://garstats.wordpress.com/2016/05/27/the-percentile-bootstrap/)
and small-sample coverage results in [Agarwal et al., *Deep RL at the Edge of
the Statistical Precipice*](https://arxiv.org/pdf/2108.13264) which recommends
interval estimates + IQM precisely because point estimates at 3–10 runs
mislead); a minimum-n gate (≥ ~10–20 per case) before *reporting* a CI is
warranted — today `median_error_bounds` engages at **n = 2**
(`benchmark.rs:312`). (b) resamples are drawn iid — see A4.3 autocorrelation.

Fix is hours, not days: finalize the LCG output (splitmix64 mix) or use a
seeded `rand::rngs::SmallRng`, replace `% upper` with Lemire bounded
rejection, deduplicate the three copies into one shared module, and pin a
regression test asserting **nonzero CI width at n = 8 with unequal values**
(the current golden tests in `reporter/tests.rs` pinned the buggy outputs).

### A4 Adaptive sampling / `target_relative_error` — 60

Design (`benchmark.rs`, `target_runner.rs:532-719`): pilot phase (default
6–12 samples) → bootstrap half-width of the median → per-case required-n
extrapolation `n_req = n·(ε_now/ε_target)²` → measured phase with stop-when-CI
≤ target, bounded by min/max samples and min duration; stop reasons logged;
plan provenance (`explicit` / `pilot-derived` / `pilot-assisted` /
`fixed-count`) recorded in the artifact.

**Assessment:**
- The *shape* is textbook: this is a Chow–Robbins-style fixed-width
  sequential confidence procedure, which is asymptotically consistent, and the
  1/√n half-width scaling used for the extrapolation is asymptotically right
  for the median (SE ≈ 1/(2f(m)√n)). Legitimate methodology — better than
  fixed-n for heteroscedastic targets.
- **Broken engine:** the stopping rule consumes the A3 intervals, so today
  stopping behavior is dominated by the power-of-two artifact, not by data.
- **Extrapolating from n = 6 is noise-driven** even with a correct bootstrap:
  the half-width estimate at pilot n has enormous variance, so `n_req` is a
  point estimate of a quantity with ~±50 % error. Mitigation: floor the pilot
  at ~10–12 and treat `n_req` as a lower bound (the current clamp to
  `cfg.runs` silently caps it — acceptable *because* the achieved
  `relative_margin_of_error` is honestly re-reported and blocks publication
  when the target is missed; that closing of the loop is genuinely good).
- **Optional-stopping bias:** testing the CI after every sample and stopping
  on success slightly biases achieved coverage below nominal (known property
  of sequential fixed-width procedures at small n). Second-order versus A3;
  acknowledge in the methodology string rather than fix.
- **Independence assumption:** consecutive attempts share path, kernel,
  connection-cache and thermal state — positive autocorrelation is expected
  (Kalibera & Jones, [*Rigorous Benchmarking in Reasonable
  Time*](https://kar.kent.ac.uk/33611/45/p63-kaliber.pdf), build their entire
  methodology around exactly this). Positive ρ → CI too narrow → premature
  stop → overclaimed precision. Nothing in the codebase measures lag-1
  autocorrelation despite the data being in hand.

### A5 Warmup / phase model — 55

Phases warmup → overhead → pilot → measured → cooldown exist with counts on
`TestRun` (`metrics.rs:372-383`) and per-sample `phase` +
`inclusion_status` (`excluded_phase_*`, `excluded_failure`,
`included_after_retry`) in the benchmark artifact — good, SPEC-flavored
anti-cherry-picking design (`output/json.rs:587-607`).

**Flaws:**
1. **Phase attribution is positional**, reconstructed from counts by index
   slicing (`output/json.rs:416-438`). Any consumer that reorders, filters, or
   concatenates attempts silently mislabels phases. A `phase` field on
   `RequestAttempt` would make this structural.
2. **Only the benchmark JSON artifact excludes non-measured phases.** The
   console stats table (`summary.rs:216-243`), the HTML report
   (`output/html/protocol_sections.rs:131,308`), and Excel
   (`output/excel.rs:211`) all run `compute_stats` over **raw
   `run.attempts`** — in benchmark mode, warmup/overhead/pilot/cooldown
   samples contaminate min/mean/p50/stddev on every human-facing surface.
   Cold-start pilot samples drag the mean up; the same run shows different
   numbers in the artifact vs the HTML.
3. **Fixed warmup counts, no steady-state detection.** Orchestrator
   `warmup_runs` (default 10) and connection-reuse warmups are fixed a priori.
   The literature is unambiguous that warmup length varies per configuration
   and that fixed counts either waste time or leak transient state into
   measurements (Kalibera & Jones 2013; [Traini et al., steady-state detection
   in Java, EMSE 2022](https://link.springer.com/article/10.1007/s10664-022-10247-x)).
   For network probes the effect is milder than for JITted VMs, so this is P3
   — but first-attempt effects (cold DNS cache, cold TLS session cache, TCP
   metrics cache) are measurably present in non-benchmark runs where *no*
   warmup exists and `--runs 3` stats include the cold outlier.

### A6 Environment / stability / noise gates — 62

Pre-run environment check (5 × 50 ms TCP-connect RTTs) and stability check
(12 × 50 ms) feed spread/jitter/loss publication gates
(`baseline.rs:5-8`, `output/json.rs:861-1012`); thresholds configurable via
`BenchmarkNoiseThresholds` (loss 5 %, jitter ratio 0.25, spread ratio 2.0, CPU
busy 85 %, steal 5 %).

**Sound:** gating publication on *environment* noise, not just sample noise,
is professional practice most tools skip; the CPU busy/steal gate with
honest-`None` min-window guards is exemplary.

**Flaws:**
1. **The gate statistic is a p95 computed from n = 5** (environment check) or
   n = 12 (stability) — at those n the "p95" *is* the max, so
   `spread_ratio = p95/p50` is really `max/median` of a handful of connects.
   One SYN caught by a scheduler hiccup flips the verdict; conversely a
   genuinely bursty link can pass between bursts. The tester's own
   `MIN_SAMPLES_P95 = 20` philosophy is violated by its own gating inputs.
   Raising defaults to ~20–30 samples costs ≈1–1.5 s.
2. **A ~600 ms pre-run window certifies a possibly multi-minute run.** Noise
   is asserted stationary. There is no *during-run* or *post-run* environment
   re-check to detect mid-run degradation (the CPU sampler covers tester
   contention during the run — the network side has no equivalent).
3. TCP-connect RTT is a reasonable proxy but conflates accept-queue delay on
   the target with path RTT; fine for a gate, worth a comment.
4. `jitter_ms` here and in `aggregate_udp_rtts` is mean absolute consecutive
   difference, labeled "RFC-3550-style". RFC 3550 specifies an EWMA
   (J += (|D|−J)/16); mean |Δ| is a fine estimator but the label overstates
   conformance. (The arrival-order-before-sorting fix from trust-audit V2 is
   correct and important.)

### A7 Data-quality verdict / CV gating — 58

CV tiers (≤3 % excellent / ≤8 % good / ≤15 % fair / else unreliable,
`reporter/stats.rs:73-85`), CV > 0.10 publication blocker, ROME > 0.05
blocker, sufficiency n ≥ 100 adequate / ≥ 30 marginal, kurtosis/skew warnings,
Tukey-fence outlier *flagging without dropping* (`output/json.rs:1044-1096`).
Yes, publication is CV-gated — the answer to the audit question — and the
sufficiency ladder (30/100) matches conventional guidance.

**Flaws:**
1. **The run-level `data_quality` is computed on a pooled cross-case
   aggregate** (`output/json.rs:483-498`): all cases sharing the primary phase
   are concatenated (`aggregate_attempts`) and one CV/skew/kurtosis is taken.
   A run mixing http1-1KB and http1-1MB (or ms-latency and MB/s-throughput
   cases — pooled regardless of unit) is bimodal by construction → CV is
   large → "Sample variability exceeds the publication threshold" fires on
   case *mix*, not noise; symmetrically, one noisy case can hide inside a
   quiet majority. Per-case summaries already exist (`summaries` vector) —
   quality should be the worst-of per-case verdicts, not the pooled one.
2. **CV is mean/σ — non-robust on skewed latency.** One tail sample at n = 20
   moves the tier two notches. MAD is already computed and then unused for
   tiering; a robust CV (1.4826·MAD/median) alongside would stabilize tiers.
3. **Tiering is sample-count-blind:** "excellent" can be awarded from 3
   repeats where the CV estimate itself has ~±40 % error. Tie the tier to
   `sample_count` (e.g. cap at "good" below n = 10).
4. The tester (`ci95` at any n ≥ 2, p999 at any n — `output/json.rs:733-739`
   computes p5…p999 **ungated**) contradicts the `Stats` gating philosophy in
   the very artifact meant for publication. p999 from 30 samples is the max,
   published without a flag.

### A8 A/B comparison rigor — 60

Orchestrator comparisons (`reporter/comparison.rs`): environment
comparability gate (fingerprint equality + RTT-ratio ≤ 1.5) **before** any
verdict — excellent and unusual; paired per-repeat ratios; Cohen's d; verdict
= "same within 5 %" iff |Δ%| ≤ 5 **and** CIs overlap.

**Flaws:**
1. **CI-overlap is used as an equivalence test, which it is not.**
   Overlapping 95 % CIs do not demonstrate similarity (absence of evidence),
   and *non*-overlap is a conservative ≈p < 0.006 test, so the current rule is
   simultaneously too eager to claim difference (with A3 zero-width CIs,
   catastrophically so) and philosophically unable to claim equivalence. The
   standard tool for "same within 5 %" is TOST (two one-sided tests) on the
   paired ratios — which are already computed (`paired_ratio_values`) and then
   only summarised, never tested.
2. **No rank-based test anywhere** (Mann-Whitney U / Cliff's delta). Cohen's d
   assumes roughly-normal, equal-variance samples; on skewed latency with
   n = 3–10 repeats a rank statistic is the defensible choice. Cliff's delta
   is ~15 lines and shares the sorting already done.
3. Cross-*target* and cross-*run* comparisons in the product UI (dashboard run
   detail, protocol comparison tables in `summary.rs`/HTML) present paired
   numbers with **no uncertainty at all** — differences of means at n = 3 are
   shown as facts.

### A9 `rpm` probe vs responsiveness methodology — 68

`runner/rpm.rs`: unloaded UDP echo baseline → 5 s saturating download +
100 ms-cadence UDP echoes → `rpm = 60000/loaded_avg`,
`bufferbloat_factor = loaded_avg/unloaded_avg`. The load-generator liveness
check (fail loudly rather than report an idle link as "loaded", lines
193-249) is exactly right.

Against [draft-ietf-ippm-responsiveness](https://datatracker.ietf.org/doc/draft-ietf-ippm-responsiveness/)
(the Apple RPM spec):

1. The spec computes RPM from a **single-sided trimmed mean at the 95th
   percentile (TM95)** of each latency component, then averages *foreign*
   responsiveness (fresh TCP/TLS/HTTP connections through the loaded
   bottleneck) with *self* responsiveness (HTTP probes on the load-bearing
   connections): `RPM = avg(60000/((TM(tcp_f)+TM(tls_f)+TM(http_f))/3),
   60000/TM(http_l))`. Ours is a plain mean of UDP echoes — a different
   quantity: UDP may be queued/QoS'd differently than the TCP flows that
   actually carry the load, and there is no on-connection component at all.
   Not wrong as *a* working-latency metric, but the "Apple-RPM-style" label
   (`rpm.rs:1`) overstates conformance; either annotate the deviation in the
   report or add the HTTP-probe component.
2. **Survivorship bias under loss:** loaded echoes that exceed
   `grace_ms = min(timeout, 1000 ms)` are recorded as *loss*, and `rpm` is
   computed from surviving echoes only (`rpm.rs:211`). On a severely bloated
   link (queue delay > 1 s) precisely the worst samples are censored →
   loaded_avg underestimates → RPM and bufferbloat_factor *flatter the worst
   links*. Right-censoring should be surfaced: report "n echoes unanswered
   within X ms" next to the loaded stats and treat loaded_avg as a lower
   bound when loss > 0 (or extend the grace drain to the window end).
3. **Self-contention:** the load generator and the echo receiver share one
   tokio runtime; loaded RTT includes the tester's own scheduler wakeup delay
   under CPU load from 32 MiB downloads. Currently invisible (see gap G9).
4. Loaded p95 comes from ~50 samples (5 s / 100 ms) via nearest-rank — below
   the tester's own honesty gate; at least label it.

### A10 Cadence semantics & coordinated omission — 70

- **The loaded rpm phase is open-loop** (fixed cadence regardless of echo
  arrival, `Pacing::Paced`, `rpm.rs:320-380`) — this is the correct,
  Tene-approved design and deserves credit: it does *not* suffer coordinated
  omission ([Tene, "How NOT to Measure Latency"; wrk2
  README](https://github.com/giltene/wrk2)). Cadence self-times from each
  send so the schedule drifts by per-iteration overhead rather than
  back-filling — acceptable at 100 ms, worth one comment.
- **The plain `udp`/`ping` probes are closed-loop** (send next after echo or
  timeout — `BackToBack`): during a stall the prober collects *fewer* samples
  per wall-second, so time-weighted latency is biased optimistic — the
  textbook CO mechanism. Severity is moderate because timeouts are recorded
  as loss (bad periods aren't fully invisible), but the RTT *distribution* is
  still under-weighted in bad periods.
- **The main dispatch loop is closed-loop by design** and that is fine —
  sequential attempts measure per-request *service time*, which is the honest
  semantic for a diagnostic prober. The invalid move would be presenting
  those percentiles as user-experienced latency under a target arrival rate;
  no current surface does, but nothing documents the distinction either. A
  one-line semantics note in the report ("percentiles of sequential probe
  service times, not of load-driven arrivals") closes the gap Tene's talk is
  about.

### A11 Trust envelope — 82 (strongest area)

- **CPU:** two-snapshot whole-run busy% + 1 s-sampled max/p95 with
  `MIN_CPU_WINDOW_MS`/`MIN_CPU_WINDOW_TICKS` guards, steal counted as busy,
  `None`-never-0 per platform, p95 gated at 20 samples
  (`metrics.rs:655-838`) — this is how it should be done everywhere.
- **Clock:** RFC 4330 SNTP one-shot with correct offset/delay algebra,
  kiss-of-death rejection, bounded by 2.5 s (`clock_sync.rs`). Weakness: a
  *single* exchange against an anycast pool — standard practice is a burst of
  4–8 exchanges keeping the minimum-delay sample (offset error is bounded by
  delay/2; one 200 ms-delay exchange gives a ±100 ms offset bound, useless
  against the `clock_skew_ms` heuristic it is meant to validate). Low effort,
  real gain.
- **NetworkContext / LoadSample:** interface kind, MTU, VPN heuristic, egress
  IP via connected-UDP route lookup; load average + MemAvailable with honest
  platform gaps. Good.
- **Holes in the envelope (all currently unmeasured):**
  - No system-wide network-stack deltas around the run: `/proc/net/snmp` +
    `/proc/net/netstat` (Linux) / `netstat -s` (macOS) before/after would
    give retransmits, RTO events, listen drops, ECN marks *during our run*
    for every mode — today per-socket TCP_INFO covers some modes only. Grep
    confirms nothing reads these paths.
  - No NIC offload state capture (`ethtool -k`: GRO/GSO/TSO/LRO). Offloads
    change timing semantics — batched delivery quantizes wire-arrival times
    and inflates single-stream latency jitter
    ([Red Hat NIC offloads guidance](https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/6/html/performance_tuning_guide/network-nic-offloads),
    [kernel timestamping docs](https://docs.kernel.org/networking/timestamping.html)) —
    and materially affect pcap-based retrans counts (capture sees super-frames).
  - No scheduler-delay sentinel: tokio wakeup latency adds directly to every
    `Instant`-measured RTT and is measurable with a trivial
    sleep-overshoot sampler; it is the missing counterpart to the CPU
    envelope (busy% says the *box* was busy; overshoot says *our runtime*
    was delayed).
  - Timer/power states: macOS timer coalescing can stretch default-QoS
    sleeps by milliseconds ([catnapgames
    measurement](https://www.catnapgames.com/2017/10/12/timer-coalescing-in-macos/));
    affects the 50 ms env-check cadence and 100 ms rpm cadence *spacing* (not
    the RTT measurements themselves, which are Instant-bracketed) — worth a
    doc note, not code.
  - Timer resolution & overhead: `Instant` is ns-resolution monotonic
    (mach_absolute_time / CLOCK_MONOTONIC); syscall + `Utc::now()` overhead is
    O(µs) against ms-scale claims — **no accounting needed**; a one-line
    statement in docs that sub-ms figures carry ~single-µs measurement
    overhead would pre-empt the question. The one distortion to note: on
    loopback targets, sub-100 µs "RTTs" are dominated by scheduler wakeups,
    and the report's Loopback classification already reference-flags these.
- **One defect:** orchestrator `collector.rs:35-50` sets
  `avg_cpu_fraction = peak_cpu_fraction = one instantaneous cpu_percent
  sample`. Fields named avg/peak that are a single point sample are fabricated
  semantics — the same sin the tester's CpuUsage was built to avoid (30/100
  for that function).

### A12 Analysis layer — 20 — **P0 trust gap**

- **Regression detection is a documented stub that the UI claims is live.**
  `RegressionAnalyzer.cs:43-61` states it contains "NO comparison logic, NO
  thresholds, NO DB queries, NO event emission"; `EmitRegressionEvent` has
  zero callers (grep). Meanwhile
  `dashboard/src/pages/BenchmarkRegressionsPage.tsx:77` tells users:
  *"Regressions are automatically flagged when a benchmark completes and its
  p50 latency increases by more than 10% or success rate drops below 99%
  compared to the baseline run."* That policy **does not exist anywhere in the
  codebase**. Every user of that page is being told an empty list means "no
  regressions" when it means "nothing ever checked". For a product whose brand
  is measurement trust, this is the single worst finding of this audit after
  A3.
- **No cross-run trend analysis** of any kind (EWMA, change-point,
  same-config drift) despite full run history in Postgres.
- **No correlation surfacing** within runs: cpu_usage samples, socket-stats
  retransmits, and per-attempt latencies coexist in one artifact and are never
  cross-referenced (e.g. Spearman ρ of busy% vs attempt latency; retrans > 0
  vs throughput dip). The measurement-gap audit's #1 finding (persistence)
  made this data reach the DB; nothing computes on it.
- Partial credit: `split_anomaly` (server-timing network-vs-server split) does
  propagate to the run detail, and the orchestrator's auto-rerun policy
  (`BenchmarkAutoRerunPolicy` — rerun weak-quality cases) is a real,
  functioning analysis loop.

---

## Part B — Gaps, scored (value 40 / trust 20 / effort-inv 20 / fit 20)

| # | Gap | What / why / path | V | T | E | F | **Score** |
|---|-----|-------------------|---|---|---|---|---|
| **G1** | **Fix the bootstrap RNG (A3)** | Splitmix64-finalize or seeded `SmallRng` + Lemire bounded index; dedupe the 3 `DeterministicRng` copies into one crate-shared module (tester also re-exports for orchestrator or duplicate-with-test); pin a test: n=8, distinct values ⇒ CI width > 0. Re-baseline the golden tests that froze buggy outputs. Unblocks every CI, stop-rule, ROME gate and comparison verdict in the product. | 34 | 20 | 19 | 18 | **91** |
| **G2** | **Make regression detection real — or retract the claim** | Implement the exact policy the UI already promises (p50 +10 % / success < 99 % vs config baseline artifact) in the run-complete path, call `EmitRegressionEvent`, persist rows the existing endpoint serves. If deferred, change the empty-state copy to "not yet implemented" the same day. | 34 | 18 | 13 | 18 | **83** |
| **G3** | **Phase-filter all human surfaces (A5.2)** | Add `phase` (or `is_measured`) to `RequestAttempt` at creation (removes positional fragility too); `summary.rs`/HTML/Excel stats use measured-only in benchmark mode, with warmup shown separately. | 26 | 18 | 16 | 16 | **76** |
| **G4** | **rpm right-censoring + spec labeling (A9)** | Extend grace drain to window end; report `unanswered_within_grace` count; mark loaded stats as lower bounds when > 0; rename docs from "Apple-RPM-style" to "RPM-inspired (UDP echo)" or add the draft's TM95 + foreign/self components. | 26 | 17 | 14 | 16 | **73** |
| **G5** | **CIs on user-facing stats** | Median ± bootstrap CI (post-G1) in console/HTML when n ≥ 10; keep suppression below. Closes "differences shown as facts" (A8.3) for single-run surfaces. | 27 | 15 | 14 | 15 | **71** |
| **G6** | **System-wide netstat/SNMP deltas (A11)** | Snapshot `/proc/net/snmp`+`/proc/net/netstat` (`netstat -s` on macOS) at run start/end; report ΔRetransSegs, ΔInErrs, ΔListenDrops in the trust envelope. Covers all modes incl. UDP/H3 where TCP_INFO can't. | 27 | 15 | 14 | 14 | **70** |
| **G7** | **CI-count gate + autocorrelation guard (A3/A4)** | Don't report/act on a CI below n=10; compute lag-1 ρ of measured samples; when ρ > 0.3, inflate CI via n_eff = n(1−ρ)/(1+ρ) (or switch to block bootstrap) and say so in `data_quality.warnings`. Kalibera-Jones-lite. | 23 | 16 | 13 | 14 | **66** |
| **G8** | **Per-case data quality (A7.1)** | `data_quality` = worst of per-case verdicts; never pool across `metric_unit`s; keep the pooled aggregate only as a convenience row explicitly labeled. | 21 | 15 | 15 | 14 | **65** |
| **G9** | **Scheduler-delay sentinel (A9.3/A11)** | Background task: `sleep(10 ms)` loop measuring overshoot; attach overshoot mean/max/p95 to the run envelope; flag rpm loaded phase when overshoot p95 > ~2 ms. Makes tokio noise visible instead of embedded. | 22 | 15 | 14 | 13 | **64** |
| **G10** | **Tail-gate consistency (A2/A7.4)** | One shared percentile fn; gate artifact p95/p99/p999 with the same MIN_SAMPLES rules (or emit `*_gated: true` flags); switch `aggregate_udp_rtts` p95 (n=10 default) to `Option` under the gate; unify sample stddev (÷(n−1)) everywhere. | 20 | 16 | 15 | 13 | **64** |
| **G11** | **Rank-based comparison + TOST (A8)** | Cliff's delta alongside Cohen's d; "same within 5 %" verdict via TOST on the already-computed paired ratios; CI-overlap demoted to a display hint. | 21 | 14 | 13 | 14 | **62** |
| **G12** | **Environment-gate sample sizes + during-run recheck (A6)** | Env/stability defaults 5/12 → 20–30 samples; optional post-run stability recheck with pre-vs-post delta in `data_quality`. | 20 | 14 | 14 | 13 | **61** |
| **G13** | **Correlation surfacing (A12)** | Per-run: Spearman busy%↔latency (needs timestamps on CPU samples — add), retrans↔throughput across attempts; report as annotations ("3 slowest attempts coincide with CPU > 80 %"). | 23 | 10 | 11 | 14 | **58** |
| **G14** | **Orchestrator collector honesty (A11)** | Sample the metrics agent every N s during the run; compute real avg/peak, or rename fields to `sampled_cpu_fraction`. | 16 | 15 | 13 | 12 | **56** |
| **G15** | **NIC offload capture (A11)** | `ethtool -k` (Linux; best-effort None elsewhere) into NetworkContext; annotate capture.rs retrans counts when GRO/LRO active. | 16 | 13 | 13 | 12 | **54** |
| **G16** | **SNTP burst (A11)** | 4 exchanges, keep min-delay sample, report offset ± delay/2 bound. | 12 | 12 | 16 | 11 | **51** |
| **G17** | **Cross-run trend detection (A12)** | EWMA or simple z-vs-trailing-window per config primary metric; annotate run list. Overlaps G2; do after it. | 20 | 10 | 8 | 12 | **50** |
| **G18** | **Steady-state warmup detection (A5.3)** | Post-hoc lag/run-sequence heuristic marking "warmup may be insufficient" instead of new adaptive machinery. | 12 | 9 | 8 | 12 | **41** |

---

## Part C — Considered and REJECTED

| Idea | Why rejected |
|---|---|
| **HdrHistogram integration** | Run-level sample counts (3–100) don't need histogram compression, and the rpm loaded phase (~50 samples) is below the regime where HDR precision matters. Revisit only if per-request streaming at 10k+ samples arrives. |
| **Coordinated-omission "correction"** (HdrHistogram-style expected-interval backfill) for the closed-loop udp/ping probes | Backfilling fabricates samples we did not take; our probes report *service time* + explicit loss, which is honest as-is. The right fix is the semantics note (A10) and the open-loop rpm mode we already have — not synthetic data. |
| **BCa or studentized bootstrap** to replace the percentile bootstrap | Real coverage gains are second-order at our n once G1 (RNG) and G7 (n-gate, autocorrelation) land; BCa's acceleration estimate is itself unstable at n < 20. Not worth the complexity today. |
| **p-values (Mann-Whitney) on every dashboard comparison row** | Invites multiple-comparison abuse and dichotomous thinking at n = 3–10; effect size + CI (G5/G11) communicates the same information without the false authority. Rank stats enter only the orchestrator verdict path. |
| **Trimmed/winsorized primary statistics** | The existing report-don't-drop policy (Tukey-flag + keep, anti-cherry-picking strings in the artifact) is the SPEC-aligned choice; silently robustifying the headline number would trade honesty for stability. The one sanctioned trimmed mean is the IETF TM95 inside a future spec-conformant rpm (G4). |
| **Welch t-tests on latency means** | Normality assumption indefensible for skewed latency at these n; median + rank methods dominate. |
| **Kernel timestamping (SO_TIMESTAMPING/PTP) for RTT probes** | Removes O(10 µs) of userspace noise from O(1 ms+) measurements — precision we can't currently use, at high platform-specific cost. The scheduler sentinel (G9) buys the same trust for 5 % of the effort. |
| **Adaptive sampling for regular (non-benchmark) runs** | A 3-run diagnostic is a spot check; imposing stop-rules there adds latency and config surface for no decision anyone makes from it. |

---

## Part D — Top-5 shortlist

1. **G1 (91) — Fix the bootstrap RNG.** Every CI, adaptive stop,
   publication gate, and comparison verdict in the product currently degrades
   to garbage at power-of-two sample counts and is quietly narrow everywhere
   else. Hours of work; add the n=8 nonzero-width regression test.
2. **G2 (83) — Ship (or retract) regression detection.** The UI documents a
   policy no code implements. Either is acceptable; the current state is not.
3. **G3 (76) — Phase-filter console/HTML/Excel stats** and put `phase` on the
   attempt. Ends warmup/pilot/cooldown contamination of every human-facing
   number in benchmark mode.
4. **G4 (73) — rpm censoring + honest labeling.** Stop flattering the worst
   links; align naming with draft-ietf-ippm-responsiveness or implement TM95 +
   foreign/self probes.
5. **G5 (71) — Median ± CI on user-facing stats** (after G1), with the
   existing suppression discipline below n = 10.

*Fold G10's shared-percentile/stddev unification into whichever of G1/G3
lands first — same files, near-zero marginal cost.*

---

## Sources

- Gil Tene, "How NOT to Measure Latency" / coordinated omission:
  [wrk2](https://github.com/giltene/wrk2),
  [ScyllaDB on CO](https://www.scylladb.com/2021/04/22/on-coordinated-omission/),
  [Brave New Geek summary](https://bravenewgeek.com/everything-you-know-about-latency-is-wrong/)
- [IETF draft-ietf-ippm-responsiveness](https://datatracker.ietf.org/doc/draft-ietf-ippm-responsiveness/)
  (TM95 trimmed mean; foreign + self responsiveness; RPM formula)
- Kalibera & Jones, [*Rigorous Benchmarking in Reasonable Time*, ISMM '13](https://kar.kent.ac.uk/33611/45/p63-kaliber.pdf)
  (autocorrelation, warmup detection, repetition hierarchy);
  [Traini et al., EMSE 2022 steady-state study](https://link.springer.com/article/10.1007/s10664-022-10247-x)
- Percentile bootstrap behavior & small-n coverage:
  [Rousselet, garstats](https://garstats.wordpress.com/2016/05/27/the-percentile-bootstrap/);
  [Hesterberg, *What Teachers Should Know about the Bootstrap*](https://arxiv.org/pdf/1411.5279)
  (percentile intervals too short in small samples);
  [Agarwal et al., NeurIPS 2021](https://arxiv.org/pdf/2108.13264)
- Tail-percentile sample sizing:
  [StatsTest on P50/P95/P99 comparisons](https://www.statstest.com/percentiles-latency-comparing-p50-p95-correctly);
  [MLPerf Inference sample-count formula](https://arxiv.org/pdf/1911.02549)
- Validity envelope:
  [Red Hat NIC offloads](https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/6/html/performance_tuning_guide/network-nic-offloads);
  [Linux kernel timestamping](https://docs.kernel.org/networking/timestamping.html);
  [macOS timer coalescing measurements](https://www.catnapgames.com/2017/10/12/timer-coalescing-in-macos/);
  [Timer coalescing overview](https://en.wikipedia.org/wiki/Timer_coalescing)

*Code anchors verified at HEAD (main, 2026-07-27): `crates/networker-tester/src/{metrics,benchmark,baseline,clock_sync,summary,target_runner}.rs`, `runner/{rpm,udp}.rs`, `output/json.rs`, `output/html/protocol_sections.rs`, `output/excel.rs`; `benchmarks/orchestrator/src/{types,collector}.rs`, `reporter/{stats,comparison}.rs`; `src/Networker.ControlPlane/Provisioning/RegressionAnalyzer.cs`; `dashboard/src/pages/BenchmarkRegressionsPage.tsx`.*
