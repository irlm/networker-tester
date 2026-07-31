// Tests for the infrastructure-envelope verdict engine (lib/infra.ts).
// The B2s scenario is the real one that motivated the feature (run e8fca1f2,
// 2026-07-31): download p50 73.2 MB/s ≈ 586 Mbps against a ~600 Mbps B2s
// egress estimate → network-bound at 98%, while upload rides the runner's
// higher egress. A steady-state regression here mislabels infra verdicts.

import { describe, it, expect } from 'vitest';
import type { LiveAttempt, RunEnvelope, RunInfra } from '../api/types';
import { assessRun, formatMbps, verdictLabel, wouldBenefitFromCeilingProbe } from './infra';

function throughputAttempt(
  protocol: string,
  payloadBytes: number,
  throughputMBs: number,
  extra?: Partial<NonNullable<LiveAttempt['tcp']>>
): LiveAttempt {
  return {
    attempt_id: 'a',
    run_id: 'r',
    sequence_num: 0,
    started_at: '2026-07-31T00:00:00Z',
    finished_at: '2026-07-31T00:00:01Z',
    success: true,
    protocol,
    http: {
      status_code: 200,
      ttfb_ms: 1,
      total_duration_ms: 100,
      negotiated_version: 'HTTP/1.1',
      payload_bytes: payloadBytes,
      throughput_mbps: throughputMBs,
    },
    tcp: extra
      ? { connect_duration_ms: 1, remote_addr: 'x', ...extra }
      : undefined,
  } as unknown as LiveAttempt;
}

const B2S_INFRA: RunInfra = {
  runner: {
    cloud: 'azure',
    vm_size: 'Standard_B2s',
    region: 'eastus',
    specs: {
      vcpus: 2, memory_gb: 4, egress_mbps: 600,
      confidence: 'estimated', accelerated_networking: false,
    },
  },
  target: {
    cloud: 'azure',
    vm_size: 'Standard_B2s',
    region: 'eastus',
    specs: {
      vcpus: 2, memory_gb: 4, egress_mbps: 600,
      confidence: 'estimated', accelerated_networking: false,
    },
  },
};

const IDLE_ENVELOPE: RunEnvelope = {
  client_info: { cpu_cores: 2 },
  client_load_before: { load_avg_1m: 0.01 },
  client_load_after: { load_avg_1m: 0.21 },
};

describe('assessRun — steady state + direction ceilings', () => {
  it('uses only the LARGEST payload p50 (small payloads are slow-start burst)', () => {
    const attempts = [
      throughputAttempt('download', 1024, 20),          // burst artifact
      throughputAttempt('download', 104857600, 73.2),   // steady state
      throughputAttempt('download', 104857600, 73.2),
    ];
    const [dl] = assessRun(attempts, B2S_INFRA, IDLE_ENVELOPE);
    expect(dl.payloadBytes).toBe(104857600);
    expect(dl.measuredMbps).toBeCloseTo(73.2 * 8, 5);   // MB/s → Mbps
  });

  it('download ceiling is the TARGET egress; upload ceiling the RUNNER egress', () => {
    const infra: RunInfra = {
      ...B2S_INFRA,
      runner: {
        ...B2S_INFRA.runner!,
        specs: { ...B2S_INFRA.runner!.specs!, egress_mbps: 12500 },
      },
    };
    const attempts = [
      throughputAttempt('download', 104857600, 73.2),
      throughputAttempt('upload', 104857600, 110.8),
    ];
    const [dl, ul] = assessRun(attempts, infra, IDLE_ENVELOPE);
    expect(dl.limitingSide).toBe('target');
    expect(dl.expectedMbps).toBe(600);
    expect(ul.limitingSide).toBe('runner');
    expect(ul.expectedMbps).toBe(12500);
  });
});

