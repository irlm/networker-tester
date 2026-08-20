// #782 P2: median + spread per measurement point. The honesty rules are the
// point of this module, so they are what gets pinned: (a) every published
// sample counts, including failed ones, (b) a median needs samples — below the
// floor the row is flagged so the UI says "1 sample" instead of presenting one
// reading as a median, (c) points are per (URL × mode × payload), so a URL-set
// run never pools two URLs into one median.

import { describe, expect, it } from 'vitest';
import type { LiveAttempt } from '../../api/types';
import {
  buildSamplePoints,
  hasRepeatedSamples,
  jitterRatio,
  usedBurstSampling,
  MIN_SAMPLES_FOR_MEDIAN,
} from './burstStats';

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

/** One http2 burst sample with a total_duration_ms of `ms`. */
function sample(ms: number, sampleIndex: number, over: Partial<LiveAttempt> = {}) {
  return attempt({
    sample_index: sampleIndex,
    http: {
      status_code: 200,
      ttfb_ms: ms / 2,
      total_duration_ms: ms,
      negotiated_version: 'HTTP/2.0',
    },
    ...over,
  });
}

describe('buildSamplePoints', () => {
  it('computes the median over a burst of five samples', () => {
    const points = buildSamplePoints([10, 12, 11, 40, 11].map((ms, i) => sample(ms, i)));

    expect(points).toHaveLength(1);
    const p = points[0];
    expect(p.protocol).toBe('http2');
    expect(p.sampleCount).toBe(5);
    expect(p.usableCount).toBe(5);
    expect(p.burstSize).toBe(5);
    expect(p.underSampled).toBe(false);
    // Sorted: 10 11 11 12 40 — the 40ms outlier does NOT move the median,
    // which is the whole reason for taking five samples.
    expect(p.stats!.p50).toBe(11);
    expect(p.stats!.min).toBe(10);
    expect(p.stats!.max).toBe(40);
  });

  it('counts a failed sample as a failed sample, never a missing one', () => {
    const points = buildSamplePoints([
      sample(10, 0),
      sample(12, 1),
      attempt({ sample_index: 2, success: false, error_message: 'connection refused' }),
      sample(11, 3),
    ]);

    const p = points[0];
    expect(p.sampleCount).toBe(4);   // all four ran
    expect(p.usableCount).toBe(3);   // three produced a timing
    expect(p.failedCount).toBe(1);
    expect(p.stats!.count).toBe(3);  // the median is over what succeeded
  });

  it('flags a point whose median rests on too few samples', () => {
    const [single] = buildSamplePoints([sample(42, 0)]);
    expect(single.usableCount).toBe(1);
    expect(single.underSampled).toBe(true);
    // The value is still reported — it is a real reading — but the caller must
    // not call it a median, and there is no spread to speak of.
    expect(single.stats!.p50).toBe(42);
    expect(jitterRatio(single)).toBeNull();

    const [two] = buildSamplePoints([sample(10, 0), sample(20, 1)]);
    expect(two.underSampled).toBe(true);

    const [three] = buildSamplePoints([sample(10, 0), sample(20, 1), sample(30, 2)]);
    expect(three.underSampled).toBe(false);
    expect(MIN_SAMPLES_FOR_MEDIAN).toBe(3);
  });

  it('reports no stats at all when every sample of a point failed', () => {
    const [p] = buildSamplePoints([
      attempt({ sample_index: 0, success: false }),
      attempt({ sample_index: 1, success: false }),
    ]);
    expect(p.sampleCount).toBe(2);
    expect(p.failedCount).toBe(2);
    expect(p.stats).toBeNull();
    expect(jitterRatio(p)).toBeNull();
  });

  it('keeps each URL of a set run its own point', () => {
    const A = 'https://a.example/';
    const B = 'https://b.example/';
    const points = buildSamplePoints([
      sample(10, 0, { target_url: A }),
      sample(12, 1, { target_url: A }),
      sample(11, 2, { target_url: A }),
      sample(90, 0, { target_url: B }),
      sample(92, 1, { target_url: B }),
      sample(91, 2, { target_url: B }),
    ]);

    expect(points.map((p) => p.targetUrl)).toEqual([A, B]);
    expect(points[0].stats!.p50).toBe(11);
    expect(points[1].stats!.p50).toBe(91);
  });

  it('separates modes and payload sizes into their own points', () => {
    const points = buildSamplePoints([
      sample(10, 0),
      sample(12, 1),
      attempt({ protocol: 'tcp', sample_index: 0, tcp: { connect_duration_ms: 3, remote_addr: '1.2.3.4:443' } }),
      attempt({ protocol: 'tcp', sample_index: 1, tcp: { connect_duration_ms: 5, remote_addr: '1.2.3.4:443' } }),
    ]);

    expect(points.map((p) => p.protocol)).toEqual(['http2', 'tcp']);
    expect(points.every((p) => p.sampleCount === 2)).toBe(true);
  });

  it('treats --runs repeats as samples but does not call them a burst', () => {
    // Pre-#782 attempts (and `--runs N` repeats) carry no sample_index: the
    // repeats are still samples of one point, but burstSize stays 1 so the UI
    // can name the section honestly.
    const points = buildSamplePoints([
      sample(10, 0),
      { ...sample(12, 0), sample_index: undefined } as LiveAttempt,
      { ...sample(11, 0), sample_index: undefined } as LiveAttempt,
    ]);

    expect(points[0].sampleCount).toBe(3);
    expect(points[0].burstSize).toBe(1);
    expect(usedBurstSampling(points)).toBe(false);
    expect(hasRepeatedSamples(points)).toBe(true);
  });

  it('hides the section for a run whose every point ran once', () => {
    const points = buildSamplePoints([
      sample(10, 0),
      attempt({ protocol: 'tcp', tcp: { connect_duration_ms: 3, remote_addr: '1.2.3.4:443' } }),
    ]);
    expect(hasRepeatedSamples(points)).toBe(false);
  });
});

describe('jitterRatio', () => {
  it('is p95 over p50 for a well-sampled point', () => {
    const [p] = buildSamplePoints([10, 10, 10, 10, 20].map((ms, i) => sample(ms, i)));
    const ratio = jitterRatio(p)!;
    expect(ratio).toBeGreaterThan(1);
    expect(ratio).toBeCloseTo(p.stats!.p95 / p.stats!.p50, 10);
  });

  it('is null rather than misleading when the median is not trustworthy', () => {
    const [p] = buildSamplePoints([sample(10, 0), sample(20, 1)]);
    expect(jitterRatio(p)).toBeNull();
  });
});
