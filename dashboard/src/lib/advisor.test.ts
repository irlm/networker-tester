// Tests for the infrastructure advisor (lib/advisor.ts). The upsize case is
// the real B2s scenario: download network-bound at ~98% of a ~600 Mbps target
// egress estimate → the advisor must name the cheapest size clearing 1.5× the
// current ceiling WITH its real price delta — and say "price unknown" rather
// than invent economics when a price is missing.

import { describe, it, expect } from 'vitest';
import type { AltVmSize, RunInfra, RunInfraSide } from '../api/types';
import type { DirectionAssessment } from './infra';
import { adviseRun } from './advisor';

function alt(
  vm_size: string,
  egress_mbps: number,
  hourly_usd: number | null,
  confidence: 'documented' | 'estimated' = 'estimated',
  accel = false
): AltVmSize {
  return {
    vm_size, egress_mbps, hourly_usd, confidence,
    vcpus: 2, memory_gb: 8, accelerated_networking: accel,
  };
}

function side(
  vm_size: string,
  egress: number,
  hourly: number | null,
  alternatives: AltVmSize[]
): RunInfraSide {
  return {
    cloud: 'azure',
    vm_size,
    region: 'eastus',
    hourly_usd: hourly,
    alternatives,
    specs: {
      vcpus: 2, memory_gb: 4, egress_mbps: egress,
      confidence: 'estimated', accelerated_networking: false,
    },
  };
}

const AZURE_ALTS: AltVmSize[] = [
  alt('Standard_F2s_v2', 875, 0.085, 'estimated', true),      // below 1.5× bar
  alt('Standard_D2s_v3', 1000, 0.096, 'estimated', true),     // cheapest above bar
  alt('Standard_D2s_v5', 12500, null, 'documented', true),    // price unknown
  alt('Standard_D4s_v5', 12500, 0.192, 'documented', true),
];

function boundDownload(measuredMbps = 586): DirectionAssessment {
  return {
    direction: 'download', measuredMbps, payloadBytes: 104857600,
    expectedMbps: 600, confidence: 'estimated', limitingSide: 'target',
    utilization: measuredMbps / 600, verdict: 'network-bound',
  };
}

describe('adviseRun — upsize (the real B2s case)', () => {
  it('names the cheapest size clearing 1.5× the ceiling, with the real delta', () => {
    const infra: RunInfra = {
      runner: null,
      target: side('Standard_B2s', 600, 0.0416, AZURE_ALTS),
    };
    const [s] = adviseRun([boundDownload()], infra);
    expect(s.kind).toBe('upsize');
    expect(s.side).toBe('target');
    expect(s.to).toBe('Standard_D2s_v3');           // $0.096 beats $0.192; F2s below bar
    expect(s.deltaUsdPerHour).toBeCloseTo(0.0544, 4);
    expect(s.text).toContain('target');
    expect(s.text).toContain('+$0.054');
  });

  it('a price-unknown winner says so instead of inventing a number', () => {
    const onlyUnknown: RunInfra = {
      runner: null,
      target: side('Standard_B2s', 600, 0.0416, [
        alt('Standard_D2s_v5', 12500, null, 'documented', true),
      ]),
    };
    const [s] = adviseRun([boundDownload()], onlyUnknown);
    expect(s.to).toBe('Standard_D2s_v5');
    expect(s.deltaUsdPerHour).toBeNull();
    expect(s.text).toContain('price unknown');
  });

  it('no alternative clears the bar → no suggestion', () => {
    const infra: RunInfra = {
      runner: null,
      target: side('Standard_D8s_v5', 12500, 0.384, [alt('Standard_B2s', 600, 0.0416)]),
    };
    expect(adviseRun([{ ...boundDownload(12000), expectedMbps: 12500, utilization: 0.96 }], infra))
      .toEqual([]);
  });
});

describe('adviseRun — downsize', () => {
  const headroomDownload: DirectionAssessment = {
    direction: 'download', measuredMbps: 400, payloadBytes: 104857600,
    expectedMbps: 12500, confidence: 'documented', limitingSide: 'target',
    utilization: 400 / 12500, verdict: 'headroom',
  };

  it('ample headroom → cheapest size still clearing 1.5× measured, with savings', () => {
    const infra: RunInfra = {
      runner: null,
      target: side('Standard_D4s_v5', 12500, 0.192, [
        alt('Standard_B2s', 600, 0.0416),          // ≥ 400×1.5=600 → qualifies
        alt('Standard_B1s', 250, 0.0104),          // too small
      ]),
    };
    const [s] = adviseRun([headroomDownload], infra);
    expect(s.kind).toBe('downsize');
    expect(s.to).toBe('Standard_B2s');
    expect(s.deltaUsdPerHour).toBeCloseTo(-0.1504, 4);
    expect(s.text).toContain('−$0.15');
  });

  it('never downsizes a side whose direction is above the utilization gate', () => {
    const busy = { ...headroomDownload, utilization: 0.6 };
    const infra: RunInfra = {
      runner: null,
      target: side('Standard_D4s_v5', 12500, 0.192, [alt('Standard_B2s', 600, 0.0416)]),
    };
    expect(adviseRun([busy], infra)).toEqual([]);
  });

  it('a network-bound side gets the upsize, not a downsize', () => {
    const infra: RunInfra = {
      runner: null,
      target: side('Standard_B2s', 600, 0.0416, AZURE_ALTS),
    };
    const out = adviseRun([boundDownload()], infra);
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe('upsize');
  });
});
