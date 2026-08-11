#!/usr/bin/env python3
"""Compare a benchmark run against the previous one and print a trend table.

Audit P2: `benchmark.yml` ran the suite, said OK/EMPTY per file, uploaded an
artifact — and stopped. It told you the benchmark EXECUTED, never whether
anything got slower. A regression could ride for weeks with every run green.

Input is the tester's `--json-stdout` payload (one file per language×workload),
whose real shape is a run object with `attempts[]`, each carrying
`http.total_duration_ms`. Per-file latency here is the MEDIAN of successful
attempts: benchmarks on shared CI runners have a long right tail, and a mean
would track the worst outlier rather than the typical request.

NOISE-AWARE: each median carries a 95% confidence interval (distribution-free
order-statistic CI — binomial ranks around the median, no bootstrap or
normality assumption). A delta is only flagged when the two CIs are DISJOINT
in addition to crossing the percentage threshold: on shared runners a 25%
swing whose intervals overlap is one noisy run, not a movement, and flagging
it teaches readers to ignore the table.

Deliberately INFORMATIONAL — it prints, it does not fail the build. A blocking
threshold on shared-runner numbers produces false alarms, and a check that
cries wolf gets muted, which is worse than no check. The point is that a human
reading the run summary can SEE the movement.

Usage:
    benchmark-trend.py CURRENT_DIR [PREVIOUS_DIR]
"""

from __future__ import annotations

import json
import math
import pathlib
import statistics
import sys
from typing import NamedTuple

# Flagged as a possible regression (when the CIs are also disjoint).
# Wide on purpose — see the module docstring.
REGRESSION_PCT = 20.0
IMPROVEMENT_PCT = -20.0

# Below this, a median is noise and a percentage swing means nothing.
MIN_ATTEMPTS = 5


class MedianEstimate(NamedTuple):
    median: float
    count: int
    ci_lo: float
    ci_hi: float


def median_ci(sorted_values: list[float]) -> tuple[float, float]:
    """Distribution-free 95% CI for the median via binomial order statistics.

    Ranks n/2 ± 1.96·√n/2 (normal approximation to Binomial(n, 0.5)),
    clamped to the sample. Assumes nothing about the latency distribution —
    exactly what shared-runner tails call for.
    """
    n = len(sorted_values)
    half_width = 1.96 * math.sqrt(n) / 2.0
    lo = max(0, int(math.floor(n / 2.0 - half_width)))
    hi = min(n - 1, int(math.ceil(n / 2.0 + half_width)))
    return sorted_values[lo], sorted_values[hi]


def median_latency(path: pathlib.Path) -> tuple[MedianEstimate | None, int]:
    """Median http.total_duration_ms over SUCCESSFUL attempts, with its CI."""
    try:
        doc = json.loads(path.read_text())
    except (json.JSONDecodeError, OSError):
        return None, 0

    # A multi-target run serializes as a list; a single run as an object.
    runs = doc if isinstance(doc, list) else [doc]

    values: list[float] = []
    for run in runs:
        if not isinstance(run, dict):
            continue
        for attempt in run.get("attempts") or []:
            if not attempt.get("success"):
                continue
            http = attempt.get("http") or {}
            ms = http.get("total_duration_ms")
            if isinstance(ms, (int, float)):
                values.append(float(ms))

    if len(values) < MIN_ATTEMPTS:
        return None, len(values)
    values.sort()
    ci_lo, ci_hi = median_ci(values)
    return (
        MedianEstimate(statistics.median(values), len(values), ci_lo, ci_hi),
        len(values),
    )


def collect(directory: pathlib.Path) -> dict[str, tuple[MedianEstimate | None, int]]:
    if not directory or not directory.is_dir():
        return {}
    return {
        path.stem: median_latency(path)
        for path in sorted(directory.glob("*.json"))
    }


def fmt(est: MedianEstimate) -> str:
    return f"{est.median:.1f} ms ±[{est.ci_lo:.1f}–{est.ci_hi:.1f}]"


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2

    current = collect(pathlib.Path(argv[1]))
    previous = collect(pathlib.Path(argv[2])) if len(argv) > 2 else {}

    if not current:
        print("No current benchmark results to compare — nothing was produced.")
        return 0

    print("| Workload | Previous p50 (95% CI) | Current p50 (95% CI) | Delta | |")
    print("|----------|----------------------:|---------------------:|------:|--|")

    regressions: list[str] = []
    for name in sorted(current):
        cur, cur_n = current[name]
        prev, _ = previous.get(name, (None, 0))

        if cur is None:
            print(f"| {name} | — | too few samples ({cur_n}) | — | ⚠ |")
            continue
        if prev is None:
            # Say so explicitly. A blank cell reads as "no change".
            label = "no baseline" if name not in previous else "insufficient"
            print(f"| {name} | {label} | {fmt(cur)} | — | new |")
            continue

        delta = (cur.median - prev.median) / prev.median * 100.0
        # Disjoint CIs = the movement exceeds both runs' sampling noise.
        disjoint = cur.ci_lo > prev.ci_hi or cur.ci_hi < prev.ci_lo
        if delta >= REGRESSION_PCT and disjoint:
            mark = "🔴"
            regressions.append(
                f"{name}: {prev.median:.1f} → {cur.median:.1f} ms ({delta:+.1f}%, CIs disjoint)"
            )
        elif delta <= IMPROVEMENT_PCT and disjoint:
            mark = "🟢"
        elif abs(delta) >= REGRESSION_PCT:
            mark = "◦ within noise"
        else:
            mark = ""
        print(f"| {name} | {fmt(prev)} | {fmt(cur)} | {delta:+.1f}% | {mark} |")

    print()
    if not previous:
        print("_No previous run to compare against — this run becomes the baseline._")
    elif regressions:
        print(
            f"**{len(regressions)} workload(s) slower by ≥{REGRESSION_PCT:.0f}% "
            "with disjoint confidence intervals:**"
        )
        for line in regressions:
            print(f"- {line}")
        print()
        print(
            "_Informational only. CI runners are shared, so even a CI-separated "
            "single run is moderate evidence — check whether the movement "
            "persists across runs before treating it as a regression._"
        )
    else:
        print(
            f"_No workload slower by ≥{REGRESSION_PCT:.0f}% beyond sampling noise "
            "versus the previous run._"
        )

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
