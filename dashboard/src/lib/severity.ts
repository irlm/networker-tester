/**
 * The one latency/intensity ramp (severity-v2 pick): cyan encodes
 * magnitude, red is reserved for breach. Replaces the page-local
 * green→orange→red ramps that used to disagree with each other
 * (PerfLogPage, ApiLogPanel, RunDetailPage p99, ShareViewPage).
 *
 * Thresholds are the caller's domain knowledge; this module only owns the
 * mapping from bucket to token so every surface draws the same ramp.
 */
export interface RampThresholds {
  /** Values below this are quiet (s2). */
  mid: number;
  /** Values below this are elevated (s3); at or above: peak (s4). */
  high: number;
  /** At or above this the value is a breach (red). */
  breach: number;
}

export function rampTextClass(value: number, t: RampThresholds): string {
  if (value >= t.breach) return 'text-breach';
  if (value >= t.high) return 'text-s4';
  if (value >= t.mid) return 'text-s3';
  return 'text-s2';
}

/** Inline style variant for non-Tailwind sinks (SVG fills, style props). */
export function rampColor(value: number, t: RampThresholds): string {
  if (value >= t.breach) return 'var(--color-breach)';
  if (value >= t.high) return 'var(--color-s4)';
  if (value >= t.mid) return 'var(--color-s3)';
  return 'var(--color-s2)';
}
