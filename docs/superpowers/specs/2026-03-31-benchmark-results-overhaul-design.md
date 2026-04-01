# Benchmark Results Overhaul — Design Spec

## Goal

Replace the current flat results table with an interactive, data-dense benchmark results experience that lets users see exactly where time is spent (DNS, TCP, TLS, TTFB, Transfer), compare two languages side-by-side within a run, and track language performance trends across runs via a grouped leaderboard.

## Architecture

Three page changes + two new shared components + one new backend endpoint. The deployed tester needs `--benchmark-mode` to produce per-phase timing in the BenchmarkArtifact format.

## Data Contract and Derivation Rules

### Source tables

| Table | Purpose | Key columns |
|-------|---------|-------------|
| `benchmark_run` (lowercase) | Lightweight per-language run linked to config/cell | `run_id`, `config_id`, `cell_id`, `config` (JSONB — full artifact or legacy JSON) |
| `benchmarksummary` (pipeline) | Per-case aggregate stats | `benchmarkrunid`, `caseid`, `protocol`, `mean`, `p5`–`p99`, `rps`, `latencymeanms`, `latencyp50ms`, `summaryjson` |
| `benchmarksample` (pipeline) | Per-request raw samples | `benchmarkrunid`, `caseid`, `metricvalue`, `totaldurationms`, `ttfbms` |
| `benchmark_cell` | Cell metadata | `cell_id`, `config_id`, `cloud`, `region`, `topology` |

### Field derivation for Results Page (box-and-whisker)

Every displayed field and its exact source:

| Field | Source | Formula |
|-------|--------|---------|
| **p5, p25, p50, p75, p95** | `benchmarksummary.summaryjson` | Direct read from the summary's percentile fields. One summary row per case (protocol+payload). For the language-level chart, use the **primary case** (first case with `metric_name = "latency"` and no payload). |
| **mean** | `benchmarksummary.mean` | Direct read from primary case summary. |
| **rps** | `benchmarksummary.rps` | Sum across all cases for that language/run. |

### Field derivation for Phase Breakdown (expanded row)

| Field | Source | Formula |
|-------|--------|---------|
| **total_ms** | `benchmarksummary.mean` per case | Mean latency for that protocol. |
| **ttfb_ms** | `benchmarksummary.latencymeanms` | If present, use directly. Otherwise derive: mean of `benchmarksample.ttfbms` WHERE caseid matches and `inclusionstatus = 'included'`. |
| **transfer_ms** | Derived | `total_ms - ttfb_ms`. If ttfb_ms is null, transfer_ms is null. |
| **dns_ms, tcp_ms, tls_ms** | `benchmark_run.config` (artifact JSON) | Parse the stored artifact JSON on the frontend. Extract from `samples[].phases` if the tester produced phase-level timing (requires `--benchmark-mode`). If phases are absent (legacy format), these are `null` — shown as "—" in the table. |

### Client-side artifact parsing rationale

Phase-level timing (DNS/TCP/TLS) is only available in the full BenchmarkArtifact JSON stored in `benchmark_run.config`. We parse this on the frontend because:

1. **No new DB columns needed** — avoids a migration for data that already exists in JSON.
2. **Only parsed on expand** — lazy extraction, not on page load. Each artifact is ~5–50KB.
3. **Acceptable now** because we expand at most 2 languages at a time.

**Limitations:** If artifact JSON exceeds ~500KB (unlikely with current runs), the API response for `get_config_results` becomes heavy. **When to move server-side:** If we add cross-run phase-level aggregation (e.g., "average DNS time for Rust across 50 runs"), the backend should extract and store phase means in dedicated columns. Not needed for this iteration.

### Field derivation for Leaderboard (grouped chart)

| Field | Source | Formula |
|-------|--------|---------|
| **p5, p25, p50, p75, p95** | PostgreSQL `percentile_cont` | Aggregate across runs: `percentile_cont(0.50) WITHIN GROUP (ORDER BY bs.mean)` where `bs` = benchmarksummary rows for the primary case of each run. Each run contributes one mean value per language. The percentile distribution represents **variation across runs**, not within a single run. |
| **mean** | `AVG(bs.mean)` | Average of per-run means. |
| **rps** | `AVG(bs.rps)` | Average of per-run RPS. |
| **run_count** | `COUNT(DISTINCT br.run_id)` | Number of runs contributing to this language's stats. |
| **group filter** | `benchmark_cell` join | WHERE `cloud = $1 AND region = $2 AND topology = $3`. |

**Statistical guarantee:** The box-and-whisker represents the distribution of per-run mean latencies, not a merged sample distribution. With 5 runs, each box plot point is a percentile of 5 values. With <3 runs, show the raw values as dots instead of a box. This is an approximation suitable for trend visualization, not publication-grade statistics.

### Comparison delta formula

