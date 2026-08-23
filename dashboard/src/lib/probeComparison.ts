import type {
  ProbeComparisonCrowns,
  ProbeComparisonHeadToHead,
  ProbeComparisonMode,
  ProbeComparisonPoint,
  ProbeRankingVerdict,
} from '../api/types';

/**
 * Presentation helpers for the URL comparison report (#782 P3).
 *
 * The STATISTICS all come from the server (`ProbeComparisonLogic`) — nothing
 * here re-aggregates a series or decides a winner. What lives here is the
 * reading of that result: which crowns a URL wears, how a head-to-head record
 * reads as a sentence, why a ranking is greyed, and the colour a URL keeps
 * across every chart and table on the page.
 */

/** The crown categories, in the order the scoreboard shows them. */
export const CROWN_ORDER = [
  'fastest',
  'most_reliable',
  'most_consistent',
  'best_dns',
  'best_tcp',
  'best_tls',
  'best_ttfb',
] as const;

export type CrownKey = (typeof CROWN_ORDER)[number];

export const CROWN_LABEL: Record<CrownKey, string> = {
  fastest: 'Fastest',
  most_reliable: 'Most reliable',
  most_consistent: 'Most consistent',
  best_dns: 'Best DNS',
  best_tcp: 'Best TCP',
  best_tls: 'Best TLS',
  best_ttfb: 'Best TTFB',
};

/** Short title text explaining what each crown actually measured. */
export const CROWN_HINT: Record<CrownKey, string> = {
  fastest: 'Lowest median of the per-bucket p50s, over the shared buckets',
  most_reliable: 'Highest share of samples that succeeded, over the shared buckets',
  most_consistent: 'Lowest p95/p50 — the URL whose median predicts its requests best',
  best_dns: 'Lowest median DNS resolution time',
  best_tcp: 'Lowest median TCP connect time',
  best_tls: 'Lowest median TLS handshake time',
  best_ttfb: 'Lowest median time to first byte',
};

/**
 * Crowns a URL holds. A category nobody won (a tie, or a phase nothing
 * measured) is absent from every URL's list rather than awarded arbitrarily.
 */
export function crownsFor(crowns: ProbeComparisonCrowns, url: string): CrownKey[] {
  return CROWN_ORDER.filter((key) => crowns[key] === url);
}

/** True when no category had a winner at all. */
export function hasNoCrowns(crowns: ProbeComparisonCrowns): boolean {
  return CROWN_ORDER.every((key) => crowns[key] === null);
}

/**
 * A head-to-head record as the sentence the report actually claims — "faster in
 * 31 of the 42 hours both were probed". Never says "faster" about buckets in
 * which the two were not both measured.
 */
export function headToHeadSentence(h: ProbeComparisonHeadToHead): string {
  if (h.buckets === 0) return 'never measured together';
  if (h.a_wins === h.b_wins) return `even over ${h.buckets} shared bucket${h.buckets === 1 ? '' : 's'}`;
  const [winner, wins] = h.a_wins > h.b_wins ? [h.a, h.a_wins] : [h.b, h.b_wins];
  // shortLabel, not the raw URL: the matchup line beside this already names both
  // sides in short form, and mixing the two spellings makes the same URL read as
  // two different things on one line.
  return `${shortLabel(winner)} faster in ${wins} of ${h.buckets}`;
}

/**
 * Why a mode carries no ranking, in the reader's terms — and what to do about
 * it. Returns null when the mode IS ranked.
 */
export function notRankedReason(mode: ProbeComparisonMode, windowBuckets: number): string | null {
  if (mode.ranked) return null;
  const verdict: ProbeRankingVerdict = mode.ranking_verdict;
  if (verdict === 'no_data') {
    return 'No probe data for these URLs in this window.';
  }
  if (verdict === 'too_few_urls') {
    return 'Fewer than two of these URLs have enough data in this window to be compared.';
  }
  return (
    `Insufficient overlap — these URLs were measured together in only ${mode.shared_buckets} ` +
    `of ${windowBuckets} buckets (${formatPercent(mode.coverage_ratio)}). ` +
    'The numbers below are shown for reference, not ranked. Probing them on the same ' +
    'schedule makes the comparison fair.'
  );
}

/** Percentage with no false precision: "25%", "3.5%" only when it is under 10. */
export function formatPercent(fraction: number | null): string {
  if (fraction === null) return '—';
  const pct = fraction * 100;
  return `${pct < 10 && pct > 0 ? pct.toFixed(1) : Math.round(pct)}%`;
}

/** Jitter as "1.77x", the multiple of the median a slow request reaches. */
export function formatJitter(ratio: number | null): string {
  return ratio === null ? '—' : `${ratio.toFixed(2)}x`;
}

/**
 * Stable per-URL colour: a URL keeps the same colour in the chart, the
 * scoreboard and the coverage table, so the eye can follow one line across all
 * three. Indexed by the URL's position in the caller's selection, which is the
 * order the user chose.
 */
export const SERIES_COLORS = [
  '#47bfff', // cyan   — the primary accent
  '#9b74e8', // purple — the brand
  '#4ade80', // green
  '#fbbf24', // amber
  '#f472b6', // pink
  '#38bdf8', // sky
  '#a3e635', // lime
  '#fb923c', // orange
] as const;

export function colorFor(urls: readonly string[], url: string): string {
  const i = urls.indexOf(url);
  return SERIES_COLORS[(i < 0 ? 0 : i) % SERIES_COLORS.length];
}

/** A host-and-path label short enough for a chart legend, full URL in `title`. */
export function shortLabel(url: string): string {
  try {
    const u = new URL(url);
    const path = u.pathname === '/' ? '' : u.pathname;
    return `${u.host}${path}`;
  } catch {
    // Not a parseable URL (a bare host from an older run) — show it verbatim.
    return url;
  }
}

/** One row per bucket, one column per URL — the shape Recharts wants. */
export interface ChartRow {
  bucket: number;
  /** Bucket start, for the tooltip. */
  label: string;
  /** `p50:<url>` → ms, `shared` → whether the bucket counted. */
  [series: string]: number | string | boolean | null;
}

/**
 * Pivot the flat series into one row per bucket. A URL with no measurement in a
 * bucket gets `null`, NOT 0 — Recharts breaks the line there, which is the
 * honest rendering of "not measured" and the whole reason the comparison uses
 * shared buckets at all.
 */
export function toChartRows(points: ProbeComparisonPoint[], urls: readonly string[]): ChartRow[] {
  const byBucket = new Map<string, ChartRow>();

  for (const p of points) {
    let row = byBucket.get(p.bucket);
    if (!row) {
      row = { bucket: Date.parse(p.bucket), label: p.bucket, shared: p.shared };
      for (const u of urls) {
        row[`p50:${u}`] = null;
        row[`ok:${u}`] = null;
      }
      byBucket.set(p.bucket, row);
    }
    // `shared` is a property of the BUCKET, so any point carrying it settles it.
    if (p.shared) row.shared = true;
    row[`p50:${p.url}`] = p.p50_total_ms;
    row[`ok:${p.url}`] = p.sample_count > 0 ? p.success_count / p.sample_count : null;
  }

  return [...byBucket.values()].sort((a, b) => a.bucket - b.bucket);
}
