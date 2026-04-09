# Benchmarks Directory

This directory contains both benchmark **source inputs** and benchmark **generated outputs**.

## Source files you should treat as part of the repo

- benchmark configs, e.g. `sample-benchmark.json`, `open-loop-*.json`
- orchestrator source under `orchestrator/`
- metrics-agent source under `metrics-agent/`
- reference API source under `reference-apis/`
- shared helper scripts/certs under `shared/` when intentionally kept in repo

## Generated files you should NOT treat as source

These are local/runtime outputs and should not be committed:

- `results-*.json`
- `results-*.html`
- `artifacts/`
- `reference-apis/*/output/`
- built binaries such as `reference-apis/*/server`
- build output trees like `bin/`, `obj/`, `publish/`, `target/`

## Working rule

If a file exists because you **ran a benchmark, built a reference API, or generated a report**, it is usually an artifact, not source.