For each phase between language A (baseline, faster) and language B:

```
delta_percent = ((B.phase_ms - A.phase_ms) / A.phase_ms) * 100
```

- If `A.phase_ms` is 0 or null: delta is `null`, shown as "—".
- Baseline = the language with lower total_ms for that mode.
- Color: green if delta < 0% (B is faster), amber if 0–20%, red if >20%.

## Pages

### 1. Run Results Page (overhaul)

**Route:** `/projects/:pid/benchmark-configs/:configId/results`

**Current state:** Flat table with language/mean/p50/p95/p99/stddev/rps columns. No phase breakdown, no interaction.

**New design:**

- **Horizontal box-and-whisker chart** at the top. Languages on Y-axis sorted by p50 ascending (fastest first). Latency on X-axis. Box = p25–p75, whisker = p5–p95, median line = p50, mean value shown to the right of each bar.
- **Click a language row** to expand it inline, revealing:
  - Per-mode phase bars (http1, http2, http3, download, upload) — stacked horizontal bars colored by phase (DNS=blue, TCP=purple, TLS=amber, TTFB=red, Transfer=green). Hover shows exact ms values. Total shown at the end.
  - Data table below the bars with columns: Mode | DNS | TCP | TLS | TTFB | Transfer | Total (ms).
- **Two languages expanded = inline comparison.** When a second language is expanded, a comparison row appears between them showing per-phase delta percentages. Phases where the slower language loses the most time are highlighted red. Small deltas are amber. DNS/TCP (network, not language-dependent) are dimmed.
- **Third expansion:** Clicking a third language collapses the oldest expanded one (FIFO). Maximum 2 expanded at a time to keep comparison meaningful.
- **"Hide incomplete" toggle** (default: on) — filters out runs that lack per-phase data (legacy format). Shows a count: "3 hidden (no phase data)" if any are hidden. If all runs are incomplete, toggle defaults to off and shows a banner: "Phase breakdown requires updated tester. Showing aggregate metrics only."
- **Cell tabs** preserved — multi-cell benchmarks show one tab per cell, each with its own chart.

**Data source:** `GET /projects/:pid/benchmark-configs/:configId/results` — returns `ConfigCellResult[]` with `summaries: BenchmarkSummary[]`. The API response also includes the artifact JSON in `benchmark_run.config` for phase extraction.

### 2. Leaderboard Page (new chart)

**Route:** `/leaderboard` (existing page, enhanced)

**Current state:** Simple table with medal emojis.

**New design:**

- **Horizontal box-and-whisker chart** (same component as results page) showing all languages aggregated across multiple runs.
- **Fingerprint grouping** — dropdown to filter by group: `Azure / eastus / loopback`, `Azure / northeurope / loopback`, `All (mixed)`. Only runs with matching cloud + region + topology are aggregated within a group.
- Each language bar shows run count below the label (e.g., "8 runs").
- **"All" group** shows everything with a warning banner: "Mixed network conditions — results are not directly comparable. Use for general trends only." Warning appears only when "All" is selected.
- Data table below the chart with language / mean / p50 / p95 / p99 / rps / run count.
- **<3 runs for a language:** Show individual dots instead of a box. Tooltip: "Not enough data for distribution (N runs)."

**Data source:** New API endpoint `GET /api/leaderboard/grouped?group=azure-eastus-loopback`.

### 3. Horizontal BoxWhiskerChart Component (new)

**Path:** `dashboard/src/components/charts/HorizontalBoxWhiskerChart.tsx`

Shared component used by both pages. Props:

```typescript
interface HorizontalBoxWhiskerProps {
  groups: Array<{
    label: string;        // language name
    sublabel?: string;    // e.g., "8 runs"
    color: string;        // language color
    p5: number;
    p25: number;
    p50: number;          // median line
    p75: number;
    p95: number;
    mean: number;         // shown as text to the right
  }>;
  unit: string;           // "ms"
  title?: string;
  onClickGroup?: (label: string) => void;
  expandedGroups?: Set<string>;
}
```

Renders:
- Y-axis: language labels, sorted by p50 ascending (fastest first).
- X-axis: auto-scaled latency. Domain: `[0, max(p95) * 1.1]`. Grid lines at power-of-10 intervals for values >100, or at 1/2/5 intervals for small values.
- Each row: 28px height. Whisker line (p5–p95, 2px gray stroke), box (p25–p75, filled with color at 25% opacity, 1px border), median vertical line (p50, 2px solid color), mean value text right-aligned.
- Hover tooltip: "Rust: p5=2.1 p25=2.8 p50=3.1 p75=3.6 p95=4.8 mean=3.2ms".
- Click handler for expand/collapse on results page. Expanded rows get a left border accent.

### 4. PhaseBreakdown Component (new)

**Path:** `dashboard/src/components/benchmark/PhaseBreakdown.tsx`

