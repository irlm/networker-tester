# Benchmark Measurement and Reporting Requirements

## Goal

Define a benchmark-grade measurement, processing, storage, comparison, and reporting pipeline for `networker-tester` and the benchmark orchestrator so results are reproducible, comparable, and reviewable in a professional and scientific way.

## Background

`networker-tester` already captures strong raw probe data, protocol timings, success/failure details, packet capture summaries, and host metadata. However, the benchmark workflow is still closer to an operational test harness than a benchmark framework.

Network benchmarks are particularly sensitive to OS noise, background traffic, TCP congestion control, transient packet loss, clock drift, and path instability. The pipeline therefore needs strong environment capture, explicit noise monitoring, and methodology that distinguishes latency-sensitive workloads from throughput-oriented workloads.

The main current gaps are:

- The orchestrator expects a summarized benchmark payload, but `networker-tester --json-stdout` currently emits raw `TestRun` JSON.
- The current statistics layer is descriptive but limited to `count`, `min`, `mean`, `p50`, `p95`, `p99`, `max`, and `stddev`.
- Warmup behavior, retries, and measured samples are not represented as explicit benchmark phases.
- The current benchmark reporter favors "best warm result" style summaries instead of statistically sound multi-run comparisons.
- The database and dashboard do not yet preserve or expose enough experimental context to support strong claims.

## Reference Inspiration

This work should borrow the process principles of BenchmarkDotNet, without attempting to copy its implementation one-to-one:

- Keep raw measurements, not just summaries
- Separate benchmark phases such as pilot, warmup, and measured execution
- Use accuracy-based stop criteria instead of relying only on fixed iteration counts
- Treat comparisons as distributions relative to an explicit baseline
- Export both raw and summarized artifacts
- Make methodology visible to the reader

## Design Principles

- Raw measurements are the source of truth.
- Benchmark phases must be explicit and queryable.
- Benchmark summaries must be derived, not hand-curated.
- Results must be explainable from stored metadata and published methodology.
- Cross-run comparison must only happen when environment and case definitions are comparable.
- Reports must show uncertainty, not just point estimates.
- Diagnostic data and benchmark claims should be related but clearly separated.
- The framework must support both latency-oriented and throughput-oriented workloads with workload-appropriate primary metrics.
- Network noise must be measured, recorded, and surfaced as part of benchmark quality.

---

## 1. Scope

### In Scope

- `networker-tester` benchmark-oriented JSON output
- Benchmark orchestrator ingestion and aggregation
- Benchmark database schema additions
- Statistical processing for benchmark summaries and comparisons
- HTML and dashboard reporting requirements for benchmark views
- Methodology and acceptance criteria for publishing benchmark results

### Out of Scope for Initial Delivery

- Reproducing every BenchmarkDotNet feature
- External academic peer review workflow
- Automatic benchmark cost modeling
- Full R-style offline statistical report generation
- Replacing existing operational reports for non-benchmark runs

---

## 2. Problem Statement

The system can currently answer "what happened during this run?" but cannot yet answer "what can we confidently claim from repeated benchmark runs?" with the level of rigor expected for a public benchmark comparison.

To support professional comparisons, the system needs:

- A normalized benchmark data contract
- Explicit sample lifecycle and inclusion rules
- Stronger statistical summaries
- A first-class baseline and comparison model
- Exportable raw data for auditability
- Reports that show both central tendency and uncertainty

---

## 3. Benchmark Lifecycle Requirements

Each benchmark case MUST be executed as a sequence of named phases.

### Required Phases

1. `pilot`
2. `warmup`
3. `measured`

### Optional Phases

1. `environment-check`
2. `stability-check`
3. `cooldown`
4. `overhead`

### Lifecycle Rules

- The system MUST tag every sample with its phase.
- Warmup samples MUST NOT be included in published measured statistics.
- Retry attempts MUST be tagged separately from first-attempt measured samples.
- Failed attempts MUST be preserved in raw data, even when excluded from latency or throughput distributions.
- Cold and warm benchmark modes MUST be modeled as different benchmark cases or distinct named scenarios, not mixed into the same measured distribution.
- A benchmark launch MUST record when it started, when each phase started, and when each phase ended.

