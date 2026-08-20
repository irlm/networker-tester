// Fair comparison across the URLs of a set run (#782 P2/P3 slice): the URLs
// were probed in the SAME run on the SAME runner — every row is a controlled
// same-tick experiment, so a side-by-side of per-phase medians is honest
// PROVIDED the medians are drawn from the same mode population. Pooling all
// modes would let protocol support masquerade as latency (#820 review): a URL
// whose http3 attempts fail simply loses those samples, so its median is
// computed over a different protocol mix than a URL where http3 succeeds.
// Timing medians therefore only use attempts whose mode succeeded on EVERY
// URL (comparedModes); the success-rate row deliberately keeps all attempts.
// Pure functions; rendered by RunDetailPage's UrlComparisonTable.

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
  /**
   * Modes (tester protocol ids, e.g. 'http1'/'http3') with >= 1 successful
   * attempt on EVERY URL — the population the timing medians draw from.
   * Empty when the URLs share no successful mode; timing rows then pool all
   * successful attempts but crown no winner (bestIndex null).
   */
  comparedModes: string[];
  /**
   * Modes successful on some but not all URLs — dropped from the timing
   * comparison so protocol support can't masquerade as latency (#820 review).
   */
  excludedModes: string[];
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
// out at the cap) would skew a latency comparison — and only over modes in
// comparedModes (see buildUrlComparison), so every URL's median describes
// the same protocol mix.
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

  // Modes with >= 1 successful attempt, per URL; the timing comparison only
  // uses the intersection so both medians describe the same protocol mix.
  const successfulModeSets = urls.map(
    url => new Set(groups[url].filter(a => a.success).map(a => a.protocol)),
  );
  const allSuccessfulModes = [...new Set(successfulModeSets.flatMap(s => [...s]))].sort();
  const comparedModes = allSuccessfulModes.filter(m => successfulModeSets.every(s => s.has(m)));
  const excludedModes = allSuccessfulModes.filter(m => !comparedModes.includes(m));
  // No shared successful mode → the populations aren't comparable. Fall back
  // to pooling all successful attempts so the values still render, but crown
  // no winner on any timing row.
  const noSharedMode = comparedModes.length === 0;
  const comparedSet = new Set(comparedModes);

  const rows: UrlComparisonRow[] = [];

  // Success rate stays over ALL attempts of ALL modes — protocol-support
  // differences (e.g. http3 failing on one URL only) are exactly what this
  // row should surface, so it must not be constrained to comparedModes.
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
        .filter(a => a.success && (noSharedMode || comparedSet.has(a.protocol)))
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
      bestIndex: noSharedMode ? null : bestIndexOf(values, false),
    });
  }

  return { columns, rows, comparedModes, excludedModes };
}

export function formatComparisonValue(value: number | null, unit: 'ms' | '%'): string {
  if (value == null) return '-';
  if (unit === '%') return `${Math.round(value)}%`;
  if (value < 1000) return `${value.toFixed(1)}ms`;
  return `${(value / 1000).toFixed(2)}s`;
}
