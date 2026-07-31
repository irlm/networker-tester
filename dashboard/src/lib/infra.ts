/**
 * Infrastructure envelope: expected-vs-measured throughput per direction and
 * the bottleneck verdict ("network-bound / cpu-bound / path-bound / headroom").
 *
 * Ceiling rule (mirrors the server's VmNetworkSpecs doc): clouds cap VM
 * *egress*, not ingress — so the infrastructure ceiling of a download is the
 * TARGET's egress and of an upload the RUNNER's egress.
 *
 * Steady-state rule: throughput of short transfers is dominated by TCP
 * slow-start burst, so the measured figure per direction is the p50 of the
 * LARGEST payload group only.
 */
import type { LiveAttempt, RunEnvelope, RunInfra } from '../api/types';
import { computeStats, groupByProtocolAndPayload } from './analysis';

export type BottleneckKind =
  | 'network-bound'
  | 'cpu-bound'
  | 'path-bound'
  | 'headroom'
  | 'unknown';

export interface DirectionAssessment {
  direction: 'download' | 'upload';
  /** Steady-state measured rate in Mbps (largest payload's p50 × 8). */
  measuredMbps: number;
  /** The payload size the steady-state figure comes from. */
  payloadBytes: number;
  /** Infrastructure ceiling in Mbps. When the run carried an mthroughput
   * attempt this is the EMPIRICAL multi-stream capacity ('measured'); else
   * the sending side's catalog egress expectation. */
  expectedMbps: number | null;
  confidence: 'measured' | 'documented' | 'estimated' | null;
  /** Which side's egress is the ceiling for this direction (spec ceilings
   * only; a measured path capacity isn't attributed to one side). */
  limitingSide: 'target' | 'runner' | null;
  /** measured / expected (0..1+), null when no ceiling is known. */
  utilization: number | null;
  verdict: BottleneckKind;
}

/** At/above this share of the ceiling we call the direction network-bound. */
const NETWORK_BOUND_UTILIZATION = 0.8;

const DOWNLOAD_PROTOCOLS = /^(download|webdownload)/;
const UPLOAD_PROTOCOLS = /^(upload|webupload)/;

/** Largest-payload p50 throughput (MB/s → Mbps) for one direction. */
function steadyStateMbps(
  attempts: LiveAttempt[],
  match: RegExp
): { mbps: number; payloadBytes: number } | null {
  const groups = groupByProtocolAndPayload(
    attempts.filter((a) => a.success && match.test(a.protocol))
  );
  let best: { mbps: number; payloadBytes: number } | null = null;
  for (const [key, atts] of groups) {
    const payloadStr = key.includes(':') ? key.split(':')[1] : null;
    const payloadBytes = payloadStr ? parseInt(payloadStr, 10) : 0;
    if (best && payloadBytes <= best.payloadBytes) continue;
    const values = atts
      .map((a) => a.http?.throughput_mbps)
      .filter((v): v is number => v != null);
    const stats = computeStats(values);
    if (!stats) continue;
    best = { mbps: stats.p50 * 8, payloadBytes }; // MB/s → Mbps
  }
  return best;
}

/** Peak 1-minute load ≥ core count on either envelope sample → CPU saturated. */
function runnerCpuSaturated(envelope?: RunEnvelope | null): boolean {
  const cores = envelope?.client_info?.cpu_cores;
  if (cores == null || cores <= 0) return false;
  const peak = Math.max(
    envelope?.client_load_before?.load_avg_1m ?? 0,
    envelope?.client_load_after?.load_avg_1m ?? 0
  );
  return peak >= cores;
}

/** Any retransmissions on the direction's attempts → lossy path signal.
 * (TCP retransmit counters are sender-side, so this primarily catches the
 * upload direction; a lossy download path shows as loss on the udp probe.) */
function pathLossSignal(attempts: LiveAttempt[], match: RegExp): boolean {
  const retrans = attempts.some(
    (a) => match.test(a.protocol) && (a.tcp?.total_retrans ?? 0) > 0
  );
  const udpLoss = attempts.some(
    (a) => a.protocol === 'udp' && (a.udp?.loss_percent ?? 0) > 1
  );
  return retrans || udpLoss;
}

/** Multi-stream measured capacity for a direction, in Mbps, from the run's
 * newest successful mthroughput attempt (V005 persistence). The tester's
 * capacity fields carry MB/s — ×8 here. Null when the run didn't include the
 * mode or the direction's stage failed. */