### Stability-Check Requirements

- The benchmark lifecycle SHOULD support a `stability-check` phase before pilot.
- The stability-check phase SHOULD measure idle latency, jitter, packet loss, and other available noise indicators for a short configurable duration.
- Stability-check results MUST be stored as part of the benchmark environment or data quality output.
- Runs with noise above a configurable publication threshold SHOULD be flagged and MAY be rejected from publication-ready summaries.

### Pilot Requirements

- The pilot phase MUST estimate a reasonable measured sample target before the full measured phase begins.
- The pilot phase SHOULD use minimum measured duration, minimum sample count, and relative error targets rather than a single fixed run count.
- The pilot phase SHOULD estimate coefficient of variation and other stability indicators to guide measured-phase stop targets.
- The system MUST allow fixed-count execution for deterministic or debugging scenarios, but it SHOULD NOT be the only supported mode.

### Measured Phase Stop Rules

The measured phase MUST support all of the following:

- minimum measured samples
- maximum measured samples
- minimum measured wall-clock duration
- target maximum relative error
- target maximum absolute error

The measured phase SHOULD stop early once the configured accuracy target is met after the minimum sample requirement has been satisfied.
The default interpretation of relative error SHOULD be the half-width of the configured confidence interval relative to the chosen primary estimator for the workload.

---

## 4. Benchmark Data Model Requirements

The benchmark pipeline MUST separate raw operational data from normalized benchmark data.

### Required Logical Entities

1. `BenchmarkRun`
2. `BenchmarkCase`
3. `BenchmarkLaunch`
4. `BenchmarkSample`
5. `BenchmarkSummary`
6. `BenchmarkComparison`
7. `BenchmarkEnvironment`
8. `BenchmarkMethodology`

### BenchmarkRun

A benchmark run represents one orchestrated benchmark execution session.

It MUST include:

- run id
- suite name
- benchmark version
- git commit or source revision
- orchestrator version
- tester version
- start and finish timestamps
- operator or automation identity when available
- config hash
- methodology version

### BenchmarkCase

A benchmark case represents the exact thing being compared.

It MUST include:

- protocol or workload type
- payload size
- concurrency
- cold or warm scenario
- connection reuse mode
- target URL or logical target id
- client and server runtime identities
- any flags that materially affect transport behavior

### BenchmarkSample

A benchmark sample represents one measured attempt or one aggregated iteration, depending on the benchmark mode.

Each sample MUST record:

- benchmark run id
- benchmark case id
- launch index
- phase
- iteration or sequence index
- success or failure
- retry count
- inclusion status for summary statistics
- primary metric value
- metric unit
- per-phase timing fields when applicable
- error classification when unsuccessful
- wall-clock timestamps
- optional secondary transport metrics such as jitter, packet loss rate, retransmission count, and goodput when relevant to the workload

### BenchmarkEnvironment

The stored environment MUST support reproducibility and comparability.

It MUST include when available:

- client host info
- server host info
- operating system and version
- CPU model and logical cores
- memory size
- region or datacenter
- network baseline data
- idle noise or stability-check results
- CPU utilization or load averages during the run
- network interface counters such as drops, retransmits, and interrupts when available
- kernel or OS networking settings that materially affect results such as congestion control, MTU, offload features, and socket buffer settings
- background traffic or competing-flow indicators when detectable
- clock synchronization status between client and server when available
- power management state such as frequency scaling or turbo mode when detectable
- HTTP/TLS capabilities that materially affect the run
- packet capture availability and confidence

### Diagnostic Extensions

- The benchmark pipeline SHOULD support pluggable diagnosers or extensions for CPU, memory, network stack, packet analysis, and runtime-specific counters.
- Diagnostic extensions MUST identify their version and collection policy in benchmark metadata.
- Diagnostic extensions MUST NOT silently change the definition of the benchmark case or summary without updating the methodology version.

