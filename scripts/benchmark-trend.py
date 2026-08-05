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

Deliberately INFORMATIONAL — it prints, it does not fail the build. A blocking
threshold on shared-runner numbers produces false alarms, and a check that
cries wolf gets muted, which is worse than no check. The point is that a human
reading the run summary can SEE the movement.

Usage:
    benchmark-trend.py CURRENT_DIR [PREVIOUS_DIR]
"""

from __future__ import annotations

import json
import pathlib
import statistics
import sys

# Flagged as a possible regression. Wide on purpose — see the module docstring.
REGRESSION_PCT = 20.0
IMPROVEMENT_PCT = -20.0

# Below this, a median is noise and a percentage swing means nothing.
MIN_ATTEMPTS = 5


def median_latency_ms(path: pathlib.Path) -> tuple[float | None, int]:
    """Median http.total_duration_ms over SUCCESSFUL attempts, and the count."""
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
    return statistics.median(values), len(values)


def collect(directory: pathlib.Path) -> dict[str, tuple[float | None, int]]:
    if not directory or not directory.is_dir():
        return {}
    return {
        path.stem: median_latency_ms(path)
        for path in sorted(directory.glob("*.json"))
    }


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2

    current = collect(pathlib.Path(argv[1]))
    previous = collect(pathlib.Path(argv[2])) if len(argv) > 2 else {}

    if not current:
        print("No current benchmark results to compare — nothing was produced.")
        return 0

    print("| Workload | Previous p50 | Current p50 | Delta | |")
    print("|----------|-------------:|------------:|------:|--|")

    regressions: list[str] = []
    for name in sorted(current):
        cur_ms, cur_n = current[name]
        prev_ms, _ = previous.get(name, (None, 0))

        if cur_ms is None:
            print(f"| {name} | — | too few samples ({cur_n}) | — | ⚠ |")
            continue
        if prev_ms is None:
            # Say so explicitly. A blank cell reads as "no change".
            label = "no baseline" if name not in previous else "insufficient"
            print(f"| {name} | {label} | {cur_ms:.1f} ms | — | new |")
            continue

        delta = (cur_ms - prev_ms) / prev_ms * 100.0
        if delta >= REGRESSION_PCT:
            mark = "🔴"
            regressions.append(f"{name}: {prev_ms:.1f} → {cur_ms:.1f} ms ({delta:+.1f}%)")
        elif delta <= IMPROVEMENT_PCT:
            mark = "🟢"
        else:
            mark = ""
        print(f"| {name} | {prev_ms:.1f} ms | {cur_ms:.1f} ms | {delta:+.1f}% | {mark} |")

    print()
    if not previous:
        print("_No previous run to compare against — this run becomes the baseline._")
    elif regressions:
        print(f"**{len(regressions)} workload(s) slower by ≥{REGRESSION_PCT:.0f}%:**")
        for line in regressions:
            print(f"- {line}")
        print()
        print(
            "_Informational only. CI runners are shared, so a single run is weak "
            "evidence — check whether the movement persists across runs before "
            "treating it as a regression._"
        )
    else:
        print(f"_No workload slower by ≥{REGRESSION_PCT:.0f}% versus the previous run._")

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
