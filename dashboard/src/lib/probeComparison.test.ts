// The reading of a comparison result: which crowns a URL wears, how a
// head-to-head record reads, why a ranking is greyed, and the pivot that turns
// a flat series into chart rows WITHOUT inventing a zero for a bucket nobody
// measured.

import { describe, expect, it } from 'vitest';
import type {
  ProbeComparisonCrowns,
  ProbeComparisonHeadToHead,
  ProbeComparisonMode,
  ProbeComparisonPoint,
} from '../api/types';
import {
  CROWN_ORDER,
  colorFor,
  crownsFor,
  formatJitter,
  formatPercent,
  hasNoCrowns,
  headToHeadSentence,
  notRankedReason,
  shortLabel,
  toChartRows,
} from './probeComparison';

const NO_CROWNS: ProbeComparisonCrowns = {
  fastest: null,
  most_reliable: null,
  most_consistent: null,
  best_dns: null,
  best_tcp: null,
  best_tls: null,
  best_ttfb: null,
};

function mode(over: Partial<ProbeComparisonMode> = {}): ProbeComparisonMode {
  return {
    mode: 'http2',
    ranked: true,
    ranking_verdict: 'ranked',
    shared_buckets: 60,
    coverage_ratio: 0.36,
    coverage: [],
    scores: [],
    head_to_head: [],
    crowns: NO_CROWNS,
    series: [],
    ...over,
  };
}

function point(over: Partial<ProbeComparisonPoint> & { url: string; bucket: string }): ProbeComparisonPoint {
  return {
    sample_count: 5,
    success_count: 5,
    shared: true,
    p50_total_ms: 100,
    p95_total_ms: 150,
    p50_dns_ms: null,
    p50_tcp_ms: null,
    p50_tls_ms: null,
    p50_ttfb_ms: null,
    dominant_error_category: null,
    ...over,
  };
}

describe('crowns', () => {
  it('lists only the categories a URL actually won', () => {
    const crowns: ProbeComparisonCrowns = {
      ...NO_CROWNS,
      fastest: 'https://a/',
      most_reliable: 'https://a/',
      most_consistent: 'https://b/',
    };

    expect(crownsFor(crowns, 'https://a/')).toEqual(['fastest', 'most_reliable']);
    expect(crownsFor(crowns, 'https://b/')).toEqual(['most_consistent']);
    expect(crownsFor(crowns, 'https://c/')).toEqual([]);
  });

  it('awards nothing when every category was a tie', () => {
    expect(hasNoCrowns(NO_CROWNS)).toBe(true);
    expect(crownsFor(NO_CROWNS, 'https://a/')).toEqual([]);
  });

  it('a category with no winner never lands on a URL', () => {
    // best_tls is null because nothing measured TLS — no URL may wear it.
    const crowns = { ...NO_CROWNS, fastest: 'https://a/' };
    expect(hasNoCrowns(crowns)).toBe(false);
    expect(crownsFor(crowns, 'https://a/')).toEqual(['fastest']);
    expect(CROWN_ORDER.every((k) => crowns[k] === null || crowns[k] === 'https://a/')).toBe(true);
  });
});

describe('head-to-head sentence', () => {
  const h = (over: Partial<ProbeComparisonHeadToHead>): ProbeComparisonHeadToHead => ({
    a: 'https://a/', b: 'https://b/', buckets: 42, a_wins: 31, b_wins: 11, ties: 0, ...over,
  });

  it('names the winner and the buckets they were compared over', () => {
    // Short label, matching the matchup line beside it — one spelling per URL.
    expect(headToHeadSentence(h({}))).toBe('a faster in 31 of 42');
  });

  it('names the other side when it won', () => {
    expect(headToHeadSentence(h({ a_wins: 11, b_wins: 31 }))).toBe('b faster in 31 of 42');
  });

  it('says even rather than picking a winner', () => {
    expect(headToHeadSentence(h({ a_wins: 21, b_wins: 21 }))).toBe('even over 42 shared buckets');
  });

  it('never claims faster about URLs that were never measured together', () => {
    expect(headToHeadSentence(h({ buckets: 0, a_wins: 0, b_wins: 0 })))
      .toBe('never measured together');
  });
});