---

## 5. Output Contract Requirements

`networker-tester --json-stdout` MUST support a benchmark-oriented output mode that can be safely consumed by the orchestrator.

### Required Output Shape

The benchmark JSON output MUST contain both:

1. raw benchmark data
2. normalized summary data

### Required Top-Level Sections

- `metadata`
- `environment`
- `methodology`
- `cases`
- `samples`
- `summaries`
- `comparisons` when a baseline is available
- `data_quality`
- `diagnostics`

### Compatibility Rules

- The raw `TestRun` shape MAY still be emitted for backward compatibility in diagnostic contexts.
- The orchestrator MUST consume the normalized benchmark summary contract rather than inferring metrics from ad hoc fields.
- The benchmark JSON contract MUST have a version number.
- Unknown fields MUST be safely ignored by downstream readers.

### Data Quality Requirements

The `data_quality` section MUST support machine-readable publication checks and SHOULD include:

- noise level classification such as `low`, `medium`, or `high`
- sample stability as coefficient of variation
- sufficiency classification such as `adequate`, `marginal`, or `insufficient`
- warning list
- publication readiness flag

---

## 6. Statistical Processing Requirements

The benchmark summary layer MUST compute more than point estimates.

### Required Summary Statistics

For each benchmark case, the summary MUST include:

- sample count
- included sample count
- excluded sample count
- success count
- failure count
- min
- mean
- median
- max
- p5
- p25
- p50
- p75
- p95
- p99
- standard deviation
- standard error
- variance
- coefficient of variation
- interquartile range
- lower fence
- upper fence
- outlier counts
- skewness
- kurtosis
- median absolute deviation or equivalent robust spread metric
- confidence interval at the configured report level

### Statistical Policy Requirements

- The report MUST state which samples are included and excluded.
- The report MUST state the outlier policy in plain language.
- The system MUST preserve raw samples even when outliers are excluded from a displayed summary.
- The benchmark methodology MUST declare the confidence level used for published summary intervals.
- The default publication policy SHOULD use a robust outlier strategy such as Tukey 1.5xIQR or a MAD-based rule for primary summaries while retaining flagged raw samples.
- The statistics layer SHOULD support non-parametric confidence intervals such as bootstrap or order-statistic-based intervals for network-facing workloads.
- The report SHOULD classify coefficient of variation into quality tiers such as excellent, good, or unreliable.
- The statistics layer SHOULD detect and flag heavy-tailed or potentially bimodal distributions.
- Latency-oriented workloads SHOULD emphasize median and tail metrics such as p95 and p99, with mean shown as a secondary metric.
- Throughput-oriented workloads SHOULD report both central tendency and stability, including CV and error rate.

### Phase Separation Requirements

- Warmup samples MUST be excluded from measured statistics.
- Failed samples MUST contribute to error-rate metrics even when they do not contribute to latency distributions.
- Cold-start summaries MUST not be merged into warm steady-state summaries.

---

## 7. Comparison Requirements

The comparison system MUST be baseline-first rather than leaderboard-first.

### Baseline Rules

- Every comparison group MUST allow exactly one explicit baseline case.
- The baseline selection MUST be stored in the summary artifact.
- A result MUST NOT be compared to a baseline if the environment or case definition differs in a material way.
- The system SHOULD support multiple named baselines in advanced comparison workflows, such as previous release and reference implementation, while still requiring exactly one primary baseline for ranking and verdicts.

### Required Comparison Outputs

For each comparable case relative to baseline, the system MUST compute:

- absolute delta
- percent delta
- ratio
- ratio spread or ratio standard deviation
- baseline confidence interval
- candidate confidence interval
- comparison sample counts
- practical significance or effect size measure

### Comparison Semantics