Props:

```typescript
interface PhaseData {
  mode: string;           // "http1", "http2", "download 64k"
  dns_ms: number | null;
  tcp_ms: number | null;
  tls_ms: number | null;
  ttfb_ms: number | null;
  transfer_ms: number | null;
  total_ms: number;
}

interface ComparisonData {
  otherLanguage: string;
  otherColor: string;
  otherModes: PhaseData[];
}

interface PhaseBreakdownProps {
  language: string;
  color: string;
  modes: PhaseData[];
  comparison?: ComparisonData;
}
```

Renders:
- Stacked horizontal phase bars per mode. Colors: DNS=#3b82f6, TCP=#8b5cf6, TLS=#f59e0b, TTFB=#ef4444, Transfer=#10b981.
- Segment width proportional to ms value. Max bar width = 100% of available space, scaled by the largest total across all modes in the breakdown.
- **Null phases:** Segment is omitted (no gap). If DNS/TCP/TLS are all null (legacy data), the bar shows only TTFB + Transfer (or just total as a single gray bar).
- **Zero values:** Segment rendered with 1px minimum width and "0.0ms" in tooltip.
- Hover on each segment: tooltip "TLS: 1.2ms (29% of total)".
- Data table below with all numbers. Null values shown as "—". Zero shown as "0.0".
- When `comparison` is present: delta row between the two, color-coded per the delta formula above.
- **Mixed protocol availability:** If language A has http3 data but language B doesn't, the comparison row for http3 shows "—" for B and no delta.

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Third language expanded | Collapse the first-expanded language (FIFO). Max 2 expanded. |
| Missing phases in a mode | Bar shows available phases only. Null phases = "—" in table. No gap in bar. |
| Only one language has phase data | Expand works normally. Comparison is not offered (delta row hidden). |
| Zero samples for a mode | Mode row hidden entirely (no bar, no table row). |
| All runs incomplete (legacy) | Toggle defaults to off. Banner: "Phase breakdown requires updated tester." Chart shows aggregate-only with no expand capability. |
| <3 runs in leaderboard group | Show dots instead of box. Tooltip explains insufficient data. |
| Language appears in some runs but not others | Shown in chart with available data. Sublabel shows actual run count. |

## Leaderboard Grouping API

**Endpoint:** `GET /api/leaderboard/grouped`

**Query params:** `group` (optional — `azure-eastus-loopback` format, or omit for all).

**Response:**
```json
{
  "groups": ["azure-eastus-loopback", "azure-northeurope-loopback"],
  "selected": "azure-eastus-loopback",
  "languages": [
    {
      "language": "rust",
      "run_count": 8,
      "p5": 2.1, "p25": 2.8, "p50": 3.1, "p75": 3.6, "p95": 4.8,
      "mean": 3.2, "rps": 12500
    }
  ]
}
```

**Backend implementation:** Uses `percentile_cont` on per-run mean values (one value per run per language). This is a percentile of means, not a merged sample distribution. Appropriate for showing run-to-run variation.

## Files to Create

- `dashboard/src/components/charts/HorizontalBoxWhiskerChart.tsx`
- `dashboard/src/components/benchmark/PhaseBreakdown.tsx`

## Files to Modify

- `dashboard/src/pages/BenchmarkConfigResultsPage.tsx` — full overhaul
- `dashboard/src/pages/LeaderboardPage.tsx` — add grouped chart
- `dashboard/src/api/client.ts` — add grouped leaderboard endpoint
- `dashboard/src/api/types.ts` — add grouped leaderboard types + phase data types
- `crates/networker-dashboard/src/db/benchmarks.rs` — add grouped leaderboard query
- `crates/networker-dashboard/src/api/benchmark_configs.rs` or new file — add grouped leaderboard API route

## Acceptance Criteria

- [ ] Results page: languages sorted by p50 ascending (fastest first)
- [ ] Results page: clicking a language expands inline with phase bars + data table
- [ ] Results page: expanding 2 languages shows comparison delta row between them
- [ ] Results page: expanding a 3rd language collapses the oldest (FIFO, max 2)
- [ ] Phase bars: hover tooltip shows "Phase: X.Xms (Y% of total)"
- [ ] Phase bars: null phases omitted from bar, shown as "—" in table
- [ ] Comparison delta: formula `((B - A) / A) * 100`, green <0%, amber 0-20%, red >20%
- [ ] Comparison delta: null when either value is 0 or null, shown as "—"
- [ ] "Hide incomplete" toggle: default on. Shows count of hidden runs.
- [ ] "Hide incomplete": if ALL runs incomplete, toggle defaults to off with banner
- [ ] Leaderboard: horizontal box-whisker grouped by fingerprint (cloud+region+topology)
- [ ] Leaderboard: dropdown to switch groups. "All" shows warning banner.
- [ ] Leaderboard: <3 runs shows dots instead of box with tooltip explanation
- [ ] Leaderboard: run count shown as sublabel per language
- [ ] Both charts: consistent horizontal layout, Y=language, X=latency
- [ ] Both charts: data table below chart with exact numbers

