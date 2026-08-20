// #782: fair comparison across the URLs of a set run — per-phase medians over
// successful attempts, side by side. Same run, same runner, same tick, so no
// shared-bucket machinery is needed; the honesty rules are (a) medians ignore
// failed attempts, (b) a winner is crowned only when unique among >= 2
// non-null values, (c) single-URL runs produce no comparison at all, and
// (d) timing medians only pool modes successful on EVERY URL (#820 review) —
// otherwise protocol support masquerades as latency; the success-rate row
// alone keeps all attempts of all modes.

import { describe, expect, it } from 'vitest';
import type { LiveAttempt } from '../../api/types';
import { buildUrlComparison, formatComparisonValue, shortUrlLabel } from './urlComparison';

let seq = 0;
function attempt(over: Partial<LiveAttempt>): LiveAttempt {
  seq += 1;
  return {
    attempt_id: `a-${seq}`,
    run_id: 'r-1',
    protocol: 'http2',
    sequence_num: seq,
    started_at: '2026-08-20T00:00:00Z',
    finished_at: '2026-08-20T00:00:01Z',
    success: true,
    retry_count: 0,
    ...over,
  } as LiveAttempt;
}

const A = 'https://a.example/';
const B = 'https://b.example/';

describe('buildUrlComparison', () => {
  it('returns null for single-URL and unattributed runs', () => {
    expect(buildUrlComparison({})).toBeNull();
    expect(buildUrlComparison({ [A]: [attempt({})] })).toBeNull();
    // pre-#782 attempts land under '' — not comparable.
    expect(buildUrlComparison({ '': [attempt({})], [A]: [attempt({})] })).toBeNull();
  });

  it('computes per-phase medians per URL and crowns the unique winner', () => {
    const groups = {
      [A]: [
        attempt({ dns: { duration_ms: 10, query_name: 'a', resolved_ips: [] }, http: { status_code: 200, ttfb_ms: 100, total_duration_ms: 200, negotiated_version: 'h2' } }),
        attempt({ dns: { duration_ms: 30, query_name: 'a', resolved_ips: [] }, http: { status_code: 200, ttfb_ms: 120, total_duration_ms: 240, negotiated_version: 'h2' } }),
        attempt({ dns: { duration_ms: 20, query_name: 'a', resolved_ips: [] } }),
      ],
      [B]: [
        attempt({ dns: { duration_ms: 50, query_name: 'b', resolved_ips: [] }, http: { status_code: 200, ttfb_ms: 80, total_duration_ms: 300, negotiated_version: 'h2' } }),
      ],
    };
    const cmp = buildUrlComparison(groups)!;
    expect(cmp.columns.map(c => c.url)).toEqual([A, B]);
    expect(cmp.columns.map(c => c.attempts)).toEqual([3, 1]);

    const byKey = Object.fromEntries(cmp.rows.map(r => [r.key, r]));
    // dns: A median of [10,20,30] = 20 vs B 50 → A wins.
    expect(byKey.dns.values).toEqual([20, 50]);
    expect(byKey.dns.bestIndex).toBe(0);
    // ttfb: A median 110 vs B 80 → B wins (lower is better).
    expect(byKey.ttfb.values).toEqual([110, 80]);
    expect(byKey.ttfb.bestIndex).toBe(1);
    // No tcp/tls data at all → rows omitted entirely.
    expect(byKey.tcp).toBeUndefined();
    expect(byKey.tls).toBeUndefined();
  });

  it('medians ignore failed attempts; success rate counts them', () => {
    const groups = {
      [A]: [
        attempt({ dns: { duration_ms: 10, query_name: 'a', resolved_ips: [] } }),
        attempt({ success: false, dns: { duration_ms: 5000, query_name: 'a', resolved_ips: [] } }),
      ],
      [B]: [attempt({ dns: { duration_ms: 15, query_name: 'b', resolved_ips: [] } })],
    };
    const cmp = buildUrlComparison(groups)!;
    const byKey = Object.fromEntries(cmp.rows.map(r => [r.key, r]));
    expect(byKey.dns.values).toEqual([10, 15]); // the timed-out 5000ms never skews A
    expect(byKey.success.values).toEqual([50, 100]);
    expect(byKey.success.bestIndex).toBe(1);
    expect(byKey.success.higherIsBetter).toBe(true);
  });

  it('a tied first place crowns nobody; a single non-null value crowns nobody', () => {
    const groups = {
      [A]: [attempt({ dns: { duration_ms: 10, query_name: 'a', resolved_ips: [] } })],
      [B]: [attempt({ dns: { duration_ms: 10, query_name: 'b', resolved_ips: [] } })],
    };
    const cmp = buildUrlComparison(groups)!;
    const byKey = Object.fromEntries(cmp.rows.map(r => [r.key, r]));
    expect(byKey.dns.bestIndex).toBeNull();
    expect(byKey.success.bestIndex).toBeNull(); // both 100%

    const oneSided = buildUrlComparison({
      [A]: [attempt({ tls: { handshake_duration_ms: 42, protocol_version: '1.3', cipher_suite: 'x' } })],
      [B]: [attempt({})],
    })!;
    const tls = oneSided.rows.find(r => r.key === 'tls')!;
    expect(tls.values).toEqual([42, null]);
    expect(tls.bestIndex).toBeNull();
  });

  // #820 review: the motivating scenario — A speaks http3, B's http3 attempts
  // fail. B's timing median is over http1 only, so A's must be too; pooling
  // A's fast h3 samples would compare different populations, and pre-fix the
  // mode mix (not latency) could pick the winner.
  it('timing medians only pool modes successful on every URL', () => {
    const groups = {
      [A]: [
        attempt({ protocol: 'http1', http: { status_code: 200, ttfb_ms: 100, total_duration_ms: 200, negotiated_version: 'http/1.1' } }),
        attempt({ protocol: 'http1', http: { status_code: 200, ttfb_ms: 120, total_duration_ms: 240, negotiated_version: 'http/1.1' } }),
        attempt({ protocol: 'http3', http: { status_code: 200, ttfb_ms: 20, total_duration_ms: 40, negotiated_version: 'h3' } }),
        attempt({ protocol: 'http3', http: { status_code: 200, ttfb_ms: 30, total_duration_ms: 60, negotiated_version: 'h3' } }),
      ],
      [B]: [
        attempt({ protocol: 'http1', http: { status_code: 200, ttfb_ms: 150, total_duration_ms: 300, negotiated_version: 'http/1.1' } }),
        attempt({ protocol: 'http3', success: false }),
        attempt({ protocol: 'http3', success: false }),
      ],
    };
    const cmp = buildUrlComparison(groups)!;
    expect(cmp.comparedModes).toEqual(['http1']);
    expect(cmp.excludedModes).toEqual(['http3']);

    const byKey = Object.fromEntries(cmp.rows.map(r => [r.key, r]));
    // A's median is over http1 only: 110, NOT the pooled [20,30,100,120] = 65.
    expect(byKey.ttfb.values).toEqual([110, 150]);
    expect(byKey.ttfb.bestIndex).toBe(0); // A wins on the shared mode
    // Success rate keeps ALL attempts — B's failed http3 probes are the very
    // protocol-support signal this row exists to surface.
    expect(byKey.success.values).toEqual([100, (1 / 3) * 100]);
    expect(byKey.success.bestIndex).toBe(0);
  });

  it('empty mode intersection: values pooled for reference, no timing winner crowned', () => {
    const groups = {
      [A]: [
        attempt({ protocol: 'http3', http: { status_code: 200, ttfb_ms: 25, total_duration_ms: 50, negotiated_version: 'h3' } }),
        attempt({ protocol: 'http1', success: false }),
      ],
      [B]: [
        attempt({ protocol: 'http1', http: { status_code: 200, ttfb_ms: 90, total_duration_ms: 180, negotiated_version: 'http/1.1' } }),
      ],
    };
    const cmp = buildUrlComparison(groups)!;
    expect(cmp.comparedModes).toEqual([]);
    expect(cmp.excludedModes).toEqual(['http1', 'http3']);

    const byKey = Object.fromEntries(cmp.rows.map(r => [r.key, r]));
    // Values are still shown (pooled over all successful attempts)…
    expect(byKey.ttfb.values).toEqual([25, 90]);
    expect(byKey.total.values).toEqual([50, 180]);
    // …but no timing row crowns a winner — the populations aren't comparable.
    for (const row of cmp.rows.filter(r => r.unit === 'ms')) {
      expect(row.bestIndex).toBeNull();
    }
    // The success-rate row is exempt: all attempts, normal winner logic.
    expect(byKey.success.values).toEqual([50, 100]);
    expect(byKey.success.bestIndex).toBe(1);
  });

  it('all modes shared → nothing excluded', () => {
    const cmp = buildUrlComparison({
      [A]: [attempt({ protocol: 'http2', dns: { duration_ms: 10, query_name: 'a', resolved_ips: [] } })],
      [B]: [attempt({ protocol: 'http2', dns: { duration_ms: 20, query_name: 'b', resolved_ips: [] } })],
    })!;
    expect(cmp.comparedModes).toEqual(['http2']);
    expect(cmp.excludedModes).toEqual([]);
    expect(cmp.rows.find(r => r.key === 'dns')!.bestIndex).toBe(0);
  });
});

describe('formatting', () => {
  it('shortUrlLabel strips scheme and trailing slash', () => {
    expect(shortUrlLabel('https://example.com/')).toBe('example.com');
    expect(shortUrlLabel('https://example.com/pricing')).toBe('example.com/pricing');
    expect(shortUrlLabel('http://example.com/')).toBe('example.com');
  });

  it('formatComparisonValue: ms under a second, s above, dash for null', () => {
    expect(formatComparisonValue(12.34, 'ms')).toBe('12.3ms');
    expect(formatComparisonValue(1500, 'ms')).toBe('1.50s');
    expect(formatComparisonValue(null, 'ms')).toBe('-');
    expect(formatComparisonValue(99.6, '%')).toBe('100%');
  });
});
