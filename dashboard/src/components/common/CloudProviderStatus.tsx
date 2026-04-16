/**
 * Cloud provider status based on project-scoped cloud accounts (DB).
 * NO CLI status — the web dashboard never uses server-side CLI credentials.
 */

interface CloudAccount {
  account_id: string;
  name: string;
  provider: string;
  status: string;
  last_validated?: string;
}

interface CloudProviderStatusProps {
  accounts: CloudAccount[];
  loading?: boolean;
}

const LABEL: Record<string, string> = {
  azure: 'Azure',
  aws: 'AWS',
  gcp: 'GCP',
  oci: 'Oracle',
  digitalocean: 'DigitalOcean',
  linode: 'Linode',
  hetzner: 'Hetzner',
  vultr: 'Vultr',
};

function statusDot(status: string): string {
  if (status === 'active') return 'bg-green-400';
  if (status === 'validating') return 'bg-yellow-400 motion-safe:animate-pulse';
  return 'bg-red-400';
}

function statusLabel(status: string): string {
  if (status === 'active') return 'connected';
  if (status === 'validating') return 'validating...';
  if (status === 'error') return 'credentials invalid';
  return status;
}

/**
 * Adaptive layout:
 * - ≤4 providers: 2-column cards
 * - 5-8: 3-column compact cards
 * - 9+: horizontal dot strip
 */
export function CloudProviderStatus({ accounts, loading }: CloudProviderStatusProps) {
  if (loading) {
    return (
      <div className="border border-gray-800 rounded p-3 mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">Cloud accounts</div>
        <p className="text-xs text-gray-500">Loading accounts...</p>
      </div>
    );
  }

  if (accounts.length === 0) {
    return (
      <div className="border border-gray-800 rounded p-3 mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">Cloud accounts</div>
        <p className="text-xs text-gray-600">No cloud accounts configured for this project.</p>
        <p className="text-xs text-cyan-400/70 mt-1">Add one in Settings → Cloud.</p>
      </div>
    );
  }

  // Group accounts by provider (may have multiple per provider)
  const byProvider = new Map<string, CloudAccount[]>();
  for (const a of accounts) {
    const list = byProvider.get(a.provider) || [];
    list.push(a);
    byProvider.set(a.provider, list);
  }

  const providers = [...byProvider.keys()];
  const count = providers.length;
  const activeCount = providers.filter(p =>
    byProvider.get(p)!.some(a => a.status === 'active')
  ).length;

  // 9+ providers: horizontal strip
  if (count > 8) {
    return (
      <div className="border border-gray-800 rounded p-3 mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">
          Cloud accounts ({activeCount}/{count} active)
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-1">
          {providers.map(p => {
            const accts = byProvider.get(p)!;
            const bestStatus = accts.some(a => a.status === 'active') ? 'active'
              : accts.some(a => a.status === 'validating') ? 'validating' : 'error';
            return (
              <div key={p} className="flex items-center gap-1.5 text-xs" title={accts.map(a => `${a.name} (${a.status})`).join(', ')}>
                <span className={`w-1.5 h-1.5 rounded-full ${statusDot(bestStatus)}`} />
                <span className={bestStatus === 'active' ? 'text-gray-300' : 'text-gray-500'}>
                  {LABEL[p] || p}
                </span>
                {accts.length > 1 && <span className="text-gray-700 text-[10px]">×{accts.length}</span>}
              </div>
            );
          })}
        </div>
      </div>
    );
  }

  const cols = count > 4 ? 'grid-cols-3' : 'grid-cols-2';

  return (
    <div className="border border-gray-800 rounded p-3 mb-4">
      <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">Cloud accounts</div>
      <div className={`grid ${cols} gap-2`}>
        {providers.map(p => {
          const accts = byProvider.get(p)!;
          const bestStatus = accts.some(a => a.status === 'active') ? 'active'
            : accts.some(a => a.status === 'validating') ? 'validating' : 'error';
          const bestAccount = accts.find(a => a.status === 'active') || accts[0];
          return (
            <div key={p} className="bg-[var(--bg-base)] border border-gray-800 rounded p-2">
              <div className="flex items-center gap-1.5 mb-0.5">
                <span className={`w-1.5 h-1.5 rounded-full ${statusDot(bestStatus)}`} />
                <span className={`text-xs font-medium ${bestStatus === 'active' ? 'text-gray-200' : 'text-gray-500'}`}>
                  {LABEL[p] || p}
                </span>
                {accts.length > 1 && <span className="text-[10px] text-gray-600">({accts.length} accounts)</span>}
              </div>
              <p className="text-[10px] text-gray-600 pl-3">
                {bestAccount.name} · {statusLabel(bestStatus)}
              </p>
            </div>
          );
        })}
      </div>
    </div>
  );
}
