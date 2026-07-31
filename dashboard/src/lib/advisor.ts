/**
 * Infrastructure advisor: turns the envelope's verdicts + the per-side
 * alternative-size pool (specs ⋈ prices, from /infra) into concrete
 * right-sizing suggestions — "more throughput for +$X/h" when a direction is
 * network-bound, "same throughput for −$Y/h" when everything has ample
 * headroom. Suggestions only ever cite catalog specs and resolved prices;
 * a missing price renders as "price unknown", never a made-up number.
 */
import type { AltVmSize, RunInfra, RunInfraSide } from '../api/types';
import type { DirectionAssessment } from './infra';
import { formatMbps } from './infra';

export interface Suggestion {
  kind: 'upsize' | 'downsize';
  /** Which side to change. */
  side: 'runner' | 'target';
  /** Current and suggested sizes. */
  from: string;
  to: string;
  /** New expected egress ceiling (Mbps) + its confidence. */
  toEgressMbps: number;
  toConfidence: 'documented' | 'estimated';
  /** Hourly delta in USD (positive = costs more); null when either price is unknown. */
  deltaUsdPerHour: number | null;
  /** Human line, ready to render. */
  text: string;
}

/** Upsize must at least this-many-× the current ceiling to be worth naming. */
const UPSIZE_MIN_FACTOR = 1.5;
/** Downsize only when every direction uses less than this share of its cap. */
const DOWNSIZE_MAX_UTILIZATION = 0.4;
/** A downsize suggestion must still leave this much headroom over measured. */
const DOWNSIZE_HEADROOM_FACTOR = 1.5;

function fmtDelta(delta: number | null): string {
  if (delta == null) return 'price unknown';
  const abs = Math.abs(delta).toFixed(3).replace(/0+$/, '').replace(/\.$/, '');
  return delta >= 0 ? `+$${abs}/h` : `−$${abs}/h`;
}

function delta(side: RunInfraSide, alt: AltVmSize): number | null {
  return side.hourly_usd != null && alt.hourly_usd != null
    ? alt.hourly_usd - side.hourly_usd
    : null;
}

/** Cheapest alternative clearing a minimum egress bar (price-unknowns last). */
function cheapestAbove(
  alts: AltVmSize[],
  minEgressMbps: number
): AltVmSize | null {
  const pool = alts.filter((a) => a.egress_mbps >= minEgressMbps);
  if (pool.length === 0) return null;
  return pool.sort(
    (a, b) =>
      (a.hourly_usd ?? Number.MAX_VALUE) - (b.hourly_usd ?? Number.MAX_VALUE) ||
      a.egress_mbps - b.egress_mbps
  )[0];
}

function upsizeFor(
  a: DirectionAssessment,
  side: RunInfraSide,
  sideName: 'runner' | 'target'
): Suggestion | null {
  if (!side.vm_size || !side.specs || !side.alternatives?.length) return null;
  const alt = cheapestAbove(
    side.alternatives,
    side.specs.egress_mbps * UPSIZE_MIN_FACTOR
  );
  if (!alt) return null;
  const d = delta(side, alt);
  return {
    kind: 'upsize',
    side: sideName,
    from: side.vm_size,
    to: alt.vm_size,
    toEgressMbps: alt.egress_mbps,
    toConfidence: alt.confidence,
    deltaUsdPerHour: d,
    text:
      `${a.direction} is at the ${sideName}'s egress cap — ${sideName} ` +
      `${alt.vm_size} (${fmtDelta(d)}) lifts the ceiling to ` +
      `${alt.confidence === 'estimated' ? '~' : ''}${formatMbps(alt.egress_mbps)}` +
      `${alt.confidence === 'documented' ? ' (doc)' : ' (est)'}` +
      `${alt.accelerated_networking && !side.specs.accelerated_networking ? ' + accelerated networking' : ''}.`,
  };
}

function downsizeFor(
  assessments: DirectionAssessment[],
  side: RunInfraSide,
  sideName: 'runner' | 'target'
): Suggestion | null {
  if (!side.vm_size || !side.specs || !side.alternatives?.length) return null;
  if (side.hourly_usd == null) return null;
  // Only the directions this side's egress actually carries.
  const carried = assessments.filter((a) =>
    sideName === 'target' ? a.direction === 'download' : a.direction === 'upload'
  );
  if (carried.length === 0) return null;
  if (!carried.every((a) => a.utilization != null && a.utilization < DOWNSIZE_MAX_UTILIZATION)) {
    return null;
  }
  const needed = Math.max(...carried.map((a) => a.measuredMbps)) * DOWNSIZE_HEADROOM_FACTOR;
  const alt = cheapestAbove(side.alternatives, needed);
  if (!alt || alt.hourly_usd == null || alt.hourly_usd >= side.hourly_usd) return null;
  const d = alt.hourly_usd - side.hourly_usd;
  const pct = Math.round((-d / side.hourly_usd) * 100);
  return {
    kind: 'downsize',
    side: sideName,
    from: side.vm_size,
    to: alt.vm_size,
    toEgressMbps: alt.egress_mbps,
    toConfidence: alt.confidence,
    deltaUsdPerHour: d,
    text:
      `${sideName} has ample network headroom — ${alt.vm_size} ` +
      `(${fmtDelta(d)}, −${pct}%) still clears the measured rate with ` +
      `${DOWNSIZE_HEADROOM_FACTOR}× margin.`,
  };
}

/** Derive suggestions from the run's assessments + infra sides. At most one
 * suggestion per side; upsizing a capped side outranks saving on it. */
export function adviseRun(
  assessments: DirectionAssessment[],
  infra: RunInfra | null
): Suggestion[] {
  if (!infra) return [];
  const out: Suggestion[] = [];
  for (const sideName of ['target', 'runner'] as const) {
    const side = infra[sideName];
    if (!side) continue;
    const bound = assessments.find(
      (a) => a.verdict === 'network-bound' && a.limitingSide === sideName
    );
    const s = bound
      ? upsizeFor(bound, side, sideName)
      : downsizeFor(assessments, side, sideName);
    if (s) out.push(s);
  }
  return out;
}