export function empiricalCapacityMbps(
  attempts: LiveAttempt[],
  direction: 'download' | 'upload'
): number | null {
  for (let i = attempts.length - 1; i >= 0; i--) {
    const a = attempts[i];
    if (!a.success || a.protocol !== 'mthroughput' || !a.mthroughput) continue;
    const cap =
      direction === 'download'
        ? a.mthroughput.capacity_down_mbps
        : a.mthroughput.capacity_up_mbps;
    if (cap != null) return cap * 8; // MB/s → Mbps
  }
  return null;
}

function assessDirection(
  direction: 'download' | 'upload',
  attempts: LiveAttempt[],
  infra: RunInfra | null,
  envelope?: RunEnvelope | null
): DirectionAssessment | null {
  const match = direction === 'download' ? DOWNLOAD_PROTOCOLS : UPLOAD_PROTOCOLS;
  const steady = steadyStateMbps(attempts, match);
  if (!steady) return null;

  // Sending side per direction: download ← target egress, upload ← runner egress.
  const limiting = direction === 'download' ? infra?.target : infra?.runner;
  const specLimitingSide = direction === 'download' ? ('target' as const) : ('runner' as const);
  // The empirical multi-stream capacity, when the run measured it, supersedes
  // the catalog estimate — measured truth over spec sheet.
  const empirical = empiricalCapacityMbps(attempts, direction);
  const expectedMbps = empirical ?? limiting?.specs?.egress_mbps ?? null;
  const confidence: DirectionAssessment['confidence'] =
    empirical != null ? 'measured' : limiting?.specs?.confidence ?? null;
  const limitingSide = empirical != null ? null : specLimitingSide;
  const utilization = expectedMbps ? steady.mbps / expectedMbps : null;

  let verdict: BottleneckKind;
  if (utilization != null && utilization >= NETWORK_BOUND_UTILIZATION) {
    verdict = 'network-bound';
  } else if (runnerCpuSaturated(envelope)) {
    verdict = 'cpu-bound';
  } else if (pathLossSignal(attempts, match)) {
    verdict = 'path-bound';
  } else if (utilization != null) {
    verdict = 'headroom';
  } else {
    verdict = 'unknown';
  }

  return {
    direction,
    measuredMbps: steady.mbps,
    payloadBytes: steady.payloadBytes,
    expectedMbps,
    confidence,
    limitingSide: expectedMbps != null ? limitingSide : null,
    utilization,
    verdict,
  };
}

/** True when the run carried no mthroughput data but a direction sits on an
 * ESTIMATED spec ceiling — the case where adding the Multi-Conn mode would
 * replace guesswork with a measured capacity. Drives the panel's hint row. */
export function wouldBenefitFromCeilingProbe(
  assessments: DirectionAssessment[]
): boolean {
  return (
    assessments.length > 0 &&
    assessments.every((a) => a.confidence !== 'measured') &&
    assessments.some(
      (a) => a.verdict === 'network-bound' && a.confidence === 'estimated'
    )
  );
}

/** Assess every direction the run actually measured (empty when the run has
 * no throughput modes — the envelope panel renders nothing then). */
export function assessRun(
  attempts: LiveAttempt[],
  infra: RunInfra | null,
  envelope?: RunEnvelope | null
): DirectionAssessment[] {
  return (['download', 'upload'] as const)
    .map((d) => assessDirection(d, attempts, infra, envelope))
    .filter((a): a is DirectionAssessment => a != null);
}

export function verdictLabel(a: DirectionAssessment): string {
  switch (a.verdict) {
    case 'network-bound':
      return a.confidence === 'measured'
        ? 'network-bound — at the measured path capacity (multi-stream)'
        : `network-bound — at ${a.limitingSide} egress cap`;
    case 'cpu-bound':
      return 'cpu-bound — runner CPU saturated';
    case 'path-bound':
      return 'path-bound — loss/retransmissions on the path';
    case 'headroom':
      return 'headroom — app/protocol limited, not infrastructure';
    default:
      return 'no spec for this size — ceiling unknown';
  }
}

export function formatMbps(mbps: number): string {
  return mbps >= 1000 ? `${(mbps / 1000).toFixed(1)} Gbps` : `${Math.round(mbps)} Mbps`;
}
