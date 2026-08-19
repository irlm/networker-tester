# Benchmark Regression Detection

How the control plane decides a benchmark got slower, why an empty
Regressions page can mean two very different things, and how to get the
pipeline producing comparisons (#810).

## The pipeline

When a benchmark run completes with an artifact, `BenchmarkRegressionDetector`
compares each case's summary against the **same case in a baseline run** and
persists breaches (`benchmark_regression`, V047):

- p50 latency more than 10% worse, or success rate below 99% → flagged
  (`RegressionAnalyzer`); cases with fewer than 10 included samples on either
  side are skipped so noise-level runs are never flagged.
- Baseline resolution: the config's pinned `baseline_run_id` when set;
  otherwise the **previous completed run of the same config that has an
  artifact**. First run of a config → no baseline → nothing to compare.

## Getting comparisons: schedules are the intended driver

A comparison needs the *same config* to complete at least twice:

- **Schedules** (`/projects/{id}/schedules`) re-run a config on a cron
  expression — run #2 onward is compared automatically. This is the intended
  repeat vehicle for regression tracking.
- **Matrix / comparison-group launches do not accumulate baselines**: each
  launch creates fresh per-cell configs (by design, per-run names), so every
  cell config has exactly one run, forever. Schedule a config, relaunch the
  same config manually, or pin a baseline instead.
- **Pin as baseline**: on a completed benchmark run's detail page, an
  operator can *Pin as baseline* — every future run of that config is then
  compared against the pinned run (`POST/DELETE
  /api/v2/test-runs/{id}/pin-baseline`; only completed artifact-bearing runs
  are accepted, because those are the only runs the detector can read).
  Unpinning falls back to previous-run baselines.

Quick verification: schedule any completed benchmark config once — its second
run produces a comparison (an empty Regressions page with a non-zero
"runs compared" count is the healthy outcome).

## Comparison-activity summary (observability)

`GET /api/projects/{projectId}/benchmark-regressions/summary` lets the
Regressions page distinguish "no baselines exist yet — detection has never
compared anything" from "N comparisons ran · 0 regressions":

| Field | Semantics |
|---|---|
| `runs_compared` | Completed artifact-bearing runs preceded by another completed artifact-bearing run of the same config — the detector's own eligibility gate, **derived from run history, not a stored counter**. Exact for the previous-run pathway; approximate for pinned baselines; includes runs whose comparison predates the current detector (pre-v0.28.239 artifacts were stubs, so those comparisons were vacuous). |
| `comparable_configs` | Configs whose *next* completed run will be compared: ≥2 completed artifact-bearing runs, or a pinned baseline plus ≥1 such run. |
| `last_comparison_at` | Finish time of the newest run counted in `runs_compared`. |
| `pinned_baseline_configs` | Configs with a pinned `baseline_run_id`. |
| `total_regressions`, `last_regression_at` | Actual persisted breach rows — exact. |
| `semantics` | The derivation statement above, on the wire, so consumers never mistake derived counts for counters. |
