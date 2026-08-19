/**
 * Comparison-group pivot logic (#794).
 *
 * A finished comparison group is a bag of runs whose config names encode the
 * cell they ran as (`ComparisonGroupsEndpoints.CellConfigName`):
 *
 *   `<cell label> · cg-<8-hex group id>·<cell index>·<4-hex launch nonce>`
 *
 * with two cell-label shapes in the wild:
 *   - Application benchmark (AppBenchmarkPage):
 *       `<language> @ <cloud>/<region> @ <os> @ <proxy>`   (language optional)
 *   - Full-stack matrix (lib/matrix-cells.ts):
 *       `<cloud>/<region> <os> · <ProxyLabel>`
 *
 * Everything here is defensive: unknown label shapes degrade to null axes and
 * the page still renders the cell under its raw label. Structured metadata
 * from the group's cells (EndpointRef kind 'pending') wins over name parsing
 * when available.
 */

import type {
  BenchmarkArtifact,
  ComparisonCell,
  EndpointRef,
  LiveAttempt,
  TestRun,
} from '../../api/types';
import { computeStats, type Stats } from '../../lib/analysis';

// ── Cell metadata ─────────────────────────────────────────────────────────────

export const CELL_AXES = ['cloud', 'region', 'os', 'proxy'] as const;
export type CellAxis = (typeof CELL_AXES)[number];

export interface CellMeta {
  language: string | null;
  cloud: string | null;
  region: string | null;
  os: string | null;
  proxy: string | null;
}

const OS_VALUES = new Set(['linux', 'windows']);
const PROXY_VALUES = new Set(['nginx', 'iis', 'caddy', 'apache', 'haproxy', 'traefik']);

/**
 * Strip the ` · cg-<id>·<index>·<nonce>` suffix a comparison-group launch
 * appends to each cell's test-config name. Names without the suffix (manual
 * configs, future formats) come back unchanged.
 */
export function stripCellNameSuffix(configName: string): string {
  return configName.replace(/\s·\scg-\S+$/u, '').trim();
}

function emptyMeta(): CellMeta {
  return { language: null, cloud: null, region: null, os: null, proxy: null };
}

function parseCloudRegion(token: string, meta: CellMeta): boolean {
  if (meta.cloud !== null || !token.includes('/')) return false;
  const [cloud, ...rest] = token.split('/');
  meta.cloud = cloud || null;
  meta.region = rest.join('/') || null;
  return true;
}

/**
 * Parse a cell label into its axes. Handles both the application-benchmark
 * (` @ `-joined) and full-stack (`cloud/region os · Proxy`) shapes; anything
 * else returns all-null axes rather than guessing.
 */
export function parseCellLabel(label: string): CellMeta {
  const meta = emptyMeta();
  const trimmed = label.trim();
  if (!trimmed) return meta;

  if (trimmed.includes(' @ ')) {
    // App benchmark: `<language?> @ <cloud>/<region> @ <os> @ <proxy>`.
    // Position the language BEFORE the cloud/region part so a language that
    // shares a proxy name (the nginx static baseline) is not misread.
    const parts = trimmed.split(' @ ').map((p) => p.trim()).filter(Boolean);
    const cloudIdx = parts.findIndex((p) => p.includes('/'));
    let rest = parts;
    if (cloudIdx >= 0) {
      parseCloudRegion(parts[cloudIdx], meta);
      const language = parts.slice(0, cloudIdx).join(' @ ');
      if (language) meta.language = language;
      rest = parts.slice(cloudIdx + 1);
    }
    for (const part of rest) {
      const lower = part.toLowerCase();
      if (meta.os === null && OS_VALUES.has(lower)) {
        meta.os = lower;
      } else if (meta.proxy === null && PROXY_VALUES.has(lower)) {
        meta.proxy = lower;
      } else if (cloudIdx < 0 && meta.language === null) {
        meta.language = part;
      }
    }
    return meta;
  }

  // Full-stack: `<cloud>/<region> <os> · <ProxyLabel>`.
  const [left, ...proxyParts] = trimmed.split(' · ');
  const proxyLabel = proxyParts.join(' · ').trim().toLowerCase();
  if (proxyLabel) meta.proxy = proxyLabel;
  let sawAxis = false;
  for (const token of left.split(/\s+/)) {
    if (parseCloudRegion(token, meta)) {
      sawAxis = true;
    } else if (meta.os === null && OS_VALUES.has(token.toLowerCase())) {
      meta.os = token.toLowerCase();
      sawAxis = true;
    }
  }
  // A bare custom name ("My cell") parsed nothing — don't invent a proxy from
  // text after a stray separator either.
  if (!sawAxis && meta.proxy !== null && !PROXY_VALUES.has(meta.proxy)) {
    meta.proxy = null;
  }
  return meta;
}

