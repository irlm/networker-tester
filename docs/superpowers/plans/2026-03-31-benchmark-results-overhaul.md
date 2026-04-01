# Benchmark Results Overhaul — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Interactive benchmark results with per-phase time breakdown, inline language comparison, and grouped leaderboard.

**Architecture:** Two new React components (HorizontalBoxWhiskerChart, PhaseBreakdown), overhaul of Results page, enhancement of Leaderboard page, one new backend API endpoint.

**Tech Stack:** React 19, TypeScript, Tailwind 4, Recharts 3 (for tooltips/axes), axum REST API, PostgreSQL.

---

### Task 1: HorizontalBoxWhiskerChart Component

**Files:**
- Create: `dashboard/src/components/charts/HorizontalBoxWhiskerChart.tsx`
- Modify: `dashboard/src/components/charts/BoxWhiskerChart.tsx` (reference existing vertical implementation)

- [ ] **Step 1: Create the component file with types**

```typescript
// HorizontalBoxWhiskerChart.tsx
export interface HBoxGroup {
  label: string;
  sublabel?: string;
  color: string;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  mean: number;
}

export interface HorizontalBoxWhiskerProps {
  groups: HBoxGroup[];
  unit: string;
  title?: string;
  onClickGroup?: (label: string) => void;
  expandedGroups?: Set<string>;
}
```

- [ ] **Step 2: Implement the SVG-based chart**

Render horizontal box-and-whisker using plain SVG (no Recharts dependency for this component):
- Y-axis: language labels sorted by p50 ascending, right-aligned at 70px width.
- X-axis: auto-scaled with grid lines. Calculate domain from min(p5) to max(p95) with 10% padding.
- Each row: 24px height, 4px gap. Whisker line from p5 to p95 (2px stroke, gray). Box from p25 to p75 (filled with color at 25% opacity, 1px border). Median line at p50 (2px, solid color). Mean text at right edge.
- Hover tooltip (CSS `title` attribute or custom tooltip div): shows all percentile values.
- Click handler: calls `onClickGroup(label)` when row is clicked. Expanded rows get a left border accent.
- Row height: 28px per language. Chart auto-sizes.

- [ ] **Step 3: Test with mock data**

Render the component in isolation with 5 mock languages to verify layout, scaling, hover, and click behavior.

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/components/charts/HorizontalBoxWhiskerChart.tsx
git commit -m "feat: horizontal box-and-whisker chart component for benchmark results"
```

---

### Task 2: PhaseBreakdown Component

**Files:**
- Create: `dashboard/src/components/benchmark/PhaseBreakdown.tsx`

- [ ] **Step 1: Create types and component skeleton**

```typescript
export interface PhaseData {
  mode: string;
  dns_ms: number | null;
  tcp_ms: number | null;
  tls_ms: number | null;
  ttfb_ms: number | null;
  transfer_ms: number | null;
  total_ms: number;
}

export interface ComparisonData {
  otherLanguage: string;
  otherColor: string;
  otherModes: PhaseData[];
}

export interface PhaseBreakdownProps {
  language: string;
  color: string;
  modes: PhaseData[];
  comparison?: ComparisonData;
}
```

- [ ] **Step 2: Implement phase bars**

For each mode, render a horizontal stacked bar:
- DNS=#3b82f6, TCP=#8b5cf6, TLS=#f59e0b, TTFB=#ef4444, Transfer=#10b981
- Each segment width proportional to its ms value relative to the max total across all modes.
- Hover on segment shows tooltip: "TLS: 1.2ms (29%)"
- Total shown at the end of the bar.

- [ ] **Step 3: Implement data table**

Table below bars: Mode | DNS | TCP | TLS | TTFB | Transfer | Total.
Monospace font, right-aligned numbers. Color the highest-value cell per row with a subtle background tint.

- [ ] **Step 4: Implement comparison deltas**

When `comparison` prop is present:
- Show a delta row between the two languages' tables.
- Per-phase percentage delta. Color: green (<0% = faster), red (>20% = slower), amber (0-20%).
- Format: "+158%" or "-12%".
- Phases where the delta is largest get bold text.

- [ ] **Step 5: Commit**

```bash
git add dashboard/src/components/benchmark/PhaseBreakdown.tsx
git commit -m "feat: phase breakdown component with stacked bars, table, and comparison deltas"
```

---

### Task 3: Overhaul Run Results Page

**Files:**
- Modify: `dashboard/src/pages/BenchmarkConfigResultsPage.tsx`
- Modify: `dashboard/src/api/types.ts` (if phase data types needed)

- [ ] **Step 1: Replace the flat table with HorizontalBoxWhiskerChart**

Remove the existing `<table>` rendering. Replace with:
```tsx
<HorizontalBoxWhiskerChart
  groups={boxGroups}
  unit="ms"
  title="Latency Distribution — click a language for phase detail"
  onClickGroup={toggleExpanded}
  expandedGroups={expanded}
