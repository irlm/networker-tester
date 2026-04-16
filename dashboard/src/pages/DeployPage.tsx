import { useState, useCallback, useMemo } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { Agent, Deployment } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { DeployWizard } from '../components/DeployWizard';
import { formatDuration } from '../lib/format';
import { useProject } from '../hooks/useProject';

export function DeployPage() {
  const { projectId, isOperator } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showWizard, setShowWizard] = useState(false);
  const navigate = useNavigate();

  const infraSearch = searchParams.get('search') || '';
  const setInfraSearch = useCallback((v: string) => {
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (!v) next.delete('search');
      else next.set('search', v);
      return next;
    }, { replace: true });
  }, [setSearchParams]);

  usePageTitle('Infrastructure');

  const loadData = useCallback(async () => {
    if (!projectId) return;
    try {
      const [deps, ags] = await Promise.all([
        api.getDeployments(projectId, { limit: 50 }),
        api.getAgents(projectId).then(r => r.agents),
      ]);
      setDeployments(deps);
      setAgents(ags);
      setError(null);
    } catch {
      setError('Failed to load infrastructure');
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  usePolling(loadData, 10000);

  // Apply search filter
  const filtered = useMemo(() => {
    if (!infraSearch.trim()) return deployments;
    const q = infraSearch.toLowerCase();
    return deployments.filter(d =>
      d.name.toLowerCase().includes(q) ||
      (d.provider_summary || '').toLowerCase().includes(q) ||
      (d.endpoint_ips || []).some(ip => ip.includes(q))
    );
  }, [deployments, infraSearch]);

  const completedDeps = filtered.filter(d => d.status === 'completed');
  const activeDeps = filtered.filter(d => d.status === 'running' || d.status === 'pending');
  const testerVms = agents.filter(a => a.provider && a.provider !== 'local');

  return (
    <div className="p-4 md:p-6">
      {/* Header */}
      <div className="flex items-center justify-between mb-6 gap-2">
        <div>
          <h2 className="text-lg md:text-xl font-bold text-gray-100">Infrastructure</h2>
          <p className="text-sm text-gray-500 mt-1 hidden md:block">
            Endpoints and testers deployed across cloud regions
          </p>
        </div>
        {isOperator && (
          <button
            onClick={() => setShowWizard(true)}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-2 rounded text-sm transition-colors flex-shrink-0"
          >
            New Endpoint
          </button>
        )}
      </div>

      {/* ── Search Bar ── */}
      <FilterBar
        activeCount={infraSearch ? 1 : 0}
        onClearAll={() => setInfraSearch('')}
        chips={infraSearch ? <FilterChip label="Search" value={infraSearch} onClear={() => setInfraSearch('')} /> : undefined}
      >
        <input
          type="search"
          value={infraSearch}
          onChange={(e) => setInfraSearch(e.target.value)}
          placeholder="Search by name, region, or IP..."
          aria-label="Search infrastructure"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-48 md:w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
        />
      </FilterBar>

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {/* ── Active Deployments (in progress) ── */}
      {activeDeps.length > 0 && (
        <div className="mb-8">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3">in progress</h3>
          <div className="space-y-2">
            {activeDeps.map(d => (
              <Link
                key={d.deployment_id}
                to={`/projects/${projectId}/deploy/${d.deployment_id}`}
                className="block border border-blue-500/20 bg-blue-500/5 rounded p-3"
              >
                <div className="flex items-center justify-between mb-1">
                  <span className="text-cyan-400 text-sm">{d.name}</span>
                  <StatusBadge status={d.status} />
                </div>
                <div className="text-xs text-gray-500">{d.provider_summary || 'Deploying...'}</div>
              </Link>
            ))}
          </div>
        </div>
      )}

      {/* ── Endpoints Section ── */}
      <div className="mb-8">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium">
            endpoints
            {completedDeps.length > 0 && <span className="text-gray-600 ml-2">({completedDeps.length})</span>}
          </h3>
        </div>

        {loading && deployments.length === 0 ? (
          <div className="table-container">
            <div className="px-4 py-3 flex gap-8">
              {[80, 48, 56, 120, 48].map((w, i) => (
                <div key={i} className="h-3 rounded bg-gray-800/50 motion-safe:animate-pulse" style={{ width: w }} />
              ))}
            </div>
          </div>
        ) : completedDeps.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">No endpoints deployed</p>
            <button onClick={() => setShowWizard(true)} className="text-xs text-cyan-400 mt-2">
              Deploy your first endpoint
            </button>
          </div>
        ) : (
          <>
            {/* Mobile */}
            <div className="md:hidden space-y-2">
              {completedDeps.map(d => (
                <Link
                  key={d.deployment_id}
                  to={`/projects/${projectId}/deploy/${d.deployment_id}`}
                  className="block border border-gray-800 rounded p-3"
                >
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-cyan-400 text-sm">{d.name}</span>
                    <StatusBadge status={d.status} />
                  </div>
                  <div className="flex items-center gap-3 text-xs text-gray-500 flex-wrap">
                    <span>{d.provider_summary || '—'}</span>
                    {d.endpoint_ips?.[0] && (
                      <span className="font-mono truncate max-w-[200px]">{d.endpoint_ips[0]}</span>
                    )}
                  </div>
                </Link>
              ))}
            </div>

            {/* Desktop */}
            <div className="hidden md:block table-container">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                    <th className="text-left px-4 py-2.5 font-medium">Name</th>
                    <th className="text-left px-4 py-2.5 font-medium">Provider</th>
                    <th className="text-left px-4 py-2.5 font-medium">Status</th>
                    <th className="text-left px-4 py-2.5 font-medium">Endpoint</th>
                    <th className="text-left px-4 py-2.5 font-medium hidden lg:table-cell">Duration</th>
                    <th className="text-left px-4 py-2.5 font-medium hidden lg:table-cell">Created</th>
                  </tr>
                </thead>
                <tbody>
                  {completedDeps.map(d => (
                    <tr key={d.deployment_id} className="border-b border-gray-800/30 hover:bg-gray-800/10">
                      <td className="px-4 py-3">
                        <Link to={`/projects/${projectId}/deploy/${d.deployment_id}`} className="text-cyan-400 hover:text-cyan-300">
                          {d.name}
                        </Link>
                      </td>
                      <td className="px-4 py-3 text-gray-400 text-xs">{d.provider_summary || '—'}</td>
                      <td className="px-4 py-3"><StatusBadge status={d.status} /></td>
                      <td className="px-4 py-3 text-gray-400 font-mono text-xs truncate max-w-48">
                        {d.endpoint_ips?.[0] || '—'}
                      </td>
                      <td className="px-4 py-3 text-gray-400 font-mono text-xs hidden lg:table-cell">
                        {formatDuration(d.started_at, d.finished_at)}
                      </td>
                      <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">
                        {new Date(d.created_at).toLocaleString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>

      {/* ── Tester VMs Section ── */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium">
            tester VMs
            {testerVms.length > 0 && <span className="text-gray-600 ml-2">({testerVms.length})</span>}
          </h3>
          <Link to={`/projects/${projectId}/vms/testers`} className="text-xs text-gray-600 hover:text-gray-400 transition-colors">
            Manage in Cloud VMs →
          </Link>
        </div>

        {testerVms.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">No tester VMs deployed</p>
            <Link to={`/projects/${projectId}/vms/testers`} className="text-xs text-cyan-400 mt-2 inline-block">
              Deploy a tester from Cloud VMs
            </Link>
          </div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-2">
            {testerVms.map(a => (
              <div
                key={a.agent_id}
                className={`border rounded p-3 flex items-center gap-3 ${
                  a.status === 'online'
                    ? 'border-green-500/20 bg-green-500/5'
                    : a.status === 'deploying'
                      ? 'border-blue-500/20 bg-blue-500/5'
                      : 'border-gray-800 opacity-60'
                }`}
              >
                <span className={`w-2.5 h-2.5 rounded-full flex-shrink-0 ${
                  a.status === 'online' ? 'bg-green-400' :
                  a.status === 'deploying' ? 'bg-blue-400 motion-safe:animate-pulse' :
                  'bg-gray-600'
                }`} />
                <div className="min-w-0 flex-1">
                  <div className="text-sm text-gray-200 truncate">{a.name}</div>
                  <div className="text-xs text-gray-600">
                    {a.provider && `${a.provider} `}
                    {a.region && a.region}
                  </div>
                </div>
                <StatusBadge status={a.status} />
              </div>
            ))}
          </div>
        )}
      </div>

      {showWizard && (
        <DeployWizard
          projectId={projectId}
          onClose={() => setShowWizard(false)}
          onCreated={(id) => {
            loadData();
            navigate(`/projects/${projectId}/deploy/${id}`);
          }}
        />
      )}
    </div>
  );
}