## Additional Clarifications and Guarantees

### Primary Case Selection (Deterministic Rule)

The "primary case" used for language-level aggregation MUST be selected deterministically. The same logic MUST be used for both Results Page and Leaderboard.

**Selection priority:**
1. Case where `metric_name = "latency"` AND `payload_size IS NULL` AND `protocol = "http1"`
2. Fallback: `protocol = "http2"`, then `protocol = "http3"`
3. If multiple candidates: select the one with the lowest `caseid`
4. If no valid case exists: exclude the language/run from the chart. Log warning: `"No primary latency case found for run_id=X"`

### Phase Breakdown Consistency Guarantee

Phase breakdown combines multiple sources:
- `total_ms` → `benchmarksummary.mean`
- `ttfb_ms` → `latencymeanms` OR derived from samples
- `dns/tcp/tls` → artifact JSON (`benchmark_run.config`)

**The sum of phases is NOT guaranteed to equal `total_ms`.** Reasons: different aggregation pipelines, rounding differences, sample filtering differences.

**UI rule:** `total_ms` is authoritative. Phases are a visual decomposition only.

### Artifact JSON Schema (Phase Extraction)

Expected structure in the artifact:
```json
{
  "samples": [
    {
      "phases": {
        "dns_ms": 0.3,
        "tcp_ms": 0.5,
        "tls_ms": 1.2,
        "ttfb_ms": 0.8,
        "transfer_ms": 1.4
      }
    }
  ]
}
```

**Extraction rules:**
- Ignore samples without `"phases"` key
- Compute mean per phase: `phase_mean = average(samples[].phases.phase_ms)`
- Include only samples with `inclusion_status = "included"` (if field present)
- If no valid samples: `phase = null`

### Artifact Parsing Performance

- Parsing occurs only when expanding a language (lazy)
- Maximum expanded languages = 2
- Artifact size ~5–50KB
- **Requirement:** Parsed results MUST be cached (memoized) by `run_id`. Re-expansion MUST NOT re-parse JSON.

### Leaderboard Aggregation Semantics

Percentiles are computed over **per-run mean values** (one value per run per language).

Example: Rust run means = [3.1, 3.3, 2.9, 3.0] → p50 = percentile of those 4 values.

**Properties:**
- Each run contributes exactly one value
- All runs are equally weighted
- Avoids bias toward longer runs
- May underrepresent variability in short runs

**Interpretation:** Shows run-to-run variation, NOT a merged latency distribution.

### Comparison Delta Definition

**Formula:** `delta_percent = ((B - A) / A) * 100`

Where A = faster language (baseline), B = slower language.

**Edge cases:**
- If `A = 0` → delta = null
- If A or B is null → delta = null

**Color rules:** Green → delta < 0%. Amber → 0% to 20%. Red → > 20%.

**Baseline rule:** Determined per mode by total_ms (not by expansion order).

### Handling Partial / Missing Data

**Mixed protocol availability:** If one language lacks a mode, show available data for the other. Missing side = "—", no delta computed.

**Phase null handling:**
- Null phase → omitted from bar segment
- Null in table → "—"
- If DNS/TCP/TLS all null → show TTFB + Transfer only
- If all phases null → show single gray total bar

### Numerical Display Rules

| Value | Display |
|-------|---------|
| Null | "—" |
| Zero | "0.0" |
| Tooltip precision | 1 decimal place |
| Percent deltas | Rounded to integer |

### Chart Rendering Constraints

- X-axis domain: `[0, max(p95) * 1.1]`
- Row height: 28px
- Minimum segment width: 1px (prevents invisible zero-width segments)
- Grid lines: power-of-10 intervals for values >100ms, 1/2/5 intervals for smaller values

### Consistency Requirements

The following MUST be identical across Results Page and Leaderboard:
- Primary case selection logic
- Units (ms)
- Percentile definitions (p5/p25/p50/p75/p95)
- Sorting order (p50 ascending, fastest first)

### Known Limitations

- No cross-run phase aggregation (would require server-side extraction)
- Requires `--benchmark-mode` for DNS/TCP/TLS phase data
- Legacy runs lack phase timing — only total/TTFB available
- Leaderboard percentiles are approximations (percentile-of-means, not merged distributions)

## Out of Scope

- Auto-provisioning VM improvements
- Tester `--benchmark-mode` deployment (separate task in plan)
- Regression detection integration with new charts
- Export/download of comparison data
- Cross-run phase-level aggregation (would need server-side extraction)
- Server-side phase extraction into dedicated columns
- Weighted percentiles
- Regression detection overlays