- The comparison layer SHOULD treat ratio as a distribution, not only a ratio of means.
- The comparison layer SHOULD support a verdict such as `faster`, `slower`, or `same within threshold`.
- The comparison layer SHOULD support configurable equivalence thresholds for practical significance.
- The comparison layer SHOULD support both cold and warm comparisons, but they MUST be reported separately.
- The default verdict policy SHOULD combine confidence intervals with a configurable practical equivalence threshold.

### Comparison Gating Requirements

The methodology MUST define what counts as a materially different environment. The default checklist SHOULD include:

- CPU family or model changes
- logical core count changes
- OS major version changes
- network path or region changes
- MTU or congestion-control changes
- power-management changes
- significant idle-latency or noise-profile drift

### Anti-Cherry-Picking Rules

- Reports MUST NOT rank implementations by their single best warm result.
- Published ranking tables MUST use a defined aggregation method such as median of repeated launches or mean with confidence intervals.
- Repeated launches MUST retain launch identity and MUST NOT be blindly pooled without a declared aggregation policy.
- The default aggregation policy SHOULD use repeated-launch summaries with launch-to-launch variance visible to the reader.
- The aggregation method MUST be stated in the report.

---

## 8. Storage Requirements

The benchmark store MUST preserve enough information to regenerate summaries and explain comparisons.

### Database Requirements

The database SHOULD support dedicated benchmark tables or equivalent normalized storage for:

- benchmark runs
- benchmark cases
- benchmark launches
- benchmark samples
- benchmark summaries
- benchmark comparisons
- benchmark environments

### Persistence Rules

- The run-level environment and methodology MUST be persisted, not only attached to transient JSON.
- Per-sample raw benchmark data MUST be retained long enough to support reprocessing.
- The stored summary MUST reference the raw sample set it was derived from.
- Recomputed summaries MUST either replace older derived summaries with version tracking or coexist as a new summary version.

---

## 9. Export Requirements

The system MUST support export formats for both human review and downstream analysis.

### Required Exports

- JSON summary export
- JSON raw sample export
- CSV raw sample export
- HTML report

### Recommended Exports

- Markdown summary export
- database-backed API responses for dashboard comparison views
- histogram or binned distribution export
- R or Python friendly columnar export such as Parquet

### Export Rules

- Exported raw sample files MUST include benchmark case identifiers and phase labels.
- Exported summaries MUST include methodology metadata.
- Exported comparisons MUST include baseline identity and comparison policy.
- Exported distribution files SHOULD preserve enough detail for offline re-analysis.

---

## 10. Reporting and Dashboard Requirements

Reports MUST look professional and MUST communicate methodological confidence, not only results.

### Required Report Sections

- executive summary
- methodology
- limitations and threats to validity
- environment and hardware table
- benchmark case definitions
- per-case summary table
- baseline comparison table
- data quality summary
- data quality notes
- raw sample availability note

### Required Visualizations

- confidence-interval-aware comparison chart
- box plot or violin plot distribution view per key case
- run-to-run variance view
- cold versus warm comparison view
- error-rate view

### Reporting Rules

- The report MUST display sample counts for every published statistic.
- The report MUST clearly distinguish measured data from warmup and diagnostic data.
- The report MUST label whether higher or lower is better for each primary metric.
- The report MUST show when a result is based on insufficient data.
- All comparison charts MUST display uncertainty such as confidence intervals, error bars, or equivalent shading.
- All charts SHOULD include a caption stating the confidence level and sample size.
- The report SHOULD highlight when error bars overlap or when a comparison is within the configured equivalence threshold.
- The report SHOULD allow inspection of raw samples for auditability.
- The report SHOULD include a traffic-light style data quality summary for publication readiness.

---

## 11. CLI and Orchestrator Requirements

### CLI Requirements

- `networker-tester` MUST offer a benchmark summary mode suitable for machine consumption.
- The CLI SHOULD allow explicit control of pilot, warmup, measured sample counts, and accuracy thresholds.
- The CLI SHOULD allow explicit control of stability-check duration, noise thresholds, and case randomization.
- The CLI SHOULD expose a methodology preset for reproducible benchmark runs.
- The CLI SHOULD provide workload-aware presets such as strict latency and throughput.

