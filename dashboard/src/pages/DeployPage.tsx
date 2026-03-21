import { useState, useCallback } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { Deployment } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { usePolling } from '../hooks/usePolling';
import { DeployWizard } from '../components/DeployWizard';
import { formatDuration } from '../lib/format';

export function DeployPage() {
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showWizard, setShowWizard] = useState(false);
  const navigate = useNavigate();

  const loadDeployments = useCallback(async () => {
    try {
      const data = await api.getDeployments({ limit: 50 });
      setDeployments(data);
      setError(null);
    } catch {
      setError('Failed to load deployments');
    } finally {
      setLoading(false);
    }
  }, []);

  usePolling(loadDeployments, 5000);

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2">
        <div>
          <h2 className="text-lg md:text-xl font-bold text-gray-100">Deployments</h2>
          <p className="text-sm text-gray-500 mt-1 hidden md:block">
            Deploy endpoints and run tests from the dashboard
          </p>
        </div>
        <button
          onClick={() => setShowWizard(true)}
          className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-2 rounded text-sm transition-colors flex-shrink-0"
        >
          New Deployment
        </button>
      </div>

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {loading && deployments.length === 0 ? (
        <p className="text-gray-500 text-sm">Loading deployments...</p>
      ) : deployments.length === 0 ? (
        <div className="py-10 text-center border border-gray-800/50 rounded">
          <p className="text-gray-500 text-sm">No deployments yet</p>
          <button onClick={() => setShowWizard(true)} className="text-xs text-cyan-400 mt-2">
            Create your first deployment
          </button>
        </div>
      ) : (
        <>
          {/* ── Mobile card layout (< md) ── */}
          <div className="md:hidden space-y-2">
            {deployments.map((d) => (
              <Link
                key={d.deployment_id}
                to={`/deploy/${d.deployment_id}`}
                className={`block border border-gray-800 rounded p-3 ${
                  d.status === 'running' ? 'border-l-2 border-l-blue-500/50' : ''
                }`}
              >
                <div className="flex items-center justify-between mb-1">
                  <span className="text-cyan-400 text-sm">{d.name}</span>
                  <StatusBadge status={d.status} />
                </div>
                <div className="flex items-center gap-3 text-xs text-gray-500 flex-wrap">
                  <span>{d.provider_summary || '—'}</span>
                  {d.endpoint_ips && Array.isArray(d.endpoint_ips) && d.endpoint_ips.length > 0 && (
                    <span className="font-mono truncate max-w-[200px]">{d.endpoint_ips[0]}</span>
                  )}
                  <span>{formatDuration(d.started_at, d.finished_at)}</span>
                </div>
              </Link>
            ))}
          </div>

          {/* ── Desktop/iPad table (≥ md) ── */}
          <div className="hidden md:block table-container">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="text-left px-4 py-2.5 font-medium">Name</th>
                  <th className="text-left px-4 py-2.5 font-medium">Provider</th>
                  <th className="text-left px-4 py-2.5 font-medium">Status</th>
                  <th className="text-left px-4 py-2.5 font-medium">Endpoints</th>
                  <th className="text-left px-4 py-2.5 font-medium hidden lg:table-cell">Duration</th>
                  <th className="text-left px-4 py-2.5 font-medium hidden lg:table-cell">Created</th>
                </tr>
              </thead>
              <tbody>
                {deployments.map((d) => (
                  <tr
                    key={d.deployment_id}
                    className={`border-b border-gray-800/30 hover:bg-gray-800/10 transition-colors ${
                      d.status === 'running' ? 'bg-blue-500/5' : ''
                    }`}
                  >
                    <td className="px-4 py-3">
                      <Link to={`/deploy/${d.deployment_id}`} className="text-cyan-400 hover:text-cyan-300">
                        {d.name}
                      </Link>
                    </td>
                    <td className="px-4 py-3 text-gray-400 text-xs">{d.provider_summary || '—'}</td>
                    <td className="px-4 py-3"><StatusBadge status={d.status} /></td>
                    <td className="px-4 py-3 text-gray-400 font-mono text-xs">
                      {d.endpoint_ips && Array.isArray(d.endpoint_ips) ? d.endpoint_ips.join(', ') : '—'}
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

      {showWizard && (
        <DeployWizard
          onClose={() => setShowWizard(false)}
          onCreated={(id) => {
            loadDeployments();
            navigate(`/deploy/${id}`);
          }}
        />
      )}
    </div>
  );
}
