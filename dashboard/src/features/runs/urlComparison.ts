// Fair comparison across the URLs of a set run (#782 P2/P3 slice): the URLs
// were probed in the SAME run on the SAME runner — every row is a controlled
// same-tick experiment, so a side-by-side of per-phase medians is honest
// without any shared-bucket machinery. Pure functions; rendered by
// RunDetailPage's UrlComparisonTable.

import type { LiveAttempt } from '../../api/types';

export interface UrlComparisonColumn {
  url: string;
  /** Display label — scheme + trailing slash stripped. */
  label: string;
  attempts: number;
}

export interface UrlComparisonRow {
  key: string;
  label: string;
  unit: 'ms' | '%';
  higherIsBetter: boolean;
  /** Parallel to columns; null = no data for that URL. */
  values: (number | null)[];
  /** Winning column, only when the winner is unique among >= 2 non-null values. */
  bestIndex: number | null;
}

export interface UrlComparison {
  columns: UrlComparisonColumn[];
  rows: UrlComparisonRow[];
}

function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

export function shortUrlLabel(url: string): string {
  return url.replace(/^https?:\/\//, '').replace(/\/$/, '');
}

interface MetricDef {
  key: string;
  label: string;
  pick: (a: LiveAttempt) => number | null | undefined;
}

// Per-phase timings, in connection order. Medians over SUCCESSFUL attempts
// only — phase rows from failed attempts (e.g. a TLS handshake that timed
// out at the cap) would skew a latency comparison.
const PHASE_METRICS: MetricDef[] = [
  { key: 'dns', label: 'dns lookup', pick: a => a.dns?.duration_ms },
  { key: 'tcp', label: 'tcp connect', pick: a => a.tcp?.connect_duration_ms },
  { key: 'tls', label: 'tls handshake', pick: a => a.tls?.handshake_duration_ms },
  { key: 'ttfb', label: 'ttfb', pick: a => a.http?.ttfb_ms },
  { key: 'total', label: 'http total', pick: a => a.http?.total_duration_ms },
];

function bestIndexOf(values: (number | null)[], higherIsBetter: boolean): number | null {
  const present = values
    .map((v, i) => ({ v, i }))
    .filter((e): e is { v: number; i: number } => e.v != null);
  if (present.length < 2) return null;
  const best = present.reduce((acc, e) =>
    (higherIsBetter ? e.v > acc.v : e.v < acc.v) ? e : acc,
  );
  // A tied first place is nobody's win.
  const ties = present.filter(e => e.v === best.v);
  return ties.length === 1 ? best.i : null;
}

/**
 * Build the side-by-side comparison from attempts grouped by target_url
 * (groupByTargetUrl output). Returns null unless the run probed >= 2
 * attributed URLs — single-URL runs and pre-#782 testers have nothing to
 * compare.
 */
export function buildUrlComparison(groups: Record<string, LiveAttempt[]>): UrlComparison | null {
  const urls = Object.keys(groups)
    .filter(u => u !== '')
    .sort((a, b) => a.localeCompare(b));
  if (urls.length < 2) return null;

  const columns: UrlComparisonColumn[] = urls.map(url => ({
    url,
    label: shortUrlLabel(url),
    attempts: groups[url].length,
  }));

  const rows: UrlComparisonRow[] = [];

  const successValues = urls.map(url => {
    const attempts = groups[url];
    if (attempts.length === 0) return null;
    return (attempts.filter(a => a.success).length / attempts.length) * 100;
  });
  rows.push({
    key: 'success',
    label: 'success rate',
    unit: '%',
    higherIsBetter: true,
    values: successValues,
    bestIndex: bestIndexOf(successValues, true),
  });

  for (const metric of PHASE_METRICS) {
    const values = urls.map(url => {
      const samples = groups[url]
        .filter(a => a.success)
        .map(metric.pick)
        .filter((v): v is number => v != null);
      return median(samples);
    });
    if (values.every(v => v == null)) continue;
    rows.push({
      key: metric.key,
      label: metric.label,
      unit: 'ms',
      higherIsBetter: false,
      values,
      bestIndex: bestIndexOf(values, false),
    });
  }

  return { columns, rows };
}

export function formatComparisonValue(value: number | null, unit: 'ms' | '%'): string {
  if (value == null) return '-';
  if (unit === '%') return `${Math.round(value)}%`;
  if (value < 1000) return `${value.toFixed(1)}ms`;
  return `${(value / 1000).toFixed(2)}s`;
}