describe('why a ranking is withheld', () => {
  it('is null when the mode IS ranked', () => {
    expect(notRankedReason(mode(), 168)).toBeNull();
  });

  it('explains thin overlap with the real numbers and what to do', () => {
    const reason = notRankedReason(
      mode({ ranked: false, ranking_verdict: 'insufficient_overlap', shared_buckets: 42, coverage_ratio: 0.25 }),
      168,
    );
    expect(reason).toContain('42');
    expect(reason).toContain('168');
    expect(reason).toContain('25%');
    expect(reason).toContain('same schedule');
  });

  it('distinguishes no data from too few URLs from thin overlap', () => {
    expect(notRankedReason(mode({ ranked: false, ranking_verdict: 'no_data' }), 168))
      .toMatch(/No probe data/);
    expect(notRankedReason(mode({ ranked: false, ranking_verdict: 'too_few_urls' }), 168))
      .toMatch(/Fewer than two/);
  });
});

describe('formatting', () => {
  it('shows a percentage without false precision', () => {
    expect(formatPercent(0.25)).toBe('25%');
    expect(formatPercent(0.035)).toBe('3.5%');
    expect(formatPercent(1)).toBe('100%');
    expect(formatPercent(0)).toBe('0%');
    expect(formatPercent(null)).toBe('—');
  });

  it('shows jitter as a multiple of the median', () => {
    expect(formatJitter(1.7734)).toBe('1.77x');
    expect(formatJitter(1)).toBe('1.00x');
    expect(formatJitter(null)).toBe('—');
  });

  it('shortens a URL to host and path but survives a bare host', () => {
    expect(shortLabel('https://example.com/')).toBe('example.com');
    expect(shortLabel('https://example.com/health')).toBe('example.com/health');
    expect(shortLabel('https://example.com:8443/x')).toBe('example.com:8443/x');
    expect(shortLabel('not a url')).toBe('not a url');
  });
});

describe('series colours', () => {
  it('give a URL the same colour wherever it appears', () => {
    const urls = ['https://a/', 'https://b/'];
    expect(colorFor(urls, 'https://a/')).toBe(colorFor(urls, 'https://a/'));
    expect(colorFor(urls, 'https://a/')).not.toBe(colorFor(urls, 'https://b/'));
  });

  it('falls back to a real colour for a URL outside the selection', () => {
    expect(colorFor(['https://a/'], 'https://zzz/')).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe('chart pivot', () => {
  it('emits one row per bucket, in time order', () => {
    const rows = toChartRows(
      [
        point({ url: 'https://a/', bucket: '2026-08-20T01:00:00Z', p50_total_ms: 110 }),
        point({ url: 'https://a/', bucket: '2026-08-20T00:00:00Z', p50_total_ms: 100 }),
        point({ url: 'https://b/', bucket: '2026-08-20T00:00:00Z', p50_total_ms: 200 }),
      ],
      ['https://a/', 'https://b/'],
    );

    expect(rows).toHaveLength(2);
    expect(rows[0]['p50:https://a/']).toBe(100);
    expect(rows[0]['p50:https://b/']).toBe(200);
    expect(rows[1]['p50:https://a/']).toBe(110);
  });

  it('leaves a bucket a URL did not measure NULL, never zero', () => {
    // A zero would draw a line to the floor and read as "instant".
    const rows = toChartRows(
      [point({ url: 'https://a/', bucket: '2026-08-20T00:00:00Z', p50_total_ms: 100 })],
      ['https://a/', 'https://b/'],
    );

    expect(rows[0]['p50:https://b/']).toBeNull();
    expect(rows[0]['p50:https://b/']).not.toBe(0);
  });

  it('keeps a failed bucket as a point with a null latency', () => {
    const rows = toChartRows(
      [point({ url: 'https://a/', bucket: '2026-08-20T00:00:00Z', sample_count: 5, success_count: 0, p50_total_ms: null })],
      ['https://a/'],
    );

    expect(rows[0]['p50:https://a/']).toBeNull();
    expect(rows[0]['ok:https://a/']).toBe(0);
  });

  it('marks the bucket shared when any point in it counted', () => {
    const rows = toChartRows(
      [
        point({ url: 'https://a/', bucket: '2026-08-20T00:00:00Z', shared: false }),
        point({ url: 'https://b/', bucket: '2026-08-20T00:00:00Z', shared: true }),
      ],
      ['https://a/', 'https://b/'],
    );

    expect(rows[0].shared).toBe(true);
  });
});
