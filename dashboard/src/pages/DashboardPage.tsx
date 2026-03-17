import { useState, useMemo, useEffect } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
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
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const events = useLiveStore((s) => s.events);

  usePageTitle('Dashboard');

  useEffect(() => {
    api.getVersionInfo().then(setVersionInfo).catch(() => {});
  }, []);

  usePolling(() => {
    api
      .getDashboardSummary()
      .then((data) => {
        setSummary(data);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, 10000);

  const recentEvents = useMemo(() => events.slice(-20).reverse(), [events]);

  if (loading && !summary) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-2">Dashboard</h2>
        {/* Skeleton: metric bar */}
        <div className="flex gap-5 py-3 mb-6 border-b border-gray-800/50">
          {[80, 72, 68, 88].map((w, i) => (
            <div key={i} className="h-3 rounded motion-safe:animate-pulse bg-gray-800" style={{ width: w }} />
          ))}
        </div>
        {/* Skeleton: live feed */}
        <div className="h-3 w-16 rounded bg-gray-800 motion-safe:animate-pulse mb-3" />
        <div className="border border-gray-800/50 rounded">
          {[1, 2, 3, 4, 5].map(i => (
            <div key={i} className="px-3 py-3 border-b border-gray-800/30 flex gap-3">
              <div className="h-3 w-16 rounded bg-gray-800 motion-safe:animate-pulse" />
              <div className="h-3 w-12 rounded bg-gray-800/60 motion-safe:animate-pulse" />
              <div className="h-3 w-20 rounded bg-gray-800/40 motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (error && !summary) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Dashboard</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load dashboard</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  const hasActiveWork = (summary?.jobs_running ?? 0) > 0 || (summary?.jobs_pending ?? 0) > 0;

  return (
    <div className="p-6">
      {/* Page header with inline status */}
      <div className="flex items-baseline justify-between mb-2">
        <h2 className="text-xl font-bold text-gray-100">Dashboard</h2>
        {versionInfo && (
          <div className="flex items-center gap-3 text-xs text-gray-500">
            <span>v{versionInfo.dashboard_version}</span>
            {versionInfo.update_available && versionInfo.latest_release && (
              <Link to="/settings" className="text-yellow-400 hover:text-yellow-300 transition-colors">
                Update available →
              </Link>
            )}
          </div>
        )}
      </div>

      {/* Compact status bar — replaces stat card grid */}
      <div className="flex flex-wrap items-center gap-x-5 gap-y-1 py-3 mb-6 text-xs border-b border-gray-800/50">
        <span className="text-gray-500">
          Testers <span className={`font-mono ml-1 ${(summary?.agents_online ?? 0) > 0 ? 'text-green-400' : 'text-gray-600'}`}>{summary?.agents_online ?? 0}</span>
        </span>
        <span className="text-gray-500">
          Running <span className={`font-mono ml-1 ${(summary?.jobs_running ?? 0) > 0 ? 'text-blue-400' : 'text-gray-600'}`}>{summary?.jobs_running ?? 0}</span>
        </span>
        <span className="text-gray-500">
          Pending <span className={`font-mono ml-1 ${(summary?.jobs_pending ?? 0) > 0 ? 'text-yellow-400' : 'text-gray-600'}`}>{summary?.jobs_pending ?? 0}</span>
        </span>
        <span className="text-gray-500">
          Runs (24h) <span className="text-gray-300 font-mono ml-1">{summary?.runs_24h ?? 0}</span>
        </span>

        {/* Endpoint versions — inline */}
        {versionInfo?.endpoints.map(ep => {
          const outdated = ep.reachable && ep.version && versionInfo.latest_release && ep.version !== versionInfo.latest_release;
          return (
            <span key={ep.host} className="text-gray-600 ml-auto first:ml-auto">
              {ep.host.split('.')[0]}{' '}
              {ep.reachable ? (
                <span className={`font-mono ${outdated ? 'text-yellow-400' : 'text-green-400/60'}`}>
                  v{ep.version}
                </span>
              ) : (
                <span className="text-gray-700">offline</span>
              )}
            </span>
          );
        })}
      </div>

      {error && (
        <div className="text-yellow-400 text-xs mb-4 flex items-center gap-2">
          <span className="w-1.5 h-1.5 rounded-full bg-yellow-400" />
          Failed to refresh: {error}
        </div>
      )}

      {/* Active work highlight — only shown when tests are running */}
      {hasActiveWork && (
        <div className="mb-6 flex items-center gap-3 text-sm">
          <span className="w-2 h-2 rounded-full bg-blue-400 motion-safe:animate-pulse" />
          <span className="text-blue-400">
            {summary?.jobs_running ?? 0} running
          </span>
          {(summary?.jobs_pending ?? 0) > 0 && (
            <span className="text-gray-500">
              · {summary?.jobs_pending} queued
            </span>
          )}
          <Link to="/tests" className="text-gray-500 hover:text-gray-300 text-xs ml-auto transition-colors">
            View tests →
          </Link>
        </div>
      )}

      {/* Live event feed — primary content */}
      <div>
        <div className="flex items-center gap-2 mb-3">
          <span className="w-1.5 h-1.5 rounded-full bg-green-400 motion-safe:animate-pulse" />
          <h3 className="text-xs text-gray-500 uppercase tracking-wider font-medium">Live Feed</h3>
        </div>
        <div className="border border-gray-800/50 rounded max-h-[calc(100vh-220px)] overflow-y-auto">
          {recentEvents.length === 0 ? (
            <div className="py-12 text-center">
              <p className="text-gray-600 text-sm">Waiting for events...</p>
              <p className="text-gray-700 text-xs mt-1">Events from running tests will appear here</p>
            </div>
          ) : (
            <>
              {recentEvents.map((event, i) => (
                <div
                  key={`${event.type}-${event.job_id ?? event.agent_id ?? ''}-${i}`}
                  className="px-3 py-2 border-b border-gray-800/30 text-sm flex items-center gap-3 hover:bg-gray-800/10"
                >
                  <StatusBadge status={event.status || event.type} />
                  <span className="text-gray-400 font-mono text-xs">
                    {event.job_id?.slice(0, 8) || event.agent_id?.slice(0, 8)}
                  </span>
                  <span className="text-gray-600 text-xs">
                    {event.type}
                  </span>
                </div>
              ))}
              <div className="px-3 py-1.5 text-xs text-gray-700 text-center">
                last {recentEvents.length} events
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
