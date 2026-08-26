// Burst sampling (#782 P2): median + spread for every measurement POINT of a
// run. A point is one (target URL × mode × payload) triple — the thing a
// single number used to be reported for. Repeats of a point (a `--samples N`
// burst, or a `--runs N` loop, or both) are its samples, and the median of
// them is the honest headline: one cold DNS cache or one TCP retransmit no
// longer decides what the point "measured".
//
// Honesty rules, deliberately load-bearing:
//   * a FAILED sample is a failed sample, never a missing one — it is counted
//     in `sampleCount`, excluded from the timing stats, and reported;
//   * a median needs samples. Below MIN_SAMPLES_FOR_MEDIAN usable samples the
//     row is `underSampled` and the UI must say "1 sample" instead of dressing
//     one reading up as a median;
//   * nothing is interpolated or synthesised for a sample that did not run.
//
// Pure functions; rendered by RunDetailPage's BurstSamplingTable.

import type { LiveAttempt } from '../../api/types';

/**
 * The attempt's failure class, from whichever transport carried it: the live
 * stream nests it under `error.category`, the REST attempts endpoint returns
 * it flat as `error_category` (since v0.28.302). Null when the attempt
 * succeeded or the payload predates the field.
 */
export function attemptErrorCategory(a: LiveAttempt): string | null {
  return a.error?.category ?? a.error_category ?? null;
}

/**
 * The category the tester writes when it did NOT run a probe because the
 * target does not offer the protocol — today only the HTTP/3 pre-flight
 * (v0.28.301), which skips h3 modes at an origin advertising no `Alt-Svc:
 * h3=`. Such a sample is unsuccessful (nothing was measured) but it is NOT a
 * failure of the target or the network, and must never be counted as one.
 */
export const NOT_OFFERED = 'unsupported';

/** Whether this attempt was skipped as not-offered rather than actually run. */
export function isNotOffered(a: LiveAttempt): boolean {
  return !a.success && attemptErrorCategory(a) === NOT_OFFERED;
}
import {
  attemptPayloadBytes,
  computeStats,
  primaryMetricLabel,
  primaryMetricValue,
  type Stats,
} from '../../lib/analysis';

/**
 * Usable samples a median needs before it is a median rather than a reading
 * with extra steps. Two samples give a midpoint, not a middle — and no p95 at
 * all — so three is the floor.
 */
export const MIN_SAMPLES_FOR_MEDIAN = 3;

export interface SamplePoint {
  /** Stable react key / identity: url|protocol|payload. */
  key: string;
  /** '' when the attempts carry no target_url (single-URL or pre-#782 run). */
  targetUrl: string;
  protocol: string;
  payloadBytes: number | null;
  /** What the median is a median OF ("Connect ms", "Throughput MB/s", …). */
  metricLabel: string;
  /** Every published attempt for this point — burst samples AND run repeats. */
  sampleCount: number;
  /** Samples that succeeded and carried the point's primary metric. */
  usableCount: number;
  /** Samples that actually ran and failed. EXCLUDES not-offered samples (see
   *  {@link notOfferedCount}), which were never run — counting those as
   *  failures is what made an h3 point against a target without HTTP/3 read as
   *  a network fault. `sampleCount - usableCount` also counts successes whose
   *  metric was absent, so this is reported separately. */
  failedCount: number;
  /** Samples the tester declined to run because the target does not offer the
   *  protocol (`unsupported`). Unsuccessful, but not a failure of anything. */
  notOfferedCount: number;
  /** Burst width the tester used for this point: max(sample_index) + 1.
   *  1 = the repeats came from `--runs`, not from a `--samples` burst. */
  burstSize: number;
  /** Stats over the usable samples; null when none were usable. */
  stats: Stats | null;
  /** True when `stats` rests on fewer than {@link MIN_SAMPLES_FOR_MEDIAN}
   *  samples — the caller must label the number instead of calling it a
   *  median. */
  underSampled: boolean;
}

/**
 * Spread as a ratio of p95 to p50 — how much worse the tail is than the middle
 * for this point. 1.0 = perfectly consistent. Null when there is no honest
 * median to compare against (under-sampled, or a zero/negative p50).
 */
export function jitterRatio(point: SamplePoint): number | null {
  if (point.underSampled || !point.stats || point.stats.p50 <= 0) return null;
  return point.stats.p95 / point.stats.p50;
}

/**
 * Group a run's attempts into measurement points and compute each point's
 * median + spread. Points are ordered by URL, then mode, then payload, so a
 * URL-set run reads as one block per URL.
 */
export function buildSamplePoints(attempts: LiveAttempt[]): SamplePoint[] {
  const groups = new Map<string, LiveAttempt[]>();
  for (const a of attempts) {
    const payload = attemptPayloadBytes(a);
    const key = `${a.target_url ?? ''}|${a.protocol}|${payload ?? ''}`;
    const bucket = groups.get(key);
    if (bucket) bucket.push(a);
    else groups.set(key, [a]);
  }

  const points: SamplePoint[] = [];
  for (const [key, group] of groups) {
    const first = group[0];
    const values = group
      .filter((a) => a.success)
      .map(primaryMetricValue)
      .filter((v): v is number => v != null);
    const stats = computeStats(values);
    points.push({
      key,
      targetUrl: first.target_url ?? '',
      protocol: first.protocol,
      payloadBytes: attemptPayloadBytes(first),
      metricLabel: primaryMetricLabel(first.protocol),
      sampleCount: group.length,
      usableCount: values.length,
      failedCount: group.filter((a) => !a.success && !isNotOffered(a)).length,
      notOfferedCount: group.filter(isNotOffered).length,
      burstSize: group.reduce((max, a) => Math.max(max, (a.sample_index ?? 0) + 1), 1),
      stats,
      underSampled: values.length < MIN_SAMPLES_FOR_MEDIAN,
    });
  }

  points.sort(
    (a, b) =>
      a.targetUrl.localeCompare(b.targetUrl) ||
      a.protocol.localeCompare(b.protocol) ||
      (a.payloadBytes ?? 0) - (b.payloadBytes ?? 0),
  );
  return points;
}

/**
 * Whether the run has anything for the median/spread section to say: at least
 * one point was measured more than once. A run of single-shot points has no
 * median to report and the section stays hidden rather than rendering a table
 * of n=1 rows.
 */
export function hasRepeatedSamples(points: SamplePoint[]): boolean {
  return points.some((p) => p.sampleCount > 1);
}

/** True when any point's repeats came from an actual `--samples` burst. */
export function usedBurstSampling(points: SamplePoint[]): boolean {
  return points.some((p) => p.burstSize > 1);
}
