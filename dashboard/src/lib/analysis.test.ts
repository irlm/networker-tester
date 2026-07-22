// Tests for the dashboard statistics module (lib/analysis.ts). This is a
// PARALLEL reimplementation of crates/networker-tester/src/metrics.rs
// (compute_stats, primary_metric_value, attempt_payload_bytes) — untested until
// now, and drift from the Rust reference silently shows wrong numbers on screen.
//
// The vectors below deliberately match the Rust known-value tests
// (metrics.rs::compute_stats_known_values et al.) so a divergence in the
// percentile/aggregate math fails here. Two intentional-or-not differences from
// Rust are PINNED (not "fixed" — see the notes) so any future change is a
// conscious decision:
//   * computeStats does NOT suppress p95/p99 at small n (Rust returns None below
//     n=20 / n=100 to avoid presenting max as a tail estimate).
//   * primaryMetricValue for 'udp' has no "exclude fully-lost attempts" guard
//     (Rust filters success_count>0 so a 0.0-sentinel RTT stays out of the dist).

import { describe, it, expect } from 'vitest';
import type { LiveAttempt } from '../api/types';
import {
  computeStats,
  primaryMetricValue,
  primaryMetricLabel,
  isThroughputProtocol,
  attemptPayloadBytes,
  computeProtocolStats,
  computeTimingBreakdown,
  groupByProtocolAndPayload,
  formatMs,
  formatThroughput,
  formatBytes,
  formatMetricValue,
  successRateClass,
} from './analysis';

// Minimal LiveAttempt factory — only the fields the analysis functions read.
function mk(overrides: Partial<LiveAttempt> & { protocol: string }): LiveAttempt {
  return {
    attempt_id: 'a',
    run_id: 'r',
    sequence_num: 0,
    started_at: '2026-07-22T00:00:00Z',
    finished_at: '2026-07-22T00:00:01Z',
    success: true,
    retry_count: 0,
    ...overrides,
  };
}

describe('computeStats', () => {
  it('returns null for an empty sample', () => {
    expect(computeStats([])).toBeNull();
  });

  it('a single value is its own min/mean/median/max with zero stddev', () => {
    const s = computeStats([7])!;
    expect(s.count).toBe(1);
    expect([s.min, s.mean, s.p50, s.max]).toEqual([7, 7, 7, 7]);
    expect(s.stddev).toBe(0);
  });

  it('matches the Rust known-value vector 1..=10 (drift guard)', () => {
    const s = computeStats([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])!;
    expect(s.count).toBe(10);
    expect(s.min).toBe(1);
    expect(s.max).toBe(10);
    expect(s.mean).toBeCloseTo(5.5, 10);
    // p50 rank = 0.5*(10-1) = 4.5 → 5 + 0.5*(6-5) = 5.5 (same formula as Rust)
    expect(s.p50).toBeCloseTo(5.5, 10);
    // stddev (population) = sqrt(8.25) ≈ 2.8723 — matches metrics.rs
    expect(s.stddev).toBeCloseTo(Math.sqrt(8.25), 10);
    // p25 rank = 0.25*9 = 2.25 → 3 + 0.25 = 3.25 ; p75 → 7.75
    expect(s.p25).toBeCloseTo(3.25, 10);
    expect(s.p75).toBeCloseTo(7.75, 10);
  });

  it('interpolates percentiles linearly between neighbours', () => {
    // [10,20,30,40] p50 rank = 0.5*3 = 1.5 → 20 + (30-20)*0.5 = 25
    expect(computeStats([10, 20, 30, 40])!.p50).toBeCloseTo(25, 10);
  });

  it('sorts before computing (input order does not matter)', () => {
    expect(computeStats([10, 1, 5])!.p50).toBe(5);
    expect(computeStats([5, 1, 10])!.min).toBe(1);
  });

  it('PINNED drift: p95/p99 ARE computed at small n (Rust suppresses them)', () => {
    // [1,2,3]: p95 rank = 0.95*2 = 1.9 → 2 + (3-2)*0.9 = 2.9. Rust would return
    // None here (n < 20). If this suddenly becomes null, someone aligned the
    // suppression rule — a deliberate contract change, not an accident.
    const s = computeStats([1, 2, 3])!;
    expect(s.p95).toBeCloseTo(2.9, 10);
    expect(typeof s.p99).toBe('number');
  });
});

