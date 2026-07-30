// Provider-block resolution — the exact bug from the 2026-07-30 report: a
// cloud target's Region/VM size/OS rendered as "—" because those facts live
// under ep.<provider> (the wizard/orchestrator shape), not at the endpoint
// top level. Mirrors the server's ParseEndpointSpecs rules.

import { describe, it, expect } from 'vitest';
import { endpointField, endpointIdentity, testSupportOf } from './deploy-endpoint';
import type { DeployEndpoint } from '../api/types';

// Copied from the prod deployment that rendered "—" (top level null, azure block full).
const PROD_AZURE: DeployEndpoint = {
  provider: 'azure',
  region: null,
  vm_size: null,
  os: null,
  http_stacks: ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'],
  azure: { os: 'linux', region: 'eastus', vm_name: 'nwk-ep-ubuntu-edne', vm_size: 'Standard_B2s' },
};

describe('endpointField / endpointIdentity', () => {
  it('resolves cloud facts from the provider block when top level is null', () => {
    expect(endpointIdentity(PROD_AZURE)).toEqual({
      region: 'eastus',
      vmSize: 'Standard_B2s',
      os: 'linux',
      vmName: 'nwk-ep-ubuntu-edne',
    });
  });

  it('top level wins over the provider block', () => {
    const ep: DeployEndpoint = { ...PROD_AZURE, region: 'westus2' };
    expect(endpointField(ep, 'region', 'zone')).toBe('westus2');
  });

  it('gcp zone falls back for region; machine_type for size', () => {
    const ep: DeployEndpoint = {
      provider: 'gcp',
      gcp: { zone: 'us-central1-a', machine_type: 'e2-small', os: 'linux' },
    };
    expect(endpointIdentity(ep)).toMatchObject({ region: 'us-central1-a', vmSize: 'e2-small' });
  });

  it('lan/upgrade endpoints with no provider block resolve top level only', () => {
    const ep: DeployEndpoint = { provider: 'lan', ip: '10.0.0.5' };
    expect(endpointIdentity(ep)).toEqual({ region: null, vmSize: null, os: null, vmName: null });
  });
});

describe('testSupportOf', () => {
  it('reports stacks and the built-in /api fallback honestly', () => {
    expect(testSupportOf(PROD_AZURE)).toBe(
      'network · throughput · page-load · stack comparison · apibench: built-in /api',
    );
  });

  it('names the language when a reference API is installed', () => {
    expect(testSupportOf({ ...PROD_AZURE, languages: ['go'] })).toContain('apibench: go');
  });
});
