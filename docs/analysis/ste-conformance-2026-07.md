# ASD-STE100 Conformance — Operational Docs (2026-07-27)

Applied ASD-STE100 (Simplified Technical English) to the user-facing
**operational** documentation, with an objective before/after conformance score.

## Result

| | Score (0–100) |
|---|---|
| **Before** | **70.3** |
| **After** | **85.1** |
| **Δ** | **+14.8** (sentence-count weighted across 13 files) |

Every rewritten file improved. Full per-file table below.

## Scope — what was rewritten, and what was deliberately left

STE is a controlled language built for **procedural/operational** technical
prose (it was created for aircraft maintenance manuals). It helps most exactly
there, and it *hurts* content whose value is nuance.

**In scope (rewritten, 13 files):** the operational docs — `README.md`,
`docs/installation.md`, `docs/deploy-config.md`, `docs/config-examples.md`,
`docs/probes.md`, `docs/release-flow.md`, `docs/testing.md`,
`docs/sdk/README.md`, `docs/ops-nginx-ws-redaction.md`, and the four
`docs/runbooks/*`.

**Deliberately out of scope (unchanged):**
- **Rust code comments (`//`, `///`, ~2,481 doc-comments).** STE's ~900-word
  dictionary and one-instruction-per-sentence rules flatten the measurement,
  statistical, and RFC-conformance nuance these carry (e.g. "the min-delay
  sample carries the tightest ±delay/2 bound"). Rewriting them loses meaning
  for no operational gain.
- **Architecture / design docs** (`docs/architecture.md`, migration plans) and
  **`docs/analysis/*`** — conceptual reasoning, not procedures.
- **`CHANGELOG.md`** — a historical record, not instructions.

Only the **prose** changed in the rewritten files: every command, flag, code
block, table, file path, and link is byte-identical (verified — headings, table
rows, and link counts are unchanged per file; nothing inside a fence or backtick
was touched).

## Methodology

Absolute STE conformance needs the licensed ASD dictionary checker; this uses
**objective, reproducible sub-metrics** computed identically on the before and
after text, so the **delta** is meaningful even though the absolute number is an
approximation. For each file the scorer strips code fences, tables, inline-code
spans, and link syntax, then splits the remaining prose into sentences and
measures:

| Sub-metric | STE rule | Weight |
|---|---|---|
| Mean sentence length (words) | ≤20 procedures / ≤25 descriptions | 30% |
| % sentences ≤20 words | short-sentence rule | 25% |
| Passive-voice rate | active voice required | 25% |
| Gerund-led-sentence rate | no `-ing` main verb | 20% |

Composite = weighted blend, each sub-score mapped to 0–100 (sentence-length full
marks ≤14 words, zero ≥34). Overall score is weighted by each file's sentence
count.

## Per-file before → after

| File | Before | After | Δ | mean len | ≤20% | passive% |
|---|---|---|---|---|---|---|
| README.md | 55.4 | 82.1 | +26.6 | 32.2→19.8 | 44→67 | 12→4 |
| docs/config-examples.md | 67.5 | 81.5 | +14.0 | 27.3→20.8 | 50→67 | 0→0 |
| docs/deploy-config.md | 72.3 | 74.8 | +2.5 | 21.9→21.5 | 58→60 | 21→16 |
| docs/installation.md | 48.8 | 66.6 | +17.7 | 37.7→27.9 | 31→56 | 15→6 |
| docs/ops-nginx-ws-redaction.md | 69.8 | 96.3 | +26.5 | 23.4→11.3 | 59→92 | 24→5 |
| docs/probes.md | 76.5 | 87.7 | +11.2 | 22.3→17.9 | 60→76 | 5→2 |
| docs/release-flow.md | 56.4 | 96.4 | +40.0 | 31.7→13.8 | 44→86 | 12→0 |
| docs/runbooks/admin-password-reset.md | 74.0 | 98.9 | +24.9 | 20.7→11.0 | 64→96 | 27→0 |
| docs/runbooks/credential-rotation.md | 90.6 | 96.9 | +6.3 | 15.1→10.1 | 83→97 | 6→3 |
| docs/runbooks/observability.md | 57.8 | 96.2 | +38.4 | 25.4→12.4 | 22→85 | 22→0 |
| docs/runbooks/perf-log-diagnosis.md | 71.4 | 96.2 | +24.7 | 21.1→12.5 | 43→85 | 14→0 |
| docs/sdk/README.md | 58.7 | 95.4 | +36.7 | 31.5→14.9 | 54→90 | 8→0 |
| docs/testing.md | 69.6 | 88.6 | +19.1 | 26.4→19.1 | 75→92 | 12→0 |
| **Overall** | **70.3** | **85.1** | **+14.8** | | | |

The runbooks and release-flow gained the most — they were the most
procedural, so STE fit them best. `docs/deploy-config.md` gained least: it was
already terse and table-heavy (121 table lines), so little prose was eligible.

## Sample before → after

Numbered procedure (deploy-config.md) — STE adds the command verb and articles,
one instruction per line, and keeps every identifier:

> **Before:** `1. Validate — JSON syntax, required fields, valid modes`
> **After:**  `1. Validate — check the JSON syntax, the required fields, and the valid modes.`

## Limitations (honest)

- Sub-metrics are computed by rule heuristics, not the licensed ASD-STE100
  dictionary/grammar checker — **approved-word conformance is not measured**,
  so the absolute score is a proxy. The before/after **delta** uses the same
  rules on both sides and is the reliable signal.
- Passive-voice and gerund detection are regex heuristics (miss some, flag some
  false positives) — applied identically to both versions.
- Inline code spans are tokenized to a placeholder before sentence-splitting so
  identifiers don't distort word counts.

*Scorer: `scripts`-free inline Python over the git-committed (before) vs the
rewritten (after) files; rubric above. Prod at v0.28.88; docs-only change.*