describe('primaryMetricValue — per-protocol field selection', () => {
  it('http-family reads total_duration_ms', () => {
    for (const p of ['http1', 'http2', 'http3', 'native', 'curl']) {
      const v = primaryMetricValue(mk({ protocol: p, http: { status_code: 200, ttfb_ms: 5, total_duration_ms: 42, negotiated_version: 'h2' } }));
      expect(v).toBe(42);
    }
  });

  it('tcp/dns/tls/browser/pageload each read their own field', () => {
    expect(primaryMetricValue(mk({ protocol: 'tcp', tcp: { connect_duration_ms: 11, remote_addr: 'x' } }))).toBe(11);
    expect(primaryMetricValue(mk({ protocol: 'dns', dns: { duration_ms: 3, query_name: 'x', resolved_ips: [] } }))).toBe(3);
    expect(primaryMetricValue(mk({ protocol: 'tls', tls: { handshake_duration_ms: 9, protocol_version: '1.3', cipher_suite: 'x' } }))).toBe(9);
    expect(primaryMetricValue(mk({ protocol: 'browser', browser: { load_ms: 120 } }))).toBe(120);
    expect(primaryMetricValue(mk({ protocol: 'pageload', page_load: { total_ms: 200, asset_count: 1, assets_fetched: 1 } }))).toBe(200);
  });

  it('throughput protocols read http.throughput_mbps, not latency', () => {
    const a = mk({ protocol: 'download', http: { status_code: 200, ttfb_ms: 5, total_duration_ms: 1000, negotiated_version: 'h2', throughput_mbps: 94.5 } });
    expect(primaryMetricValue(a)).toBe(94.5); // NOT total_duration_ms — a swap here would show 1000 MB/s
  });

  it('returns null when the relevant sub-result is absent', () => {
    expect(primaryMetricValue(mk({ protocol: 'http1' }))).toBeNull();
    expect(primaryMetricValue(mk({ protocol: 'tcp' }))).toBeNull();
  });

  it('PINNED drift: udp does NOT exclude a fully-lost (0.0 RTT) attempt', () => {
    // Rust filters success_count>0 so a lost probe stays out of the RTT dist;
    // the TS side returns the 0.0 sentinel. Pin it so the divergence is visible.
    const lost = mk({ protocol: 'udp', udp: { rtt_avg_ms: 0, loss_percent: 100, probe_count: 10, success_count: 0 } });
    expect(primaryMetricValue(lost)).toBe(0);
  });
});

describe('primaryMetricLabel / isThroughputProtocol / attemptPayloadBytes', () => {
  it('labels match the metric each protocol reports', () => {
    expect(primaryMetricLabel('tcp')).toBe('Connect ms');
    expect(primaryMetricLabel('udp')).toBe('RTT avg ms');
    expect(primaryMetricLabel('dns')).toBe('Resolve ms');
    expect(primaryMetricLabel('tls')).toBe('Handshake ms');
    expect(primaryMetricLabel('download')).toBe('Throughput MB/s');
    expect(primaryMetricLabel('browser')).toBe('Load ms');
    expect(primaryMetricLabel('http2')).toBe('Total ms'); // default
  });

  it('classifies only transfer protocols as throughput', () => {
    for (const p of ['download', 'download3', 'upload', 'webdownload', 'udpupload']) {
      expect(isThroughputProtocol(p)).toBe(true);
    }
    for (const p of ['http1', 'tcp', 'dns', 'tls', 'pageload', 'browser']) {
      expect(isThroughputProtocol(p)).toBe(false);
    }
  });

  it('extracts payload bytes for throughput protocols only', () => {
    const dl = mk({ protocol: 'download', http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 1, negotiated_version: 'h2', payload_bytes: 1048576 } });
    expect(attemptPayloadBytes(dl)).toBe(1048576);
    const http = mk({ protocol: 'http1', http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 1, negotiated_version: 'h2', payload_bytes: 1048576 } });
    expect(attemptPayloadBytes(http)).toBeNull(); // latency protocol → no payload grouping
  });
});

