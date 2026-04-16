import { useState, useMemo, useEffect, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
import type { Agent, Deployment, TestRun } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { useLiveStore } from '../stores/liveStore';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

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
  const { projectId } = useProject();
  const [summary, setSummary] = useState<Summary | null>(null);
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [recentRuns, setRecentRuns] = useState<TestRun[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const events = useLiveStore((s) => s.events);

  usePageTitle('Dashboard');

  useEffect(() => {
    api.getVersionInfo().then(setVersionInfo).catch(() => {});
  }, []);

  const loadData = useCallback(() => {
    if (!projectId) return;
    Promise.all([
      api.getDashboardSummary(projectId).then(setSummary),
      api.getAgents(projectId).then(r => setAgents(r.agents)),
      api.listTestRuns(projectId, { limit: 5 }).then(setRecentRuns),
      api.getDeployments(projectId, { limit: 10 }).then(setDeployments),
    ]).then(() => { setError(null); setLoading(false); })
     .catch(e => { setError(String(e)); setLoading(false); });
  }, [projectId]);

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

      {/* ── KPI Row — primary stat is dominant ── */}
      <div className="flex flex-wrap gap-x-8 gap-y-3 mb-8 pb-6 border-b border-gray-800/50 items-end">
        <div>
          <div className={`text-3xl font-bold tabular-nums ${onlineAgents.length > 0 ? 'text-green-400' : 'text-gray-600'}`}>
            {onlineAgents.length}
          </div>
          <div className="text-xs text-gray-500">runners online</div>
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
          <div className="text-xs text-gray-600">targets</div>
        </div>
      </div>

      {/* Empty workspace: full-width setup checklist */}
      {endpoints.length === 0 && agents.length === 0 && recentRuns.length === 0 ? (
        <div className="max-w-2xl">
          {/* Update notice — above checklist */}
          {versionInfo?.update_available && (
            <Link
              to={`/projects/${projectId}/settings`}
              className="flex items-center gap-2 mb-6 text-xs text-yellow-400/70 hover:text-yellow-400 transition-colors"
            >
              <span>Update available: {versionInfo.tester_version ?? versionInfo.dashboard_version} → {versionInfo.latest_release}</span>
            </Link>
          )}

          <h3 className="text-sm text-gray-300 font-medium mb-4">Get started</h3>
          <div className="space-y-2">
            {[
              { to: `/projects/${projectId}/deploy`, step: '1', title: 'Deploy a target', desc: 'Install networker-endpoint on a remote host to create a test target' },
              { to: `/projects/${projectId}/runs`, step: '2', title: 'Add a runner', desc: 'Connect an agent to run diagnostics from a remote location' },
              { to: `/projects/${projectId}/runs`, step: '3', title: 'Run your first test', desc: 'HTTP, DNS, TLS, UDP latency measured per-phase' },
              { to: `/projects/${projectId}/schedules`, step: '4', title: 'Create a schedule', desc: 'Automate recurring tests with cron expressions' },
            ].map(item => (
              <Link
                key={item.step}
                to={item.to}
                className="flex items-center gap-4 p-3 rounded border border-gray-800 hover:border-gray-700 transition-colors group"
              >
                <span className="w-6 h-6 rounded-full border border-gray-700 flex items-center justify-center text-xs text-gray-600 group-hover:border-cyan-500/50 group-hover:text-cyan-400 transition-colors flex-shrink-0">
                  {item.step}
                </span>
                <div className="min-w-0">
                  <p className="text-sm text-gray-300 group-hover:text-gray-100 transition-colors">{item.title}</p>
                  <p className="text-xs text-gray-600">{item.desc}</p>
                </div>
              </Link>
            ))}
          </div>
        </div>
      ) : (
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
          {/* ── Left column: Endpoints + Testers + Recent Tests ── */}
          <div className="lg:col-span-2 space-y-6">
            {/* Endpoint Health */}
            <div>
              <div className="flex items-center justify-between mb-3">
                <h3 className="text-xs text-gray-500 tracking-wider font-medium">target health</h3>
                <Link to={`/projects/${projectId}/deploy`} className="text-xs text-gray-600 hover:text-gray-400 transition-colors">View all →</Link>
              </div>
              {endpoints.length === 0 ? (
                <div className="border border-gray-800 rounded p-6 text-center">
                  <p className="text-gray-600 text-sm">No targets deployed</p>
                  <Link to={`/projects/${projectId}/deploy`} className="text-xs text-cyan-400 mt-1 inline-block">Deploy your first target</Link>
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
                <h3 className="text-xs text-gray-500 tracking-wider font-medium">runners</h3>
                <Link to={`/projects/${projectId}/runs`} className="text-xs text-gray-600 hover:text-gray-400 transition-colors">Manage →</Link>
              </div>
              {agents.length === 0 ? (
                <div className="border border-gray-800 rounded p-6 text-center">
                  <p className="text-gray-600 text-sm">No runners deployed</p>
                  <Link to={`/projects/${projectId}/runs`} className="text-xs text-cyan-400 mt-1 inline-block">Add a runner</Link>
                </div>
              ) : (
                // Dashboard tester card ordering:
                //   1. online first (what users actually care about)
                //   2. then currently-attached-but-offline (still a live row
                //      in project_tester — could come back)
                //   3. orphan agent rows (tester_id NULL, left over from
                //      deletes) filtered out entirely — they add clutter
                //      without actionable info. VM History still surfaces
                //      the deletion event for anyone who wants the audit.
                //   4. Capped at 8 cards so the dashboard doesn't become a
                //      long scrolling list once a project has many testers.
                (() => {
                  // Agents whose `tester_id` is NULL are orphaned (V032
                  // sets agent.tester_id NULL when the tester row is
                  // deleted, preserving the agent for audit). Hide them
                  // unless they happen to be online right now — a
                  // still-heartbeating orphan is worth surfacing so the
                  // user can delete it, but a stopped orphan is pure noise.
                  const sorted = [...agents]
                    .filter((a) => a.status === 'online' || a.tester_id != null)
                    .sort((a, b) => {
                      if (a.status === 'online' && b.status !== 'online') return -1;
                      if (a.status !== 'online' && b.status === 'online') return 1;
                      return a.name.localeCompare(b.name);
                    });
                  const MAX_CARDS = 8;
                  const shown = sorted.slice(0, MAX_CARDS);
                  const hidden = sorted.length - shown.length;
                  return (
                    <>
                      <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
                        {shown.map((a) => (
                          <div
                            key={a.agent_id}
                            className={`border border-gray-800 rounded p-3 flex items-center gap-3 ${
                              a.status !== 'online' ? 'opacity-50' : ''
                            }`}
                          >
                            <span
                              className={`w-2 h-2 rounded-full flex-shrink-0 ${
                                a.status === 'online' ? 'bg-green-400' : 'bg-gray-600'
                              }`}
                            />
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
                      {hidden > 0 && (
                        <Link
                          to={`/projects/${projectId}/testers`}
                          className="block text-xs text-gray-500 hover:text-cyan-400 mt-2 text-center"
                        >
                          + {hidden} more -- view all runners →
                        </Link>
                      )}
                    </>
                  );
                })()
              )}
            </div>

            {/* Recent Runs */}
            <div>
              <div className="flex items-center justify-between mb-3">
                <h3 className="text-xs text-gray-500 tracking-wider font-medium">recent runs</h3>
                <Link to={`/projects/${projectId}/runs`} className="text-xs text-gray-600 hover:text-gray-400 transition-colors">View all →</Link>
              </div>
              {recentRuns.length === 0 ? (
                <div className="border border-gray-800 rounded p-6 text-center">
                  <p className="text-gray-600 text-sm">No runs yet</p>
                  <Link to={`/projects/${projectId}/tests/new`} className="text-xs text-cyan-400 mt-1 inline-block">Run your first test</Link>
                </div>
              ) : (
                <div className="table-container">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                        <th className="px-3 py-2 text-left font-medium">ID</th>
                        <th className="px-3 py-2 text-left font-medium">Status</th>
                        <th className="px-3 py-2 text-left font-medium">Success</th>
                        <th className="px-3 py-2 text-left font-medium">Failed</th>
                        <th className="px-3 py-2 text-left font-medium">Duration</th>
                      </tr>
                    </thead>
                    <tbody>
                      {recentRuns.map(run => (
                        <tr key={run.id} className="border-b border-gray-800/30 hover:bg-gray-800/20">
                          <td className="px-3 py-2">
                            <Link to={`/projects/${projectId}/runs/${run.id}`} className="text-cyan-400 hover:underline font-mono text-xs">
                              {run.id.slice(0, 8)}
                            </Link>
                          </td>
                          <td className="px-3 py-2"><StatusBadge status={run.status} /></td>
                          <td className={`px-3 py-2 tabular-nums text-xs ${run.success_count > 0 ? 'text-green-400' : 'text-gray-600'}`}>
                            {run.success_count}
                          </td>
                          <td className={`px-3 py-2 tabular-nums text-xs ${run.failure_count > 0 ? 'text-red-400 font-semibold' : 'text-gray-600'}`}>
                            {run.failure_count}
                          </td>
                          <td className="px-3 py-2 text-gray-500 text-xs font-mono">
                            {run.started_at && run.finished_at
                              ? `${((new Date(run.finished_at).getTime() - new Date(run.started_at).getTime()) / 1000).toFixed(1)}s`
                              : run.status === 'running' ? '...' : '-'}
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
                to={`/projects/${projectId}/settings`}
                className="block border border-gray-800 rounded p-3 hover:border-gray-700 transition-colors"
              >
                <div className="text-yellow-400/70 text-xs">Update available</div>
                <div className="text-gray-600 text-xs mt-0.5">
                  {versionInfo.tester_version ?? versionInfo.dashboard_version} → {versionInfo.latest_release}
                </div>
              </Link>
            )}

            {/* Quick Actions — compact links, no descriptions */}
            <div>
              <h3 className="text-xs text-gray-600 mb-3">quick actions</h3>
              <div className="space-y-1">
                <Link to={`/projects/${projectId}/runs/new`} className="block px-3 py-2 text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/30 rounded transition-colors">
                  New Run
                </Link>
                <Link to={`/projects/${projectId}/deploy`} className="block px-3 py-2 text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/30 rounded transition-colors">
                  Deploy Target
                </Link>
                <Link to={`/projects/${projectId}/schedules`} className="block px-3 py-2 text-sm text-gray-400 hover:text-gray-200 hover:bg-gray-800/30 rounded transition-colors">
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
      )}
    </div>
  );
}