/** Structured axes from a group cell's endpoint ref (wins over name parsing). */
export function cellMetaFromEndpoint(endpoint: EndpointRef | null | undefined): Partial<CellMeta> {
  if (!endpoint || endpoint.kind !== 'pending') return {};
  const out: Partial<CellMeta> = {};
  if (endpoint.region) out.region = endpoint.region;
  if (endpoint.os) out.os = endpoint.os;
  if (endpoint.proxy_stack) out.proxy = endpoint.proxy_stack;
  // App-benchmark cells carry the language on the pending ref (not yet in the
  // published EndpointRef type) — read it defensively.
  const language = (endpoint as { language?: unknown }).language;
  if (typeof language === 'string' && language) out.language = language;
  return out;
}

/** Label parsing enriched by the group cell's structured endpoint (if any). */
export function resolveCellMeta(label: string, groupCells?: ComparisonCell[]): CellMeta {
  const parsed = parseCellLabel(label);
  const cell = groupCells?.find((c) => c.label === label);
  return { ...parsed, ...cellMetaFromEndpoint(cell?.endpoint) };
}

// ── Attempt-derived stats ─────────────────────────────────────────────────────

export interface CellStats {
  /** All attempts the run reported. */
  attempts: number;
  /** Successful attempts with an HTTP total duration (chart/table samples). */
  samples: number;
  /** Percent 0–100 over all attempts; null when there are none. */
  successRate: number | null;
  ttfb: Stats | null;
  total: Stats | null;
}

/**
 * p50/p95 of http.ttfb_ms and http.total_duration_ms plus success rate,
 * computed client-side from the run's attempts (the artifact's per-case
 * summaries are preferred when present — see artifactCaseStats).
 */
export function computeCellStats(attempts: LiveAttempt[]): CellStats {
  const totals: number[] = [];
  const ttfbs: number[] = [];
  let successCount = 0;
  for (const a of attempts) {
    if (a.success) successCount += 1;
    if (!a.success || !a.http) continue;
    if (typeof a.http.total_duration_ms === 'number') totals.push(a.http.total_duration_ms);
    if (typeof a.http.ttfb_ms === 'number') ttfbs.push(a.http.ttfb_ms);
  }
  return {
    attempts: attempts.length,
    samples: totals.length,
    successRate: attempts.length > 0 ? (successCount / attempts.length) * 100 : null,
    ttfb: computeStats(ttfbs),
    total: computeStats(totals),
  };
}

// ── Cell result model ─────────────────────────────────────────────────────────

export interface CellResult {
  run: TestRun;
  /** Config name with the ` · cg-…` suffix stripped. */
  cellLabel: string;
  meta: CellMeta;
  stats: CellStats;
  artifact: BenchmarkArtifact | null;
}

export function buildCellResult(
  run: TestRun,
  attempts: LiveAttempt[],
  artifact: BenchmarkArtifact | null,
  groupCells?: ComparisonCell[],
): CellResult {
  const cellLabel = stripCellNameSuffix(run.config_name ?? run.id.slice(0, 8));
  return {
    run,
    cellLabel,
    meta: resolveCellMeta(cellLabel, groupCells),
    stats: computeCellStats(attempts),
    artifact,
  };
}

function p50OrInfinity(cell: CellResult): number {
  return cell.stats.total?.p50 ?? Number.POSITIVE_INFINITY;
}

function byP50Ascending(a: CellResult, b: CellResult): number {
  return p50OrInfinity(a) - p50OrInfinity(b);
}

// ── Pivot 1: by environment (cloud/region/os/proxy) ───────────────────────────

export function envKey(meta: CellMeta): string {
  return CELL_AXES.map((axis) => meta[axis] ?? '—').join('|');
}

export function envLabel(meta: CellMeta): string {
  const cloudRegion =
    meta.cloud && meta.region ? `${meta.cloud}/${meta.region}` : meta.cloud ?? meta.region;
  return [cloudRegion, meta.os, meta.proxy].filter(Boolean).join(' · ') || 'unlabeled environment';
}

export interface EnvSection {
  key: string;
  label: string;
  /** Sorted fastest-first by attempt-derived total p50 (no-data cells last). */
  cells: CellResult[];
}

export function groupByEnvironment(cells: CellResult[]): EnvSection[] {
  const sections = new Map<string, EnvSection>();
  for (const cell of cells) {
    const key = envKey(cell.meta);
    let section = sections.get(key);
    if (!section) {
      section = { key, label: envLabel(cell.meta), cells: [] };
      sections.set(key, section);
    }
    section.cells.push(cell);
  }
  const out = [...sections.values()];
  for (const section of out) section.cells.sort(byP50Ascending);
  out.sort((a, b) => a.label.localeCompare(b.label));
  return out;
}

// ── Pivot 2: by language across environments ──────────────────────────────────