/>
```

Build `boxGroups` from the existing `ConfigCellResult[]` summaries (same data, new shape).

- [ ] **Step 2: Add expand/collapse state and PhaseBreakdown rendering**

State: `const [expanded, setExpanded] = useState<Set<string>>(new Set());`

After the chart, render expanded PhaseBreakdown components:
```tsx
{Array.from(expanded).map(lang => {
  const result = activeCellResults.find(r => r.language === lang);
  if (!result) return null;
  return <PhaseBreakdown key={lang} language={lang} color={...} modes={extractPhaseData(result)} />;
})}
```

- [ ] **Step 3: Extract phase data from summaries**

Write `extractPhaseData(result: ConfigCellResult): PhaseData[]` that maps summaries to per-mode phase data. Group by protocol, extract timing from summary fields.

For summaries that have `latency_mean_ms` / `latency_p50_ms` etc — use those as TTFB. Calculate Transfer = mean - TTFB. DNS/TCP/TLS come from the stored artifact JSON if available, otherwise show as null (dash in table).

- [ ] **Step 4: Implement inline comparison**

When `expanded.size === 2`, pass `comparison` prop to both PhaseBreakdown components. Calculate deltas between the two languages' phase data.

- [ ] **Step 5: Add "hide incomplete" toggle**

```tsx
const [hideIncomplete, setHideIncomplete] = useState(true);
const visibleResults = hideIncomplete
  ? activeCellResults.filter(r => r.summaries.length > 0)
  : activeCellResults;
const hiddenCount = activeCellResults.length - visibleResults.length;
```

Render toggle: `hide incomplete (3 hidden)` link.

- [ ] **Step 6: Commit**

```bash
git add dashboard/src/pages/BenchmarkConfigResultsPage.tsx dashboard/src/api/types.ts
git commit -m "feat: overhaul benchmark results page with interactive phase breakdown"
```

---

### Task 4: Grouped Leaderboard Backend API

**Files:**
- Modify: `crates/networker-dashboard/src/db/benchmarks.rs` — add grouped leaderboard query
- Modify: `crates/networker-dashboard/src/api/benchmark_configs.rs` or leaderboard routes — add endpoint

- [ ] **Step 1: Add DB query for grouped leaderboard**

```rust
pub async fn get_grouped_leaderboard(
    client: &Client,
    group: Option<&str>,  // "azure-eastus-loopback" or None for all
) -> anyhow::Result<GroupedLeaderboard> {
    // Parse group into cloud/region/topology
    // Query: SELECT language, COUNT(DISTINCT config_id) as run_count,
    //   percentile_cont(0.05) WITHIN GROUP (ORDER BY mean) as p5,
    //   percentile_cont(0.25) ... as p25, ...
    // FROM benchmarksummary bs
    // JOIN benchmark_run br ON br.run_id = bs.benchmarkrunid
    // JOIN benchmark_config bc ON bc.config_id = br.config_id
    // JOIN benchmark_cell bcell ON bcell.config_id = bc.config_id
    // WHERE bcell.cloud = $1 AND bcell.region = $2 AND bcell.topology = $3
    // GROUP BY language
    // ORDER BY percentile_cont(0.5) ... ASC
}
```

- [ ] **Step 2: Add API endpoint**

`GET /api/leaderboard/grouped?group=azure-eastus-loopback`

Returns `{ groups: string[], selected: string, languages: [...] }`

- [ ] **Step 3: Compile and test**

Run: `cargo check -p networker-dashboard`

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/db/benchmarks.rs crates/networker-dashboard/src/api/
git commit -m "feat: grouped leaderboard API endpoint with fingerprint filtering"
```

---

### Task 5: Enhance Leaderboard Page

**Files:**
- Modify: `dashboard/src/pages/LeaderboardPage.tsx`
- Modify: `dashboard/src/api/client.ts` — add grouped leaderboard method
- Modify: `dashboard/src/api/types.ts` — add types

- [ ] **Step 1: Add API client method and types**

```typescript
// types.ts
export interface GroupedLeaderboard {
  groups: string[];
  selected: string;
  languages: Array<{
    language: string;
    run_count: number;
    p5: number; p25: number; p50: number; p75: number; p95: number;
    mean: number; rps: number;
  }>;
}

// client.ts
async getGroupedLeaderboard(group?: string): Promise<GroupedLeaderboard>
```

- [ ] **Step 2: Replace medal table with HorizontalBoxWhiskerChart**

Add fingerprint group dropdown at top. Render chart with sublabels showing run count. Keep the data table below the chart (same pattern: bars for shape, table for numbers).

- [ ] **Step 3: Add "All" group with mixed-conditions warning**

When "All" is selected, show: "Mixed network conditions — use for general trends only" warning banner above the chart.

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/pages/LeaderboardPage.tsx dashboard/src/api/client.ts dashboard/src/api/types.ts
git commit -m "feat: leaderboard with grouped horizontal box-whisker chart"
```

---

### Task 6: Deploy Updated Tester with --benchmark-mode

**Files:**
- No code changes (already exists in source)
- Build and deploy binary

- [ ] **Step 1: Build tester for Linux x86_64**

```bash
cd /Users/irlm/Projects/Rust/Network
docker run --rm --platform linux/amd64 -v "$(pwd)":/src -w /src rust:latest \
  cargo build --release -p networker-tester
```

- [ ] **Step 2: Deploy to dashboard VM**

```bash
scp target/release/networker-tester azureuser@20.42.8.158:/tmp/
ssh azureuser@20.42.8.158 "sudo cp /tmp/networker-tester /usr/local/bin/networker-tester && sudo chmod +x /usr/local/bin/networker-tester"
```

- [ ] **Step 3: Update orchestrator to pass --benchmark-mode**

Add `"--benchmark-mode".to_string()` back to the args in `executor.rs` now that the deployed tester supports it.

- [ ] **Step 4: Rebuild and deploy orchestrator**

- [ ] **Step 5: Run a Quick Check benchmark to verify full pipeline tables populate**

- [ ] **Step 6: Commit orchestrator change**

```bash
git add benchmarks/orchestrator/src/executor.rs
git commit -m "feat: enable --benchmark-mode for full artifact output"
```
