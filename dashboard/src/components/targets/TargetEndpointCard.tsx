import type { DeployEndpoint, DeploymentCostEstimate } from '../../api/types';
import { DetailList } from '../common/DetailList';
import { endpointIdentity, testSupportOf } from '../../lib/deploy-endpoint';

interface TargetEndpointCardProps {
  ep: DeployEndpoint;
  index: number;
  /** Live IP from the deployment record (endpoint_ips[i]), if any. */
  ip?: string | null;
  cost?: DeploymentCostEstimate['endpoints'][number];
  /** Render without the border/box (drawer context provides its own section). */
  bare?: boolean;
}

/**
 * One deployment endpoint's identity core — cloud · region · VM size · OS ·
 * VM name · IP · stacks · test support · cost — shared by the target detail
 * page and the target drawer so the two can never drift. Field values resolve
 * across the endpoint top level AND the provider block (cloud configs nest
 * region/vm_size/os under ep.<provider>).
 */
export function TargetEndpointCard({ ep, index, ip, cost, bare }: TargetEndpointCardProps) {
  const id = endpointIdentity(ep);
  const body = (
    <>
      <p className="text-xs text-gray-200 font-medium mb-2">
        {ep.label ?? cost?.label ?? `endpoint ${index + 1}`}
      </p>
      <DetailList
        rows={[
          { label: 'Cloud', value: ep.provider },
          { label: 'Region', value: id.region },
          { label: 'VM size', value: id.vmSize },
          { label: 'OS', value: id.os },
          { label: 'VM name', value: id.vmName },
          { label: 'IP', value: ep.ip ?? ip },
          ...(ep.http_stacks?.length
            ? [{ label: 'Stacks', value: ep.http_stacks.join(', ') }]
            : []),
          { label: 'Test support', value: testSupportOf(ep) },
          ...(cost?.hourly_usd != null
            ? [
                { label: 'Hourly', value: `$${cost.hourly_usd.toFixed(3)}` },
                { label: 'Monthly (always-on)', value: `$${(cost.monthly_usd ?? 0).toFixed(2)}`, accent: true },
              ]
            : []),
        ]}
      />
    </>
  );
  return bare ? <div>{body}</div> : <div className="border border-gray-800 rounded-lg p-3">{body}</div>;
}