export interface LanguageCellRow {
  cell: CellResult;
  /** Built from the axes that differ within the language's cells. */
  variantLabel: string;
  /** p50 delta vs the language's fastest cell, in percent (0 = fastest). */
  deltaPct: number | null;
}

export interface LanguageSection {
  /** '' when the cells carry no language (full-stack matrices). */
  language: string;
  /** Axes with more than one distinct value among this language's cells. */
  differingAxes: CellAxis[];
  /** Fairness flag (#794): >1 axis differs, so deltas are not causal. */
  multiVariable: boolean;
  rows: LanguageCellRow[];
}

export function groupByLanguage(cells: CellResult[]): LanguageSection[] {
  const byLang = new Map<string, CellResult[]>();
  for (const cell of cells) {
    const lang = cell.meta.language ?? '';
    const bucket = byLang.get(lang);
    if (bucket) bucket.push(cell);
    else byLang.set(lang, [cell]);
  }

  const sections: LanguageSection[] = [];
  for (const [language, group] of byLang) {
    const differingAxes = CELL_AXES.filter(
      (axis) => new Set(group.map((c) => c.meta[axis] ?? '—')).size > 1,
    );
    const sorted = [...group].sort(byP50Ascending);
    const fastestP50 = sorted.length > 0 ? sorted[0].stats.total?.p50 ?? null : null;
    const rows: LanguageCellRow[] = sorted.map((cell) => {
      const p50 = cell.stats.total?.p50 ?? null;
      const deltaPct =
        p50 !== null && fastestP50 !== null && fastestP50 > 0
          ? ((p50 - fastestP50) / fastestP50) * 100
          : null;
      const variantLabel =
        differingAxes.length > 0
          ? differingAxes.map((axis) => cell.meta[axis] ?? '—').join(' · ')
          : envLabel(cell.meta);
      return { cell, variantLabel, deltaPct };
    });
    sections.push({
      language,
      differingAxes,
      multiVariable: differingAxes.length > 1,
      rows,
    });
  }

  // Named languages alphabetically; the unlabeled bucket last.
  sections.sort((a, b) => {
    if (a.language === '') return b.language === '' ? 0 : 1;
    if (b.language === '') return -1;
    return a.language.localeCompare(b.language);
  });
  return sections;
}

// ── Per-case stats from the benchmark artifact (post-#796 depth) ──────────────

export interface CaseStat {
  caseId: string;
  label: string;
  unit: string;
  higherIsBetter: boolean;
  p50: number;
  p95: number;
  successRate: number | null;
  samples: number;
}

/**
 * Join artifact.summaries with artifact.cases. Returns [] when the artifact
 * has no per-case rows — which is every apibench comparison run until #796
 * lands (the page falls back to attempt-derived stats with a note).
 */
export function artifactCaseStats(artifact: BenchmarkArtifact | null | undefined): CaseStat[] {
  if (!artifact || !Array.isArray(artifact.cases) || artifact.cases.length === 0) return [];
  const caseById = new Map(artifact.cases.map((c) => [c.id, c]));
  const summaries = Array.isArray(artifact.summaries) ? artifact.summaries : [];
  const out: CaseStat[] = [];
  for (const s of summaries) {
    const c = caseById.get(s.case_id);
    if (!c) continue;
    const total = (s.success_count ?? 0) + (s.failure_count ?? 0);
    out.push({
      caseId: s.case_id,
      label: c.payload_bytes != null ? `${c.id} (${c.payload_bytes}b)` : c.id,
      unit: s.metric_unit || c.metric_unit || 'ms',
      higherIsBetter: s.higher_is_better ?? c.higher_is_better,
      p50: s.p50,
      p95: s.p95,
      successRate: total > 0 ? ((s.success_count ?? 0) / total) * 100 : null,
      samples: s.included_sample_count ?? s.sample_count ?? 0,
    });
  }
  return out;
}

export interface CaseMatrixColumn {
  cell: CellResult;
  byCase: Map<string, CaseStat>;
}

export interface CaseMatrix {
  /** Union of case labels, in first-appearance order. */
  caseLabels: string[];
  /** Only cells whose artifact actually has per-case rows. */
  columns: CaseMatrixColumn[];
}

export function buildCaseMatrix(cells: CellResult[]): CaseMatrix {
  const caseLabels: string[] = [];
  const seen = new Set<string>();
  const columns: CaseMatrixColumn[] = [];
  for (const cell of cells) {
    const stats = artifactCaseStats(cell.artifact);
    if (stats.length === 0) continue;
    const byCase = new Map<string, CaseStat>();
    for (const stat of stats) {
      byCase.set(stat.label, stat);
      if (!seen.has(stat.label)) {
        seen.add(stat.label);
        caseLabels.push(stat.label);
      }
    }
    columns.push({ cell, byCase });
  }
  return { caseLabels, columns };
}
