import { useState, useEffect, useRef } from 'react';
import { api } from '../api/client';
import type { Deployment, CloudConnection } from '../api/types';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { useLiveStore } from '../stores/liveStore';

interface VersionInfo {
  dashboard_version: string;
  tester_version: string | null;
  latest_release: string | null;
  update_available: boolean;
  endpoints: { host: string; version: string | null; reachable: boolean }[];
}

const EMPTY_LINES: string[] = [];

export function SettingsPage() {
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [loading, setLoading] = useState(true);
  const [updating, setUpdating] = useState<Record<string, boolean>>({});
  const [activeUpdateId, setActiveUpdateId] = useState<string | null>(null);
  const [testerUpdating, setTesterUpdating] = useState(false);
  const [dashboardUpdating, setDashboardUpdating] = useState(false);
  const [inventory, setInventory] = useState<{ provider: string; name: string; region: string; status: string; public_ip: string | null; fqdn: string | null; vm_size: string | null; os: string | null; resource_group: string | null; managed: boolean }[]>([]);
  const [inventoryErrors, setInventoryErrors] = useState<string[]>([]);
  const [inventoryLoading, setInventoryLoading] = useState(false);
  const [cloudConnections, setCloudConnections] = useState<CloudConnection[]>([]);
  const [showAddAccount, setShowAddAccount] = useState(false);
  const [newProvider, setNewProvider] = useState<'azure' | 'aws' | 'gcp'>('azure');
  const [newName, setNewName] = useState('');
  const [newConfig, setNewConfig] = useState<Record<string, string>>({});
  const [validating, setValidating] = useState<Record<string, boolean>>({});
  const [addingAccount, setAddingAccount] = useState(false);
  const logRef = useRef<HTMLDivElement>(null);
  const addToast = useToast();
  const userRole = localStorage.getItem('role');

  // Live deploy logs from WebSocket
  const liveLines = useLiveStore(s =>
    activeUpdateId ? (s.deployLogs[activeUpdateId] || EMPTY_LINES) : EMPTY_LINES
  );

  usePageTitle('Settings');

  const loadData = () => {
    const promises: Promise<unknown>[] = [
      api.getVersionInfo().then(setVersionInfo),
      api.getDeployments({ limit: 50 }).then(deps => {
        setDeployments(deps.filter(d => d.status === 'completed'));
      }),
    ];
    if (userRole === 'admin') {
      promises.push(api.getCloudConnections().then(setCloudConnections).catch(() => {}));
    }
    Promise.all(promises).finally(() => setLoading(false));
  };

  useEffect(() => { loadData(); }, []);

  // Auto-scroll log
  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [liveLines]);

  const handleUpdateEndpoint = async (deploymentId: string, name: string) => {
    setUpdating(prev => ({ ...prev, [deploymentId]: true }));
    setActiveUpdateId(deploymentId);
    try {
      await api.updateEndpoint(deploymentId);
      addToast('success', `Update started for ${name}`);
    } catch {
      addToast('error', `Failed to update ${name}`);
      setUpdating(prev => ({ ...prev, [deploymentId]: false }));
      setActiveUpdateId(null);
    }
  };

  // Watch for deploy_complete events to know when update finished
  const events = useLiveStore(s => s.events);
  useEffect(() => {
    if (!activeUpdateId) return;
    const latest = events[events.length - 1];
    if (latest?.type === 'deploy_complete' && latest.deployment_id === activeUpdateId) {
      setUpdating(prev => ({ ...prev, [activeUpdateId]: false }));
      setTesterUpdating(false);
      addToast(
        latest.status === 'completed' ? 'success' : 'error',
        latest.status === 'completed' ? 'Update completed' : 'Update failed'
      );
      // Refresh versions after update
      setTimeout(() => {
        loadData();
        setActiveUpdateId(null);
      }, 2000);
    }
  }, [events, activeUpdateId]);

  const handleUpdateAll = async () => {
    const outdated = getOutdatedDeployments();
    if (outdated.length === 0) {
      addToast('info', 'All endpoints are up to date');
      return;
    }
    for (const dep of outdated) {
      await handleUpdateEndpoint(dep.deployment_id, dep.name);
      // Wait for this one to finish before starting next
      await new Promise(resolve => setTimeout(resolve, 5000));
    }
  };

  const getOutdatedDeployments = () => {
    if (!versionInfo?.latest_release) return [];
    return deployments.filter(d => {
      const ips: string[] = d.endpoint_ips && Array.isArray(d.endpoint_ips) ? d.endpoint_ips : [];
      return ips.some(ip => {
        const ep = versionInfo.endpoints.find(e => e.host === ip);
        return ep?.reachable && ep.version && ep.version !== versionInfo.latest_release;
      });
    });
  };

  if (loading) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Settings</h2>
        <p className="text-gray-500">Loading...</p>
      </div>
    );
  }

  const latestRelease = versionInfo?.latest_release;
  const outdatedDeps = getOutdatedDeployments();

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Settings</h2>
        <button
          onClick={loadData}
          className="text-xs text-gray-500 hover:text-gray-300 transition-colors"
        >
          Refresh
        </button>
      </div>

      {/* System Versions */}
      <div className="mb-8">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider font-medium">System Versions</h3>
          {latestRelease && (
            <span className="text-xs text-gray-600 font-mono">
              latest: v{latestRelease}
            </span>
          )}
        </div>

        <div>
          {/* Dashboard */}
          {(() => {
            const dashOutdated = versionInfo?.dashboard_version && latestRelease && versionInfo.dashboard_version !== latestRelease;
            return (
              <div className="flex items-center justify-between py-2.5">
                <div>
                  <span className="text-sm text-gray-200">Dashboard</span>
                  <span className="text-xs text-gray-600 ml-2">Control plane API + UI</span>
                </div>
                <div className="flex items-center gap-3">
                  <span className={`text-xs font-mono ${
                    versionInfo?.dashboard_version === latestRelease ? 'text-green-400' :
                    versionInfo?.dashboard_version ? 'text-yellow-400' : 'text-gray-400'
                  }`}>
                    v{versionInfo?.dashboard_version}
                  </span>
                  {dashOutdated && (
                    <button
                      onClick={async () => {
                        setDashboardUpdating(true);
                        try {
                          const result = await api.updateDashboard();
                          setActiveUpdateId(result.update_id);
                          addToast('success', 'Dashboard update started — will restart automatically');
                        } catch {
                          addToast('error', 'Failed to start dashboard update');
                          setDashboardUpdating(false);
                        }
                      }}
                      disabled={dashboardUpdating}
                      className={`text-xs px-3 py-1 rounded border transition-colors ${
                        dashboardUpdating
                          ? 'border-blue-500/30 text-blue-400 motion-safe:animate-pulse'
                          : 'border-yellow-500/30 text-yellow-400 hover:bg-yellow-500/10'
                      } disabled:opacity-50`}
                    >
                      {dashboardUpdating ? 'Updating...' : `Update to v${latestRelease}`}
                    </button>
                  )}
                </div>
              </div>
            );
          })()}

          {/* Local Tester — only shown when detected */}
          {versionInfo?.tester_version && (() => {
            const testerOutdated = latestRelease && versionInfo.tester_version !== latestRelease;
            return (
              <div className="flex items-center justify-between py-2.5 border-t border-gray-800/30">
                <div>
                  <span className="text-sm text-gray-200">Local Tester</span>
                  <span className="text-xs text-gray-600 ml-2">Probe executor</span>
                </div>
                <div className="flex items-center gap-3">
                  <span className={`text-xs font-mono ${
                    versionInfo.tester_version === latestRelease ? 'text-green-400' : 'text-yellow-400'
                  }`}>
                    v{versionInfo.tester_version}
                  </span>
                  {testerOutdated && (
                    <button
                      onClick={async () => {
                        setTesterUpdating(true);
                        try {
                          const result = await api.updateLocalTester();
                          setActiveUpdateId(result.update_id);
                          addToast('success', 'Tester update started');
                        } catch {
                          addToast('error', 'Failed to start tester update');
                          setTesterUpdating(false);
                        }
                      }}
                      disabled={testerUpdating}
                      className={`text-xs px-3 py-1 rounded border transition-colors ${
                        testerUpdating
                          ? 'border-blue-500/30 text-blue-400 motion-safe:animate-pulse'
                          : 'border-yellow-500/30 text-yellow-400 hover:bg-yellow-500/10'
                      } disabled:opacity-50`}
                    >
                      {testerUpdating ? 'Updating...' : `Update to v${latestRelease}`}
                    </button>
                  )}
                </div>
              </div>
            );
          })()}

          {/* Each endpoint */}
          {versionInfo?.endpoints.map(ep => {
            const host = ep.host;
            const dep = deployments.find(d =>
              d.endpoint_ips && Array.isArray(d.endpoint_ips) && d.endpoint_ips.includes(host)
            );
            const outdated = ep.reachable && ep.version && latestRelease && ep.version !== latestRelease;
            const isUpdating = dep ? updating[dep.deployment_id] : false;

            return (
              <div key={host} className="flex items-center justify-between py-2.5 border-t border-gray-800/30">
                <div>
                  <span className="text-sm text-gray-200">{host.split('.')[0]}</span>
                  <span className="text-xs text-gray-600 ml-2 truncate" title={host}>{host}</span>
                </div>
                <div className="flex items-center gap-3">
                  <span className={`text-xs font-mono ${
                    !ep.reachable ? 'text-gray-600' :
                    outdated ? 'text-yellow-400' : 'text-green-400'
                  }`}>
                    {ep.reachable ? `v${ep.version}` : 'offline'}
                  </span>
                  {ep.reachable && dep && outdated && (
                    <button
                      onClick={() => handleUpdateEndpoint(dep.deployment_id, dep.name)}
                      disabled={isUpdating}
                      className={`text-xs px-3 py-1 rounded border transition-colors ${
                        isUpdating
                          ? 'border-blue-500/30 text-blue-400 motion-safe:animate-pulse'
                          : 'border-yellow-500/30 text-yellow-400 hover:bg-yellow-500/10'
                      } disabled:opacity-50`}
                    >
                      {isUpdating ? 'Updating...' : 'Update'}
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Update All */}
      {outdatedDeps.length > 0 && (
        <div className="flex justify-end mb-6">
          <button
            onClick={handleUpdateAll}
            className="bg-yellow-600/20 border border-yellow-500/30 hover:bg-yellow-600/30 text-yellow-400 px-4 py-2 rounded text-sm transition-colors"
          >
            Update All Endpoints ({outdatedDeps.length})
          </button>
        </div>
      )}

      {/* Live Update Log */}
      {activeUpdateId && liveLines.length > 0 && (
        <div className="border border-blue-500/20 rounded-lg mb-6 overflow-hidden">
          <div className="flex items-center justify-between px-4 py-3 border-b border-gray-800">
            <h3 className="text-sm text-blue-400 font-medium flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-blue-400 motion-safe:animate-pulse" />
              Update Progress
            </h3>
            {!Object.values(updating).some(v => v) && (
              <button
                onClick={() => setActiveUpdateId(null)}
                className="text-xs text-gray-500 hover:text-gray-300"
              >
                Close
              </button>
            )}
          </div>
          <div
            ref={logRef}
            className="bg-[var(--bg-base)] p-4 h-[400px] overflow-y-auto font-mono text-xs leading-5"
          >
            {liveLines.map((line, i) => (
              <div key={i} className="text-gray-300 whitespace-pre-wrap break-all">
                {line}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Deployed Endpoints */}
      <div className="section-divider">
        <h3 className="text-xs text-gray-500 uppercase tracking-wider font-medium mb-3">Deployed Endpoints</h3>
        {deployments.length === 0 ? (
          <p className="text-gray-600 text-sm">No endpoints deployed</p>
        ) : (
          <div>
            {deployments.map((d, i) => {
              const ips: string[] = d.endpoint_ips && Array.isArray(d.endpoint_ips) ? d.endpoint_ips : [];
              const isUpdating = updating[d.deployment_id];
              return (
                <div key={d.deployment_id} className={`flex items-center justify-between py-3 ${i > 0 ? 'border-t border-gray-800/30' : ''}`}>
                  <div>
                    <span className="text-sm text-gray-200">{d.name}</span>
                    <span className="text-xs text-gray-600 ml-2">{d.provider_summary}</span>
                    <div className="text-xs text-gray-500 mt-0.5 font-mono">
                      {ips.join(', ') || 'No IPs'}
                    </div>
                  </div>
                  <button
                    onClick={() => handleUpdateEndpoint(d.deployment_id, d.name)}
                    disabled={isUpdating}
                    className={`text-xs px-3 py-1 rounded border transition-colors ${
                      isUpdating
                        ? 'border-blue-500/30 text-blue-400 motion-safe:animate-pulse'
                        : 'border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400'
                    } disabled:opacity-50`}
                  >
                    {isUpdating ? 'Updating...' : 'Update'}
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Cloud Inventory */}
      <div className="section-divider">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider font-medium">Cloud Inventory</h3>
          <button
            onClick={async () => {
              setInventoryLoading(true);
              try {
                const result = await api.getInventory();
                setInventory(result.vms);
                setInventoryErrors(result.errors);
              } catch {
                addToast('error', 'Failed to scan cloud inventory');
              } finally {
                setInventoryLoading(false);
              }
            }}
            disabled={inventoryLoading}
            className="text-xs px-3 py-1 rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 transition-colors disabled:opacity-50"
          >
            {inventoryLoading ? 'Scanning...' : 'Scan All Providers'}
          </button>
        </div>

        {inventoryErrors.length > 0 && (
          <div className="mb-3 space-y-1">
            {inventoryErrors.map((err, i) => (
              <p key={i} className="text-xs text-yellow-400">{err}</p>
            ))}
          </div>
        )}

        {inventory.length === 0 && !inventoryLoading ? (
          <p className="text-gray-600 text-sm">
            Click "Scan All Providers" to discover networker VMs across Azure, AWS, and GCP.
          </p>
        ) : inventoryLoading ? (
          <p className="text-gray-500 text-sm motion-safe:animate-pulse">Scanning cloud providers...</p>
        ) : (
          <div className="table-container">
            <table className="text-xs">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 bg-[var(--bg-surface)]">
                  <th className="px-3 py-2 text-left">Provider</th>
                  <th className="px-3 py-2 text-left">Name</th>
                  <th className="px-3 py-2 text-left">Region</th>
                  <th className="px-3 py-2 text-left">Status</th>
                  <th className="px-3 py-2 text-left">IP / DNS</th>
                  <th className="px-3 py-2 text-left">Size</th>
                  <th className="px-3 py-2 text-left">OS</th>
                  <th className="px-3 py-2 text-left">Managed</th>
                </tr>
              </thead>
              <tbody>
                {inventory.map((vm, i) => (
                  <tr key={`${vm.provider}-${vm.name}-${i}`} className="border-b border-gray-800/30 hover:bg-gray-800/20">
                    <td className="px-3 py-2">
                      <span className={`text-xs font-medium ${
                        vm.provider === 'azure' ? 'text-blue-400' :
                        vm.provider === 'aws' ? 'text-orange-400' :
                        'text-green-400'
                      }`}>
                        {vm.provider.toUpperCase()}
                      </span>
                    </td>
                    <td className="px-3 py-2 text-gray-200">{vm.name}</td>
                    <td className="px-3 py-2 text-gray-400">{vm.region}</td>
                    <td className="px-3 py-2">
                      <span className={`inline-flex items-center gap-1 ${
                        vm.status === 'running' ? 'text-green-400' :
                        vm.status === 'stopped' || vm.status === 'deallocated' ? 'text-yellow-400' :
                        vm.status === 'terminated' ? 'text-red-400' :
                        'text-gray-400'
                      }`}>
                        <span className={`w-1.5 h-1.5 rounded-full ${
                          vm.status === 'running' ? 'bg-green-400' :
                          vm.status === 'stopped' || vm.status === 'deallocated' ? 'bg-yellow-400' :
                          vm.status === 'terminated' ? 'bg-red-400' :
                          'bg-gray-500'
                        }`} />
                        {vm.status}
                      </span>
                    </td>
                    <td className="px-3 py-2 text-gray-400 font-mono text-[11px] max-w-48 truncate" title={vm.fqdn || vm.public_ip || ''}>
                      {vm.fqdn || vm.public_ip || '-'}
                    </td>
                    <td className="px-3 py-2 text-gray-500">{vm.vm_size || '-'}</td>
                    <td className="px-3 py-2 text-gray-500">{vm.os || '-'}</td>
                    <td className="px-3 py-2">
                      {vm.managed ? (
                        <span className="text-gray-300 text-[11px]">tracked</span>
                      ) : (
                        <span className="text-gray-600 text-[11px]">untracked</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="px-3 py-2 text-xs text-gray-600 border-t border-gray-800">
              {inventory.length} VM{inventory.length !== 1 ? 's' : ''} found
              {' · '}
              {inventory.filter(v => v.managed).length} tracked
              {' · '}
              {inventory.filter(v => v.status === 'running').length} running
            </div>
          </div>
        )}
      </div>

      {/* Cloud Accounts (admin only) */}
      {userRole === 'admin' && (
        <div className="section-divider">
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-xs text-gray-500 uppercase tracking-wider font-medium">Cloud Accounts</h3>
            <button
              onClick={() => {
                setShowAddAccount(true);
                setNewProvider('azure');
                setNewName('');
                setNewConfig({});
              }}
              className="text-xs px-3 py-1 rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 transition-colors"
            >
              + Add Account
            </button>
          </div>

          {/* Add Account Form */}
          {showAddAccount && (
            <div className="bg-[var(--bg-surface)] border border-gray-800 rounded-lg p-4 mb-4">
              <div className="flex items-center justify-between mb-3">
                <span className="text-sm text-gray-200 font-medium">New Cloud Connection</span>
                <button
                  onClick={() => setShowAddAccount(false)}
                  className="text-xs text-gray-500 hover:text-gray-300"
                >
                  Cancel
                </button>
              </div>

              <div className="grid grid-cols-2 gap-3 mb-3">
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Provider</label>
                  <select
                    value={newProvider}
                    onChange={e => {
                      setNewProvider(e.target.value as 'azure' | 'aws' | 'gcp');
                      setNewConfig({});
                    }}
                    className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  >
                    <option value="azure">Azure</option>
                    <option value="aws">AWS</option>
                    <option value="gcp">GCP</option>
                  </select>
                </div>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Display Name</label>
                  <input
                    value={newName}
                    onChange={e => setNewName(e.target.value)}
                    placeholder={`${newProvider.toUpperCase()} Production`}
                    className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              </div>

              {/* Provider-specific fields */}
              {newProvider === 'azure' && (
                <div className="space-y-2 mb-3">
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Subscription ID *</label>
                    <input
                      value={newConfig.subscription_id || ''}
                      onChange={e => setNewConfig(c => ({ ...c, subscription_id: e.target.value }))}
                      placeholder="00000000-0000-0000-0000-000000000000"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Tenant ID (auto-detected if blank)</label>
                    <input
                      value={newConfig.tenant_id || ''}
                      onChange={e => setNewConfig(c => ({ ...c, tenant_id: e.target.value }))}
                      placeholder="Optional"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <p className="text-xs text-gray-600">
                    The dashboard's managed identity needs Contributor role on this subscription.
                    Run: <code className="text-gray-400">az role assignment create --assignee 7dc26030-4be5-4866-938e-772cfe965043 --role Contributor --scope /subscriptions/&lt;id&gt;</code>
                  </p>
                </div>
              )}

              {newProvider === 'aws' && (
                <div className="space-y-2 mb-3">
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Account ID *</label>
                    <input
                      value={newConfig.account_id || ''}
                      onChange={e => setNewConfig(c => ({ ...c, account_id: e.target.value }))}
                      placeholder="123456789012"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">IAM Role ARN *</label>
                    <input
                      value={newConfig.role_arn || ''}
                      onChange={e => setNewConfig(c => ({ ...c, role_arn: e.target.value }))}
                      placeholder="arn:aws:iam::123456789012:role/networker-dashboard"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">External ID (optional)</label>
                    <input
                      value={newConfig.external_id || ''}
                      onChange={e => setNewConfig(c => ({ ...c, external_id: e.target.value }))}
                      placeholder="Auto-generated if blank"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Regions (comma-separated)</label>
                    <input
                      value={newConfig.regions || ''}
                      onChange={e => setNewConfig(c => ({ ...c, regions: e.target.value }))}
                      placeholder="us-east-1, us-west-2"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <p className="text-xs text-gray-600">
                    Create an IAM role that trusts Azure AD for cross-cloud federation. The role must allow <code className="text-gray-400">sts:AssumeRoleWithWebIdentity</code>.
                  </p>
                </div>
              )}

              {newProvider === 'gcp' && (
                <div className="space-y-2 mb-3">
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Project ID *</label>
                    <input
                      value={newConfig.project_id || ''}
                      onChange={e => setNewConfig(c => ({ ...c, project_id: e.target.value }))}
                      placeholder="my-project-id"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Workload Identity Pool (optional)</label>
                    <input
                      value={newConfig.workload_identity_pool || ''}
                      onChange={e => setNewConfig(c => ({ ...c, workload_identity_pool: e.target.value }))}
                      placeholder="projects/PROJECT_NUMBER/locations/global/workloadIdentityPools/POOL"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-gray-400 mb-1">Regions (comma-separated)</label>
                    <input
                      value={newConfig.regions || ''}
                      onChange={e => setNewConfig(c => ({ ...c, regions: e.target.value }))}
                      placeholder="us-central1, europe-west1"
                      className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                  <p className="text-xs text-gray-600">
                    Create a workload identity pool and grant the Azure managed identity access. See GCP documentation for setup.
                  </p>
                </div>
              )}

              <button
                onClick={async () => {
                  const configObj: Record<string, unknown> = { ...newConfig };
                  // Parse comma-separated regions into array
                  if (newConfig.regions) {
                    configObj.regions = newConfig.regions.split(',').map(r => r.trim()).filter(Boolean);
                  }
                  // Remove empty optional fields
                  Object.keys(configObj).forEach(k => {
                    if (configObj[k] === '' || configObj[k] === undefined) delete configObj[k];
                  });

                  setAddingAccount(true);
                  try {
                    const result = await api.createCloudConnection({
                      name: newName || `${newProvider.toUpperCase()} Account`,
                      provider: newProvider,
                      config: configObj,
                    });
                    addToast('success', 'Cloud account added');
                    setShowAddAccount(false);
                    // Auto-validate
                    setValidating(v => ({ ...v, [result.connection_id]: true }));
                    api.validateCloudConnection(result.connection_id)
                      .then(r => {
                        addToast(r.status === 'active' ? 'success' : 'error',
                          r.status === 'active' ? 'Connection validated' : `Validation failed: ${r.validation_error || 'unknown error'}`);
                      })
                      .catch(() => addToast('error', 'Validation request failed'))
                      .finally(() => {
                        setValidating(v => ({ ...v, [result.connection_id]: false }));
                        loadData();
                      });
                    loadData();
                  } catch {
                    addToast('error', 'Failed to add cloud account');
                  } finally {
                    setAddingAccount(false);
                  }
                }}
                disabled={addingAccount}
                className="bg-cyan-600/20 border border-cyan-500/30 hover:bg-cyan-600/30 text-cyan-400 px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
              >
                {addingAccount ? 'Adding...' : 'Add Connection'}
              </button>
            </div>
          )}

          {/* Connection List */}
          {cloudConnections.length === 0 && !showAddAccount ? (
            <p className="text-gray-600 text-sm">
              No cloud accounts configured. Add one to enable identity-federated deployments.
            </p>
          ) : (
            <div className="space-y-1">
              {cloudConnections.map(conn => {
                const isValidating = validating[conn.connection_id];
                const statusColor =
                  conn.status === 'active' ? 'text-green-400' :
                  conn.status === 'error' ? 'text-red-400' :
                  conn.status === 'disabled' ? 'text-gray-500' :
                  'text-yellow-400';
                const dotColor =
                  conn.status === 'active' ? 'bg-green-400' :
                  conn.status === 'error' ? 'bg-red-400' :
                  conn.status === 'disabled' ? 'bg-gray-500' :
                  'bg-yellow-400';

                // eslint-disable-next-line @typescript-eslint/no-explicit-any
                const cfg = conn.config as any;
                const configSummary = conn.provider === 'azure'
                  ? `subscription: ${(cfg.subscription_id || '').slice(0, 8)}...`
                  : conn.provider === 'aws'
                  ? `account: ${cfg.account_id || ''}`
                  : `project: ${cfg.project_id || ''}`;

                const regions = cfg.regions;
                const regionStr = Array.isArray(regions) ? regions.join(', ') : '';

                const providerColor =
                  conn.provider === 'azure' ? 'text-blue-400' :
                  conn.provider === 'aws' ? 'text-orange-400' :
                  'text-green-400';

                const validated = conn.last_validated
                  ? timeAgo(conn.last_validated)
                  : 'Not validated yet';

                return (
                  <div key={conn.connection_id} className="bg-[var(--bg-surface)] border border-gray-800 rounded p-3">
                    <div className="flex items-center justify-between mb-1">
                      <div className="flex items-center gap-2">
                        <span className={`w-2 h-2 rounded-full ${dotColor}`} />
                        <span className="text-sm text-gray-200 font-medium">{conn.name}</span>
                      </div>
                      <span className={`text-xs ${statusColor}`}>{conn.status}</span>
                    </div>
                    <div className="flex items-center justify-between">
                      <div className="text-xs text-gray-500">
                        <span className={providerColor}>{conn.provider}</span>
                        {' · '}{configSummary}
                        {regionStr && <>{' · '}{regionStr}</>}
                      </div>
                      <div className="flex items-center gap-2">
                        <button
                          onClick={async () => {
                            setValidating(v => ({ ...v, [conn.connection_id]: true }));
                            try {
                              const r = await api.validateCloudConnection(conn.connection_id);
                              addToast(r.status === 'active' ? 'success' : 'error',
                                r.status === 'active' ? 'Connection validated' : `Validation failed: ${r.validation_error || 'unknown'}`);
                            } catch {
                              addToast('error', 'Validation request failed');
                            } finally {
                              setValidating(v => ({ ...v, [conn.connection_id]: false }));
                              loadData();
                            }
                          }}
                          disabled={isValidating}
                          className="text-xs text-gray-400 hover:text-cyan-400 transition-colors disabled:opacity-50"
                        >
                          {isValidating ? 'Validating...' : 'Validate'}
                        </button>
                        <button
                          onClick={async () => {
                            try {
                              await api.deleteCloudConnection(conn.connection_id);
                              addToast('success', `Removed ${conn.name}`);
                              loadData();
                            } catch {
                              addToast('error', `Failed to remove ${conn.name}`);
                            }
                          }}
                          className="text-xs text-gray-500 hover:text-red-400 transition-colors"
                        >
                          Remove
                        </button>
                      </div>
                    </div>
                    <div className="text-xs text-gray-600 mt-1">
                      {conn.status === 'error' && conn.validation_error ? (
                        <span className="text-red-400/70">{conn.validation_error}</span>
                      ) : (
                        <span>Last validated: {validated}</span>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function timeAgo(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}
