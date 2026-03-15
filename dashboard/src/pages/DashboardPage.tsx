import { useState } from 'react';
import { api } from '../api/client';
import { StatCard } from '../components/cards/StatCard';
import { StatusBadge } from '../components/common/StatusBadge';
import { useLiveStore } from '../stores/liveStore';
import { usePolling } from '../hooks/usePolling';

interface Summary {
  agents_online: number;
  jobs_running: number;
  runs_24h: number;
  jobs_pending: number;
}

export function DashboardPage() {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const events = useLiveStore((s) => s.events);

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

  const recentEvents = events.slice(-20).reverse();

  if (loading && !summary) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Dashboard</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading dashboard...</div>
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

  return (
    <div className="p-6">
      <h2 className="text-xl font-bold text-gray-100 mb-6">Dashboard</h2>

      {/* Stat cards */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
        <StatCard
          label="Agents Online"
          value={summary?.agents_online ?? '-'}
          accent="text-green-400"
        />
        <StatCard
          label="Jobs Running"
          value={summary?.jobs_running ?? '-'}
          accent="text-cyan-400"
        />
        <StatCard
          label="Runs (24h)"
          value={summary?.runs_24h ?? '-'}
          accent="text-blue-400"
        />
        <StatCard
          label="Pending"
          value={summary?.jobs_pending ?? '-'}
          accent="text-yellow-400"
        />
      </div>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh: {error}
        </div>
      )}

      {/* Live event feed */}
      <div className="bg-[#12131a] border border-gray-800 rounded-lg">
        <div className="px-4 py-3 border-b border-gray-800 flex items-center gap-2">
          <span className="w-2 h-2 rounded-full bg-green-400 motion-safe:animate-pulse" />
          <h3 className="text-sm text-gray-300 font-medium">Live Feed</h3>
        </div>
        <div className="max-h-96 overflow-y-auto">
          {recentEvents.length === 0 ? (
            <p className="p-4 text-gray-600 text-sm">
              Waiting for events...
            </p>
          ) : (
            recentEvents.map((event, i) => (
              <div
                key={`${event.type}-${event.job_id ?? event.agent_id ?? ''}-${i}`}
                className="px-4 py-2 border-b border-gray-800/50 text-sm flex items-center gap-3"
              >
                <StatusBadge status={event.status || event.type} />
                <span className="text-gray-400 font-mono text-xs">
                  {event.job_id?.slice(0, 8) || event.agent_id?.slice(0, 8)}
                </span>
                <span className="text-gray-500 text-xs">
                  {event.type}
                </span>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
