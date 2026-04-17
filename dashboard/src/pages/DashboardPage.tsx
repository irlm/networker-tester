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
      api.listTestRuns(projectId, { limit: 10 }).then(setRecentRuns),
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

  // Filter out queued zombie runs — only show runs with real signal
  const nonQueuedRuns = useMemo(
    () => recentRuns.filter(r => r.status !== 'queued').slice(0, 5),
    [recentRuns],
  );

  // Sparse data: fewer than 3 completed runs AND no endpoints deployed
  const isSparse = nonQueuedRuns.length < 3 && endpoints.length === 0;

  // Truly empty: nothing at all
  const isEmpty = endpoints.length === 0 && agents.length === 0 && recentRuns.length === 0;

  // Active count: only actually running, not queued
  const activeCount = summary?.jobs_running ?? 0;

  // Hide runs_24h KPI when fewer than 5 total runs
  const totalRuns = (summary?.runs_24h ?? 0);
  const showRuns24h = totalRuns >= 5;

  // All KPIs zero — show onboarding instead of 4 zeros
  const allKpisZero = onlineAgents.length === 0 && activeCount === 0 && completedDeps.length === 0 && !showRuns24h;

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

      {/* ── KPI Row — skip when all zeros ── */}
      {!allKpisZero && (
        <div className="flex flex-wrap gap-x-8 gap-y-3 mb-8 pb-6 border-b border-gray-800/50 items-end">
          <div>
            <div className={`text-3xl font-bold tabular-nums ${onlineAgents.length > 0 ? 'text-green-400' : 'text-gray-600'}`}>
              {onlineAgents.length}
            </div>
            <div className="text-xs text-gray-500" title="Agents that execute network tests from remote locations">runners online</div>
          </div>
          <div>
            <div className={`text-xl font-semibold tabular-nums ${activeCount > 0 ? 'text-blue-400' : 'text-gray-600'}`}>
              {activeCount}
            </div>
            <div className="text-xs text-gray-600">running</div>
          </div>
          {showRuns24h && (
            <div>
              <div className="text-xl font-semibold tabular-nums text-gray-300">
                {summary?.runs_24h ?? 0}
              </div>
              <div className="text-xs text-gray-600">runs · last 24h</div>
            </div>
          )}
          <div>
            <div className="text-xl font-semibold tabular-nums text-gray-300">
              {completedDeps.length}
            </div>
            <div className="text-xs text-gray-600">targets</div>
          </div>
        </div>
      )}

      {/* Update notice */}
      {versionInfo?.update_available && (
        <Link
          to={`/projects/${projectId}/settings`}
          className="flex items-center gap-2 mb-6 text-xs text-yellow-400/70 hover:text-yellow-400 transition-colors"
        >
          <span>Update available: {versionInfo.tester_version ?? versionInfo.dashboard_version} → {versionInfo.latest_release}</span>
        </Link>
      )}

      {/* Empty workspace: full-width setup checklist */}
      {isEmpty ? (
        <div className="max-w-2xl">
          <h3 className="text-sm text-gray-300 font-medium mb-4">Get started</h3>
          <div className="space-y-2">
            {[
              { to: `/projects/${projectId}/deploy`, step: '1', title: 'Deploy a test target', desc: 'Install networker-endpoint on a remote host to create a test target' },
              { to: `/projects/${projectId}/runs`, step: '2', title: 'Add a runner', desc: 'Connect an agent to run probes and benchmarks from a remote location' },
              { to: `/projects/${projectId}/runs`, step: '3', title: 'Run your first test', desc: 'HTTP, DNS, TLS, UDP latency measured per-phase' },
              { to: `/projects/${projectId}/schedules`, step: '4', title: 'Schedule recurring tests', desc: 'Automate recurring tests with cron expressions' },
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
      ) : isSparse ? (
        /* ── Sparse data: single-column onboarding view ── */
        <div className="max-w-2xl space-y-6">
          {/* Inline summary line */}
          {allKpisZero && (
            <p className="text-sm text-gray-500">
              {onlineAgents.length} runners online · {completedDeps.length} targets · {nonQueuedRuns.length} runs
            </p>
          )}

          <div>
            <h3 className="text-sm text-gray-300 font-medium mb-4">Get started</h3>
            <div className="space-y-2">
              {[
                { to: `/projects/${projectId}/deploy`, step: '1', title: 'Deploy a test target', done: endpoints.length > 0 },
                { to: `/projects/${projectId}/tests/new`, step: '2', title: 'Run your first network test', done: nonQueuedRuns.length > 0 },
                { to: `/projects/${projectId}/schedules`, step: '3', title: 'Schedule recurring tests', done: false },
              ].map(item => (
                <Link
                  key={item.step}
                  to={item.to}
                  className={`flex items-center gap-4 p-3 rounded border border-gray-800 hover:border-gray-700 transition-colors group ${item.done ? 'opacity-50' : ''}`}
                >
                  <span className={`w-6 h-6 rounded-full border flex items-center justify-center text-xs flex-shrink-0 ${
                    item.done
                      ? 'border-green-500/50 text-green-400'
                      : 'border-gray-700 text-gray-600 group-hover:border-cyan-500/50 group-hover:text-cyan-400'
                  } transition-colors`}>
                    {item.done ? '\u2713' : item.step}
                  </span>
                  <p className="text-sm text-gray-300 group-hover:text-gray-100 transition-colors">{item.title}</p>
                  <span className="ml-auto text-gray-700 group-hover:text-gray-500 transition-colors">&rarr;</span>
                </Link>
              ))}
            </div>
          </div>

          {/* Show infrastructure if there are agents */}
          {agents.length > 0 && (
            <InfraSection agents={agents} endpoints={endpoints} projectId={projectId} />
          )}

          {/* Show recent runs if any non-queued exist */}
          {nonQueuedRuns.length > 0 && (
            <RecentRunsSection runs={nonQueuedRuns} projectId={projectId} />
          )}
        </div>
      ) : (
        /* ── Normal data: single-column when no live events, 2-col when live ── */
        <div className={recentEvents.length > 0 ? 'grid grid-cols-1 lg:grid-cols-3 gap-6' : 'max-w-4xl space-y-6'}>
          <div className={recentEvents.length > 0 ? 'lg:col-span-2 space-y-6' : 'space-y-6'}>
            {/* Infrastructure: combined targets + runners */}
            <InfraSection agents={agents} endpoints={endpoints} projectId={projectId} />

            {/* Recent Runs */}
            <RecentRunsSection runs={nonQueuedRuns} projectId={projectId} />
          </div>

          {/* Live Feed — only when there are events */}
          {recentEvents.length > 0 && (
            <div>
              <div className="flex items-center gap-2 mb-3">
                <span className="w-1.5 h-1.5 rounded-full bg-green-400 motion-safe:animate-pulse" />
                <h3 className="text-xs text-gray-500 tracking-wider font-medium">live feed</h3>
              </div>
              <div className="border border-gray-800/50 rounded max-h-64 overflow-y-auto">
                {recentEvents.map((event, i) => (
                  <div
                    key={`${event.type}-${event.job_id ?? event.agent_id ?? ''}-${i}`}
                    className="px-3 py-2 border-b border-gray-800/30 text-xs flex items-center gap-2"
                  >
                    <StatusBadge status={event.status || event.type} />
                    <span className="text-gray-500 font-mono">
                      {event.job_id?.slice(0, 8) || event.agent_id?.slice(0, 8)}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/* ── Infrastructure section: targets + runners combined ── */
function InfraSection({ agents, endpoints, projectId }: {
  agents: Agent[];
  endpoints: { host: string; version: string | null; reachable: boolean }[];
  projectId: string;
}) {
  return (
    <div>
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-xs text-gray-500 tracking-wider font-medium">infrastructure</h3>
        <Link to={`/projects/${projectId}/vms`} className="text-xs text-gray-600 hover:text-gray-400 transition-colors">View all &rarr;</Link>
      </div>

      {endpoints.length === 0 && agents.length === 0 ? (
        <div className="border border-gray-800 rounded p-6 text-center">
          <p className="text-gray-600 text-sm">No runners or targets deployed yet</p>
          <Link to={`/projects/${projectId}/deploy`} className="text-xs text-cyan-400 mt-1 inline-block">Deploy your first target</Link>
        </div>
      ) : (
        <div className="space-y-2">
          {/* Targets */}
          {endpoints.length > 0 && (
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

          {/* Runners */}
          {agents.length > 0 && (() => {
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
                    to={`/projects/${projectId}/vms`}
                    className="block text-xs text-gray-500 hover:text-cyan-400 mt-2 text-center"
                  >
                    + {hidden} more -- view all runners &rarr;
                  </Link>
                )}
              </>
            );
          })()}
        </div>
      )}
    </div>
  );
}

/* ── Recent Runs section ── */
function RecentRunsSection({ runs, projectId }: { runs: TestRun[]; projectId: string }) {
  return (
    <div>
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-xs text-gray-500 tracking-wider font-medium">recent runs</h3>
        <Link to={`/projects/${projectId}/runs`} className="text-xs text-gray-600 hover:text-gray-400 transition-colors">View all &rarr;</Link>
      </div>
      {runs.length === 0 ? (
        <div className="border border-gray-800 rounded p-6 text-center">
          <p className="text-gray-600 text-sm">No completed runs yet</p>
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
              {runs.map(run => (
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
  );
}