### Orchestrator Requirements

- The orchestrator MUST consume the normalized benchmark contract instead of parsing loosely named summary keys.
- The orchestrator MUST preserve multiple launches per language or runtime and summarize them with a defined aggregation policy.
- The orchestrator MUST support explicit baseline designation.
- The orchestrator SHOULD reject or flag comparisons where the case definition or environment differs materially.
- The orchestrator SHOULD recommend or schedule reruns when data quality is poor, confidence intervals are too wide, or noise exceeds configured thresholds.

---

## 12. Non-Functional Requirements

- Benchmark processing MUST be deterministic for a given raw sample set and methodology version.
- The pipeline MUST be versioned so old artifacts remain readable.
- Derived summaries SHOULD be cheap to regenerate from stored raw data.
- The default report SHOULD remain understandable to non-statisticians.
- The system SHOULD avoid statistical claims that are stronger than the collected data supports.
- The pipeline SHOULD minimize avoidable interference through optional CPU pinning, network isolation, power-state controls, or similar environment controls when supported.
- The system SHOULD expose a repeatability or stability score across launches.

---

## 13. Acceptance Criteria

This initiative is acceptable for initial release when all of the following are true:

- The orchestrator consumes a versioned benchmark summary contract emitted by `networker-tester`.
- Warmup, measured, retry, and failed samples are explicitly distinguishable in stored data and exported artifacts.
- Every published summary includes sample counts, uncertainty, and a stated outlier policy.
- The benchmark reporter no longer selects a winner from a single best warm result.
- The dashboard can compare at least two benchmark runs using a defined baseline and display uncertainty-aware results.
- A reviewer can inspect raw samples, environment metadata, and methodology for any published comparison.
- Comparison charts in the dashboard display uncertainty such as confidence intervals or error bars.
- A publication readiness check flags runs with high CV, insufficient samples, excessive noise, or otherwise weak comparison evidence.

---

## 14. Phased Delivery Plan

### Phase 1 - Data Contract and Summary Layer

- Add a versioned benchmark summary JSON contract
- Normalize case identifiers and sample tagging
- Fix orchestrator ingestion to use the new contract
- Persist methodology and environment metadata

### Phase 2 - Statistical Processing and Storage

- Expand summary statistics
- Add confidence intervals, fences, and outlier accounting
- Add stability-check output and CV-based data quality reporting
- Add dedicated benchmark summary and comparison persistence
- Add raw sample CSV export

### Phase 3 - Baselines and Professional Reporting

- Add explicit baseline support
- Add ratio-based comparison outputs
- Replace best-result leaderboards with multi-run aggregate comparisons
- Upgrade HTML and dashboard benchmark views

### Phase 4 - Advanced Comparison Policy

- Add equivalence thresholds
- Add optional statistical testing policy
- Add stronger data quality warnings and publication readiness checks

### Phase 5 - Advanced Diagnostics and Publication Controls

- Add pluggable diagnosers and richer runtime counters
- Add advanced packet-level benchmark summaries
- Add optional non-parametric or hypothesis-test based comparison policies
- Add formal publication-readiness and benchmark-review gates

---

## 15. Open Questions for Review

- What should be the default report confidence level for network benchmarks: 95%, 99%, or 99.9%?
- Should the default outlier policy use Tukey, MAD, or another robust method for primary summaries?
- What should count as a materially different environment for comparison gating?
- Should benchmark cases be stored inside the existing run schema first, or introduced as dedicated benchmark tables immediately?
- Which primary publication metrics should we favor by workload type in the first release?
- How should the system handle bimodal distributions, which are common in real network latency data?
- Which system-level counters should be mandatory by default and which should remain optional diagnosers?
- What should be the default minimum sample threshold for a result to be considered publishable?
- Should the default publication policy prefer bootstrap or another non-parametric confidence interval for latency workloads?