describe('assessRun — verdicts', () => {
  it('the real B2s run reads network-bound at ~98% of the target egress cap', () => {
    const [dl] = assessRun(
      [throughputAttempt('download', 104857600, 73.2)],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(dl.verdict).toBe('network-bound');
    expect(dl.utilization!).toBeGreaterThan(0.9);
    expect(verdictLabel(dl)).toContain('target egress cap');
  });

  it('well under the cap with idle CPU and clean path → headroom', () => {
    const [dl] = assessRun(
      [throughputAttempt('download', 104857600, 20)], // 160 of 600 Mbps
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(dl.verdict).toBe('headroom');
  });

  it('runner load at/over core count → cpu-bound (when not at the cap)', () => {
    const saturated: RunEnvelope = {
      client_info: { cpu_cores: 2 },
      client_load_after: { load_avg_1m: 2.4 },
    };
    const [dl] = assessRun(
      [throughputAttempt('download', 104857600, 20)],
      B2S_INFRA,
      saturated
    );
    expect(dl.verdict).toBe('cpu-bound');
  });

  it('retransmissions on the direction → path-bound (when not at the cap)', () => {
    const [ul] = assessRun(
      [throughputAttempt('upload', 104857600, 20, { total_retrans: 42 })],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(ul.verdict).toBe('path-bound');
  });

  it('no catalog spec → unknown, never an invented ceiling', () => {
    const noSpecs: RunInfra = {
      runner: { cloud: 'azure', vm_size: 'Standard_ZZ99', region: null, specs: null },
      target: { cloud: 'azure', vm_size: 'Standard_ZZ99', region: null, specs: null },
    };
    const [dl] = assessRun(
      [throughputAttempt('download', 104857600, 73.2)],
      noSpecs,
      IDLE_ENVELOPE
    );
    expect(dl.expectedMbps).toBeNull();
    expect(dl.verdict).toBe('unknown');
  });

  it('runs without throughput modes assess to nothing (panel renders no bars)', () => {
    expect(assessRun([], B2S_INFRA, IDLE_ENVELOPE)).toEqual([]);
  });
});

describe('assessRun — empirical ceiling (mthroughput, Phase 3)', () => {
  // The tester's capacity fields carry MB/s; 73.9 MB/s ≈ 591 Mbps measured
  // path capacity — the multi-stream truth that supersedes the ~600 estimate.
  const mthroughputAttempt = {
    attempt_id: 'mt',
    run_id: 'r',
    sequence_num: 99,
    started_at: '2026-07-31T00:00:00Z',
    finished_at: '2026-07-31T00:00:30Z',
    success: true,
    protocol: 'mthroughput',
    mthroughput: {
      capacity_down_mbps: 73.9,
      capacity_up_mbps: 112.4,
      conns_down: 4,
      conns_up: 3,
    },
  } as unknown as LiveAttempt;

  it('a measured capacity supersedes the catalog estimate', () => {
    const [dl] = assessRun(
      [throughputAttempt('download', 104857600, 73.2), mthroughputAttempt],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(dl.confidence).toBe('measured');
    expect(dl.expectedMbps).toBeCloseTo(73.9 * 8, 5);
    expect(dl.limitingSide).toBeNull();       // path capacity, not one side
    expect(dl.verdict).toBe('network-bound'); // 586/591 ≈ 99%
    expect(verdictLabel(dl)).toContain('measured path capacity');
  });

  it('per-direction: upload uses capacity_up, independent of download', () => {
    const [, ul] = assessRun(
      [
        throughputAttempt('download', 104857600, 73.2),
        throughputAttempt('upload', 104857600, 110.8),
        mthroughputAttempt,
      ],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(ul.expectedMbps).toBeCloseTo(112.4 * 8, 5);
    expect(ul.confidence).toBe('measured');
  });

  it('no mthroughput data → falls back to the catalog spec ceiling', () => {
    const [dl] = assessRun(
      [throughputAttempt('download', 104857600, 73.2)],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(dl.confidence).toBe('estimated');
    expect(dl.expectedMbps).toBe(600);
  });

  it('hint fires only for estimate-capped runs without a measured ceiling', () => {
    const capped = assessRun(
      [throughputAttempt('download', 104857600, 73.2)],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(wouldBenefitFromCeilingProbe(capped)).toBe(true);

    const measured = assessRun(
      [throughputAttempt('download', 104857600, 73.2), mthroughputAttempt],
      B2S_INFRA,
      IDLE_ENVELOPE
    );
    expect(wouldBenefitFromCeilingProbe(measured)).toBe(false);
  });
});

describe('formatMbps', () => {
  it('scales to Gbps at 1000', () => {
    expect(formatMbps(586)).toBe('586 Mbps');
    expect(formatMbps(12500)).toBe('12.5 Gbps');
  });
});
