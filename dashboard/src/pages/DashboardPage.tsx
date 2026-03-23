import { useState, useMemo, useEffect, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
import type { Agent, Job, Deployment, RunSummary } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { useLiveStore } from '../stores/liveStore';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';

interface Summary {
  agents_online: number;
  jobs_running: number;
  runs_24h: number;
  jobs_pending: number;
}

interface VersionInfo {
  dashboard_version: string;
  tester_version: string | null;
  latest_release: string | null;
  update_available: boolean;
  endpoints: { host: string; version: string | null; reachable: boolean }[];
}

export function DashboardPage() {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [recentJobs, setRecentJobs] = useState<Job[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [, setRecentRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const events = useLiveStore((s) => s.events);

  usePageTitle('Dashboard');

  useEffect(() => {
    api.getVersionInfo().then(setVersionInfo).catch(() => {});
  }, []);

  const loadData = useCallback(() => {
    Promise.all([
      api.getDashboardSummary().then(setSummary),
      api.getAgents().then(r => setAgents(r.agents)),
      api.getJobs({ limit: 5 }).then(setRecentJobs),
      api.getDeployments({ limit: 10 }).then(setDeployments),
      api.getRuns({ limit: 5 }).then(setRecentRuns),
    ]).then(() => { setError(null); setLoading(false); })
     .catch(e => { setError(String(e)); setLoading(false); });
  }, []);

  usePolling(loadData, 15000);

  const recentEvents = useMemo(
    () => events.filter(e => e.type !== 'deploy_log').slice(-10).reverse(),
    [events],
  );

  const onlineAgents = agents.filter(a => a.status === 'online');
  const completedDeps = deployments.filter(d => d.status === 'completed' && d.endpoint_ips?.length);
  const endpoints = versionInfo?.endpoints || [];

  if (loading && !summary) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Dashboard</h2>
        <div className="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6">
          {[1, 2, 3, 4].map(i => (
            <div key={i} className="border border-gray-800 rounded p-4">
              <div className="h-6 w-12 bg-gray-800 rounded motion-safe:animate-pulse mb-2" />
              <div className="h-3 w-20 bg-gray-800/60 rounded motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (error && !summary) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Dashboard</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded p-4">
          <p className="text-red-400 text-sm">Failed to load dashboard data. Check your connection and try refreshing.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      {/* Header */}
      <div className="flex items-baseline justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Dashboard</h2>
        <span className="text-xs text-gray-600">v{versionInfo?.dashboard_version}</span>
      </div>

      {/* ── KPI Row — no borders, just numbers ── */}
      <div className="flex flex-wrap gap-x-8 gap-y-3 mb-8 pb-6 border-b border-gray-800/50">
        <div>
          <div className={`text-xl font-semibold tabular-nums ${onlineAgents.length > 0 ? 'text-green-400' : 'text-gray-600'}`}>
            {onlineAgents.length}
          </div>
          <div className="text-xs text-gray-600">testers online</div>
        </div>
        <div>
          <div className={`text-xl font-semibold tabular-nums ${(summary?.jobs_running ?? 0) > 0 ? 'text-blue-400' : 'text-gray-600'}`}>
            {summary?.jobs_running ?? 0}
          </div>
          <div className="text-xs text-gray-600">running</div>
        </div>
        <div>
          <div className="text-xl font-semibold tabular-nums text-gray-300">
            {summary?.runs_24h ?? 0}
          </div>
          <div className="text-xs text-gray-600">runs (24h)</div>
        </div>
        <div>
          <div className="text-xl font-semibold tabular-nums text-gray-300">
            {completedDeps.length}
          </div>
          <div className="text-xs text-gray-600">endpoints</div>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* ── Left column: Endpoints + Testers ── */}
        <div className="lg:col-span-2 space-y-6">
          {/* Endpoint Health */}
          <div>
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-xs text-gray-500 tracking-wider font-medium">endpoint health</h3>
              <Link to="/deploy" className="text-xs text-gray-600 hover:text-gray-400 transition-colors">View all →</Link>
            </div>
            {endpoints.length === 0 ? (
              <div className="border border-gray-800 rounded p-6 text-center">
                <p className="text-gray-600 text-sm">No endpoints deployed</p>
                <Link to="/deploy" className="text-xs text-cyan-400 mt-1 inline-block">Deploy your first endpoint</Link>
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
                {endpoints.map(ep => {
                  const host = ep.host.split('.')[0];
                  return (
                    <div
                      key={ep.host}
                      className={`border border-gray-800 rounded p-3 flex items-center gap-3 ${
                        !ep.reachable ? 'opacity-50' : ''
                      }`}
                    >
                      <span className={`w-2 h-2 rounded-full flex-shrink-0 ${
                        ep.reachable ? 'bg-green-400' : 'bg-gray-600'
                      }`} />
                      <div className="min-w-0 flex-1">
                        <div className="text-sm text-gray-300 truncate">{host}</div>
                        <div className="text-xs text-gray-700 truncate" title={ep.host}>{ep.host}</div>
                      </div>
                      <span className="text-xs font-mono text-gray-600">
                        {ep.reachable ? `v${ep.version}` : 'offline'}
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {/* Testers */}
          <div>
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-xs text-gray-500 tracking-wider font-medium">testers</h3>
              <Link to="/tests" className="text-xs text-gray-600 hover:text-gray-400 transition-colors">Manage →</Link>
            </div>
            {agents.length === 0 ? (
              <div className="border border-gray-800 rounded p-6 text-center">
                <p className="text-gray-600 text-sm">No testers deployed</p>
                <Link to="/tests" className="text-xs text-cyan-400 mt-1 inline-block">Add a tester</Link>
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
                {agents.map(a => (
                  <div
                    key={a.agent_id}
                    className={`border border-gray-800 rounded p-3 flex items-center gap-3 ${
                      a.status !== 'online' ? 'opacity-50' : ''
                    }`}
                  >
                    <span className={`w-2 h-2 rounded-full flex-shrink-0 ${
                      a.status === 'online' ? 'bg-green-400' : 'bg-gray-600'
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

          {/* Recent Tests */}
          <div>
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-xs text-gray-500 tracking-wider font-medium">recent tests</h3>
              <Link to="/tests" className="text-xs text-gray-600 hover:text-gray-400 transition-colors">View all →</Link>
            </div>
            {recentJobs.length === 0 ? (
              <div className="border border-gray-800 rounded p-6 text-center">
                <p className="text-gray-600 text-sm">No tests yet</p>
                <Link to="/tests" className="text-xs text-cyan-400 mt-1 inline-block">Run your first test</Link>
              </div>
            ) : (
              <div className="table-container">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                      <th className="px-3 py-2 text-left font-medium">ID</th>
                      <th className="px-3 py-2 text-left font-medium">Target</th>
                      <th className="px-3 py-2 text-left font-medium">Modes</th>
                      <th className="px-3 py-2 text-left font-medium">Status</th>
                      <th className="px-3 py-2 text-left font-medium">Duration</th>
                    </tr>
                  </thead>
                  <tbody>
                    {recentJobs.map(job => (
                      <tr key={job.job_id} className="border-b border-gray-800/30 hover:bg-gray-800/20">
                        <td className="px-3 py-2">
                          <Link to={`/tests/${job.job_id}`} className="text-cyan-400 hover:underline font-mono text-xs">
                            {job.job_id.slice(0, 8)}
                          </Link>
                        </td>
                        <td className="px-3 py-2 text-gray-400 text-xs truncate max-w-40">
                          {job.config?.target?.replace('https://', '').split(':')[0]}
                        </td>
                        <td className="px-3 py-2 text-gray-500 text-xs truncate max-w-24">
                          {job.config?.modes?.join(', ')}
                        </td>
                        <td className="px-3 py-2"><StatusBadge status={job.status} /></td>
                        <td className="px-3 py-2 text-gray-500 text-xs font-mono">
                          {job.started_at && job.finished_at
                            ? `${((new Date(job.finished_at).getTime() - new Date(job.started_at).getTime()) / 1000).toFixed(1)}s`
                            : '-'}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>

        {/* ── Right column: Live Feed + Quick Actions ── */}
        <div className="space-y-6">
          {/* Update notice (admin only) — subtle */}
          {versionInfo?.update_available && (
            <Link
              to="/settings"
              className="block border border-gray-800 rounded p-3 hover:border-gray-700 transition-colors"
            >
              <div className="text-yellow-400/70 text-xs">Update available</div>
              <div className="text-gray-600 text-xs mt-0.5">
                {versionInfo.dashboard_version} → {versionInfo.latest_release}
              </div>
            </Link>
          )}

          {/* Quick Actions — compact links, no descriptions */}
          <div>
            <h3 className="text-xs text-gray-600 mb-3">quick actions</h3>
            <div className="space-y-1">
              <Link to="/tests" className="block px-3 py-2 text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/30 rounded transition-colors">
                New Test
              </Link>
              <Link to="/deploy" className="block px-3 py-2 text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/30 rounded transition-colors">
                Deploy Endpoint
              </Link>
              <Link to="/schedules" className="block px-3 py-2 text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/30 rounded transition-colors">
                New Schedule
              </Link>
            </div>
          </div>

          {/* Live Feed */}
          <div>
            <div className="flex items-center gap-2 mb-3">
              <span className="w-1.5 h-1.5 rounded-full bg-green-400 motion-safe:animate-pulse" />
              <h3 className="text-xs text-gray-500 tracking-wider font-medium">live feed</h3>
            </div>
            <div className="border border-gray-800/50 rounded max-h-64 overflow-y-auto">
              {recentEvents.length === 0 ? (
                <div className="py-8 text-center">
                  <p className="text-gray-700 text-xs">Waiting for events...</p>
                </div>
              ) : (
                recentEvents.map((event, i) => (
                  <div
                    key={`${event.type}-${event.job_id ?? event.agent_id ?? ''}-${i}`}
                    className="px-3 py-2 border-b border-gray-800/30 text-xs flex items-center gap-2"
                  >
                    <StatusBadge status={event.status || event.type} />
                    <span className="text-gray-500 font-mono">
                      {event.job_id?.slice(0, 8) || event.agent_id?.slice(0, 8)}
                    </span>
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
