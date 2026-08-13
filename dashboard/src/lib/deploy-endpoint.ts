import type { DeployEndpoint, DeployProviderBlock, TargetCapabilityReport } from '../api/types';

/**
 * Resolve an endpoint fact across the top level and the provider block —
 * first present key wins, top level before nested. Cloud deploys nest
 * region/vm_size/os under `ep.<provider>` (the wizard + provisioning
 * orchestrator shape); LAN/upgrade deploys put them top-level. Mirrors the
 * server's ParseEndpointSpecs so the UI and the cost endpoint can't disagree.
 */
export function endpointField(
  ep: DeployEndpoint,
  ...keys: (keyof DeployProviderBlock)[]
): string | null {
  const block = (ep as unknown as Record<string, unknown>)[ep.provider] as
    | DeployProviderBlock
    | undefined;
  for (const k of keys) {
    const v = (ep as unknown as Record<string, unknown>)[k];
    if (typeof v === 'string' && v) return v;
  }
  for (const k of keys) {
    const v = block?.[k];
    if (typeof v === 'string' && v) return v;
  }
  return null;
}

/** The identity facts both target views (detail page + drawer) render. */
export function endpointIdentity(ep: DeployEndpoint) {
  return {
    region: endpointField(ep, 'region', 'zone'),
    vmSize: endpointField(ep, 'vm_size', 'instance_type', 'machine_type'),
    os: endpointField(ep, 'os'),
    vmName: endpointField(ep, 'vm_name', 'instance_name'),
  };
}

/** One-line test-support summary: what this target can honestly serve. */
export function testSupportOf(ep: DeployEndpoint): string {
  return [
    'network · throughput · page-load',
    ep.http_stacks?.length ? 'stack comparison' : null,
    ep.languages?.length
      ? `apibench: ${ep.languages.join(', ')}`
      : 'apibench: built-in /api',
  ].filter(Boolean).join(' · ');
}

/**
 * Test-support summary from the target's LIVE capability self-report
 * (/health `services` → supported modes). Falls back to null when the target
 * gave no report (unreachable / pre-0.28.202) — callers then render the
 * config-derived `testSupportOf` instead of a fabricated list.
 */
export function liveTestSupportOf(
  report: TargetCapabilityReport | undefined,
  ep: DeployEndpoint,
): { summary: string; disabled: string[] } | null {
  if (!report?.supported_modes) return null;
  const has = (m: string) => report.supported_modes!.includes(m);
  const summary = [
    'network',
    has('download') ? 'throughput' : null,
    has('udp') ? 'udp' : null,
    has('stamp') ? 'stamp' : null,
    has('websocket') ? 'websocket' : null,
    has('pageload') ? 'page-load' : null,
    ep.http_stacks?.length ? 'stack comparison' : null,
    ep.languages?.length
      ? `apibench: ${ep.languages.join(', ')}`
      : 'apibench: built-in /api',
  ].filter(Boolean).join(' · ');
  const disabled = (report.unsupported_modes ?? []).map(u => u.mode);
  return { summary, disabled };
}
