import type { CloudStatus } from '../../api/types';

interface CloudProviderStatusProps {
  cloudStatus: CloudStatus | null;
  loading?: boolean;
  availableClouds?: string[];
}

const ALL_PROVIDERS = ['azure', 'aws', 'gcp', 'ssh'] as const;

const LABEL: Record<string, string> = {
  azure: 'Azure',
  aws: 'AWS',
  gcp: 'GCP',
  ssh: 'SSH/LAN',
  oci: 'Oracle',
  digitalocean: 'DigitalOcean',
  linode: 'Linode',
  hetzner: 'Hetzner',
  vultr: 'Vultr',
};

function statusText(s: { available: boolean; authenticated: boolean; account: string | null }): string {
  if (!s.available) return 'not installed';
  if (!s.authenticated) return 'not authenticated';
  if (s.account) return s.account;
  return 'ready';
}

function statusDot(s: { available: boolean; authenticated: boolean }): string {
  if (s.authenticated) return 'bg-green-400';
  if (s.available) return 'bg-yellow-400';
  return 'bg-gray-600';
}

/**
 * Compact cloud provider status display. Adapts layout:
 * - ≤4 providers: 2-column card grid with status detail
 * - 5-8 providers: 3-column compact grid
 * - 9+ providers: horizontal inline strip (dots + names only)
 */
export function CloudProviderStatus({ cloudStatus, loading, availableClouds }: CloudProviderStatusProps) {
  if (loading) {
    return (
      <div className="border border-gray-800 rounded p-3 mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">Cloud providers</div>
        <p className="text-xs text-gray-500">Checking providers...</p>
      </div>
    );
  }

  if (!cloudStatus) {
    return (
      <div className="border border-gray-800 rounded p-3 mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">Cloud providers</div>
        <p className="text-xs text-yellow-400/70">Could not detect cloud providers.</p>
        {availableClouds && availableClouds.length === 0 && (
          <p className="text-xs text-gray-600 mt-1">Add a cloud account in Settings → Cloud.</p>
        )}
      </div>
    );
  }

  // Build list from CloudStatus keys (known) + any extra availableClouds
  const providers = [...ALL_PROVIDERS];
  if (availableClouds) {
    for (const c of availableClouds) {
      if (!providers.includes(c as typeof ALL_PROVIDERS[number])) {
        providers.push(c as typeof ALL_PROVIDERS[number]);
      }
    }
  }

  const count = providers.length;

  // 9+ providers: ultra-compact horizontal strip
  if (count > 8) {
    return (
      <div className="border border-gray-800 rounded p-3 mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">
          Cloud providers ({providers.filter(p => cloudStatus[p as keyof CloudStatus]?.authenticated).length}/{count} connected)
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-1">
          {providers.map(p => {
            const s = cloudStatus[p as keyof CloudStatus];
            const connected = s?.authenticated ?? availableClouds?.includes(p) ?? false;
            return (
              <div key={p} className="flex items-center gap-1.5 text-xs" title={s ? statusText(s) : 'unknown'}>
                <span className={`w-1.5 h-1.5 rounded-full ${s ? statusDot(s) : connected ? 'bg-green-400' : 'bg-gray-600'}`} />
                <span className={connected ? 'text-gray-300' : 'text-gray-600'}>
                  {LABEL[p] || p}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    );
  }

  // 5-8 providers: 3-column compact cards
  const cols = count > 4 ? 'grid-cols-3' : 'grid-cols-2';

  return (
    <div className="border border-gray-800 rounded p-3 mb-4">
      <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">Cloud providers</div>
      <div className={`grid ${cols} gap-2`}>
        {providers.map(p => {
          const s = cloudStatus[p as keyof CloudStatus];
          const connected = s?.authenticated ?? availableClouds?.includes(p) ?? false;
          return (
            <div key={p} className="bg-[var(--bg-base)] border border-gray-800 rounded p-2">
              <div className="flex items-center gap-1.5 mb-0.5">
                <span className={`w-1.5 h-1.5 rounded-full ${s ? statusDot(s) : connected ? 'bg-green-400' : 'bg-gray-600'}`} />
                <span className={`text-xs font-medium ${connected ? 'text-gray-200' : 'text-gray-500'}`}>
                  {LABEL[p] || p}
                </span>
              </div>
              <p className="text-[10px] text-gray-600 pl-3">
                {s ? statusText(s) : connected ? 'connected' : 'not configured'}
              </p>
            </div>
          );
        })}
      </div>
      {availableClouds && availableClouds.length === 0 && (
        <p className="text-xs text-yellow-400/70 mt-2">
          No cloud accounts connected. Add one in Settings → Cloud.
        </p>
      )}
    </div>
  );
}