describe('computeProtocolStats', () => {
  it('computes per-group stats, success rate and label from a mixed set', () => {
    const attempts = [
      mk({ protocol: 'http1', success: true, http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 10, negotiated_version: 'h2' } }),
      mk({ protocol: 'http1', success: true, http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 20, negotiated_version: 'h2' } }),
      mk({ protocol: 'http1', success: false }), // failure counts toward the rate, not the stats
    ];
    const res = computeProtocolStats(attempts);
    expect(res).toHaveLength(1);
    const g = res[0];
    expect(g.protocol).toBe('http1');
    expect(g.label).toBe('Total ms');
    expect(g.stats.mean).toBe(15); // over [10,20] only
    expect(g.successCount).toBe(2);
    expect(g.totalAttempts).toBe(3);
    expect(g.successRate).toBeCloseTo((2 / 3) * 100, 10); // 66.67%, NOT 100%
  });

  it('separates throughput groups by payload size', () => {
    const attempts = [
      mk({ protocol: 'download', success: true, http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 1, negotiated_version: 'h2', throughput_mbps: 90, payload_bytes: 1048576 } }),
      mk({ protocol: 'download', success: true, http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 1, negotiated_version: 'h2', throughput_mbps: 900, payload_bytes: 10485760 } }),
    ];
    const res = computeProtocolStats(attempts);
    expect(res).toHaveLength(2); // two distinct payloads → two rows
    expect(res.map((r) => r.payloadBytes).sort((a, b) => (a ?? 0) - (b ?? 0))).toEqual([1048576, 10485760]);
  });
});

describe('computeTimingBreakdown', () => {
  it('averages each phase over successful attempts and excludes throughput', () => {
    const attempts = [
      mk({ protocol: 'http1', success: true, dns: { duration_ms: 1, query_name: 'x', resolved_ips: [] }, tcp: { connect_duration_ms: 10, remote_addr: 'x' }, http: { status_code: 200, ttfb_ms: 4, total_duration_ms: 40, negotiated_version: 'h2' } }),
      mk({ protocol: 'http1', success: true, dns: { duration_ms: 3, query_name: 'x', resolved_ips: [] }, tcp: { connect_duration_ms: 20, remote_addr: 'x' }, http: { status_code: 200, ttfb_ms: 6, total_duration_ms: 60, negotiated_version: 'h2' } }),
      mk({ protocol: 'download', success: true, http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 1, negotiated_version: 'h2', throughput_mbps: 90, payload_bytes: 1024 } }),
    ];
    const res = computeTimingBreakdown(attempts);
    expect(res).toHaveLength(1); // download excluded
    const b = res[0];
    expect(b.protocol).toBe('http1');
    expect(b.avgDns).toBe(2);
    expect(b.avgTcp).toBe(15);
    expect(b.avgTtfb).toBe(5);
    expect(b.avgTotal).toBe(50);
  });
});

describe('groupByProtocolAndPayload', () => {
  it('keys throughput by protocol:payload and others by protocol', () => {
    const g = groupByProtocolAndPayload([
      mk({ protocol: 'http1' }),
      mk({ protocol: 'download', http: { status_code: 200, ttfb_ms: 1, total_duration_ms: 1, negotiated_version: 'h2', payload_bytes: 2048 } }),
    ]);
    expect([...g.keys()].sort()).toEqual(['download:2048', 'http1']);
  });
});

describe('formatters — boundary correctness', () => {
  it('formatMs switches units at the 1ms and 100ms boundaries', () => {
    expect(formatMs(null)).toBe('-');
    expect(formatMs(0.5)).toBe('500µs');   // < 1 → microseconds
    expect(formatMs(0.999)).toBe('999µs');
    expect(formatMs(1)).toBe('1.00ms');     // boundary → 2 decimals
    expect(formatMs(99.99)).toBe('99.99ms');
    expect(formatMs(100)).toBe('100.0ms');  // ≥ 100 → 1 decimal
    expect(formatMs(1234.5)).toBe('1234.5ms');
  });

  it('formatThroughput switches to GB/s at 1000 MB/s', () => {
    expect(formatThroughput(null)).toBe('-');
    expect(formatThroughput(999.9)).toBe('999.9 MB/s');
    expect(formatThroughput(1000)).toBe('1.0 GB/s');
    expect(formatThroughput(1500)).toBe('1.5 GB/s');
  });

  it('formatBytes uses binary divisors at each boundary', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1023)).toBe('1023 B');
    expect(formatBytes(1024)).toBe('1 KB');
    expect(formatBytes(1048576)).toBe('1.0 MB');
    expect(formatBytes(1073741824)).toBe('1.0 GB');
  });

  it('formatMetricValue routes throughput vs latency by protocol', () => {
    expect(formatMetricValue('download', 500)).toBe('500.0 MB/s');
    expect(formatMetricValue('http1', 50)).toBe('50.00ms');
    expect(formatMetricValue('http1', null)).toBe('-');
  });

  it('successRateClass thresholds at 100 and 80', () => {
    expect(successRateClass(100)).toBe('text-green-400');
    expect(successRateClass(99.9)).toBe('text-yellow-400');
    expect(successRateClass(80)).toBe('text-yellow-400');
    expect(successRateClass(79.9)).toBe('text-red-400');
  });
});
