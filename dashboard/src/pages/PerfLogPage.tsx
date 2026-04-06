import { useState, useCallback, useMemo, useRef } from 'react';
import { api } from '../api/client';
import type { PerfLogRow, PerfLogStats } from '../api/types';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { stableSet } from '../lib/stableUpdate';

type Tab = 'logs' | 'stats';
type Kind = 'all' | 'api' | 'render';

function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '-';
  if (ms < 1) return '<1ms';
  return `${ms.toFixed(1)}ms`;
}

function speedColor(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return 'text-gray-500';
  if (ms < 50) return 'text-green-400';
  if (ms < 200) return 'text-yellow-400';
  if (ms < 500) return 'text-orange-400';
  return 'text-red-400';
}

function renderSpeedColor(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return 'text-gray-500';
  if (ms < 16) return 'text-green-400';
  if (ms < 50) return 'text-yellow-400';
  return 'text-red-400';
}

function StatCard({ label, value, color, sub }: { label: string; value: string; color: string; sub?: string }) {
  return (
    <div className="bg-[var(--bg-surface)] border border-gray-800 rounded-lg p-4">
      <p className="text-[10px] text-gray-500 tracking-wider uppercase mb-1">{label}</p>
      <p className={`text-2xl font-mono font-bold ${color}`}>{value}</p>
      {sub && <p className="text-xs text-gray-600 mt-1">{sub}</p>}
    </div>
  );
}

const PAGE_SIZE = 50;

export function PerfLogPage() {
  const [tab, setTab] = useState<Tab>('logs');
  const [logs, setLogs] = useState<PerfLogRow[]>([]);
  const [stats, setStats] = useState<PerfLogStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [kindFilter, setKindFilter] = useState<Kind>('all');
  const [pathFilter, setPathFilter] = useState('');
  const [page, setPage] = useState(0);
  const logsFingerprint = useRef('');
  const statsFingerprint = useRef('');

  usePageTitle('Performance Log');

  const loadLogs = useCallback(() => {
    const params: { kind?: string; path?: string; limit?: number } = { limit: 200 };
    if (kindFilter !== 'all') params.kind = kindFilter;
    if (pathFilter.trim()) params.path = pathFilter.trim();
    api.getPerfLogs(params).then(data => {
      stableSet(setLogs, data, logsFingerprint);
      setLoading(false);
    }).catch(() => setLoading(false));
  }, [kindFilter, pathFilter]);

  const loadStats = useCallback(() => {
    api.getPerfLogStats().then((data: PerfLogStats) => {
      const fp = JSON.stringify(data);
      if (fp !== statsFingerprint.current) {
        statsFingerprint.current = fp;
        setStats(data);
      }
    }).catch(() => {});
  }, []);

  usePolling(loadLogs, 15000);
  usePolling(loadStats, 15000);

  // Reset page when filters change
  const activeFilterCount = [kindFilter !== 'all', pathFilter].filter(Boolean).length;
  const totalPages = Math.max(1, Math.ceil(logs.length / PAGE_SIZE));
  const currentPage = Math.min(page, totalPages - 1);
  const pagedLogs = logs.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);

  // Compute top slow paths
  const topSlowPaths = useMemo(() => {
    const apiLogs = logs.filter(l => l.kind === 'api' && l.total_ms !== null);
    const byPath = new Map<string, { count: number; totalMs: number; maxMs: number }>();
    for (const l of apiLogs) {
      const key = l.path || '?';
      const existing = byPath.get(key) || { count: 0, totalMs: 0, maxMs: 0 };
      existing.count++;
      existing.totalMs += l.total_ms!;
      existing.maxMs = Math.max(existing.maxMs, l.total_ms!);
      byPath.set(key, existing);
    }
    return [...byPath.entries()]
      .map(([path, data]) => ({ path, ...data, avgMs: data.totalMs / data.count }))
      .sort((a, b) => b.avgMs - a.avgMs)
      .slice(0, 10);
  }, [logs]);

  if (loading && logs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Performance Log</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading performance data...</div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Performance Log</h2>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setTab('logs')}
            className={`px-3 py-1 text-xs rounded border ${tab === 'logs' ? 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5' : 'border-gray-700 text-gray-500'}`}
          >
            Logs ({logs.length})
          </button>
          <button
            onClick={() => setTab('stats')}
            className={`px-3 py-1 text-xs rounded border ${tab === 'stats' ? 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5' : 'border-gray-700 text-gray-500'}`}
          >
            Stats
          </button>
        </div>
      </div>

      {tab === 'logs' && (
        <>
          <FilterBar
            activeCount={activeFilterCount}
            onClearAll={() => { setKindFilter('all'); setPathFilter(''); }}
            chips={
              <>
                {kindFilter !== 'all' && <FilterChip label="Kind" value={kindFilter} onClear={() => setKindFilter('all')} />}
                {pathFilter && <FilterChip label="Path" value={pathFilter} onClear={() => setPathFilter('')} />}
              </>
            }
          >
            <select
              value={kindFilter}
              onChange={e => setKindFilter(e.target.value as Kind)}
              className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
            >
              <option value="all">All types</option>
              <option value="api">API only</option>
              <option value="render">Render only</option>
            </select>
            <input
              type="search"
              value={pathFilter}
              onChange={e => setPathFilter(e.target.value)}
              placeholder="Filter by path..."
              className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-48 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </FilterBar>

          <div className="table-container mt-4">
            <table className="w-full text-sm font-mono">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="px-3 py-2 text-left font-medium">Time</th>
                  <th className="px-3 py-2 text-left font-medium w-14">Kind</th>
                  <th className="px-3 py-2 text-left font-medium">Path / Component</th>
                  <th className="px-3 py-2 text-left font-medium w-14">Status</th>
                  <th className="px-3 py-2 text-right font-medium w-20">Total</th>
                  <th className="px-3 py-2 text-right font-medium w-20">Server</th>
                  <th className="px-3 py-2 text-right font-medium w-20">Net/Render</th>
                  <th className="px-3 py-2 text-left font-medium w-16 hidden lg:table-cell">Source</th>
                </tr>
              </thead>
              <tbody>
                {pagedLogs.map(log => {
                  // Shorten UUID paths for readability
                  const displayPath = log.path?.replace(
                    /\/projects\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
                    '/projects/…'
                  );
                  return (
                  <tr key={log.id} className="border-b border-gray-800/30 hover:bg-gray-800/20">
                    <td className="px-3 py-2 text-gray-500 text-xs">{new Date(log.logged_at).toLocaleString()}</td>
                    <td className="px-3 py-2">
                      <span className={`text-[10px] uppercase px-1.5 py-0.5 rounded border ${
                        log.kind === 'api'
                          ? 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5'
                          : 'border-green-500/30 text-green-400 bg-green-500/5'
                      }`}>
                        {log.kind}
                      </span>
                    </td>
                    <td className="px-3 py-2 text-gray-300 text-xs truncate max-w-[250px]" title={log.path || log.component || ''}>
                      {log.kind === 'api' ? (
                        <><span className="text-gray-500">{log.method} </span>{displayPath}</>
                      ) : (
                        <><span className="text-gray-400">{log.component}</span> <span className="text-gray-600">{log.trigger}</span></>
                      )}
                    </td>
                    <td className="px-3 py-2 text-xs">
                      {log.kind === 'api' ? (
                        <span className={log.status && log.status < 300 ? 'text-green-400' : log.status && log.status < 400 ? 'text-yellow-400' : 'text-red-400'}>
                          {log.status || '-'}
                        </span>
                      ) : (
                        <span className="text-gray-600">{log.item_count ?? '-'}</span>
                      )}
                    </td>
                    <td className={`px-3 py-2 text-right text-xs ${log.kind === 'api' ? speedColor(log.total_ms) : renderSpeedColor(log.render_ms)}`}>
                      {log.kind === 'api' ? formatMs(log.total_ms) : formatMs(log.render_ms)}
                    </td>
                    <td className="px-3 py-2 text-right text-xs text-cyan-400">
                      {log.kind === 'api' ? formatMs(log.server_ms) : '-'}
                    </td>
                    <td className="px-3 py-2 text-right text-xs text-purple-400">
                      {log.kind === 'api' ? formatMs(log.network_ms) : formatMs(log.render_ms)}
                    </td>
                    <td className="px-3 py-2 text-xs text-gray-600 hidden lg:table-cell">
                      {log.source || '-'}
                    </td>
                  </tr>
                  );
                })}
              </tbody>
            </table>
            {logs.length === 0 && (
              <div className="py-10 text-center text-gray-500 text-sm">
                No performance logs recorded yet. Logs are flushed every 30 seconds.
              </div>
            )}
            {totalPages > 1 && (
              <div className="flex items-center justify-between px-3 py-2 border-t border-gray-800/50 text-xs text-gray-500">
                <span>{logs.length} rows &middot; page {currentPage + 1} of {totalPages}</span>
                <div className="flex items-center gap-1">
                  <button
                    onClick={() => setPage(0)}
                    disabled={currentPage === 0}
                    className="px-2 py-1 rounded border border-gray-700 disabled:opacity-30 hover:border-cyan-500/30 hover:text-cyan-400 transition-colors"
                  >
                    &laquo;
                  </button>
                  <button
                    onClick={() => setPage(p => Math.max(0, p - 1))}
                    disabled={currentPage === 0}
                    className="px-2 py-1 rounded border border-gray-700 disabled:opacity-30 hover:border-cyan-500/30 hover:text-cyan-400 transition-colors"
                  >
                    &lsaquo; Prev
                  </button>
                  <button
                    onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))}
                    disabled={currentPage >= totalPages - 1}
                    className="px-2 py-1 rounded border border-gray-700 disabled:opacity-30 hover:border-cyan-500/30 hover:text-cyan-400 transition-colors"
                  >
                    Next &rsaquo;
                  </button>
                  <button
                    onClick={() => setPage(totalPages - 1)}
                    disabled={currentPage >= totalPages - 1}
                    className="px-2 py-1 rounded border border-gray-700 disabled:opacity-30 hover:border-cyan-500/30 hover:text-cyan-400 transition-colors"
                  >
                    &raquo;
                  </button>
                </div>
              </div>
            )}
          </div>
        </>
      )}

      {tab === 'stats' && stats && (
        <div className="space-y-6">
          {/* Summary cards */}
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
            <StatCard
              label="API Requests"
              value={String(stats.api_count)}
              color="text-cyan-400"
              sub={`${stats.slow_api_count} slow (>200ms)`}
            />
            <StatCard
              label="Avg Total"
              value={formatMs(stats.avg_total_ms)}
              color={speedColor(stats.avg_total_ms)}
              sub={`P95: ${formatMs(stats.p95_total_ms)}`}
            />
            <StatCard
              label="Renders"
              value={String(stats.render_count)}
              color="text-green-400"
              sub={`${stats.janky_render_count} janky (>16ms)`}
            />
            <StatCard
              label="Avg Render"
              value={formatMs(stats.avg_render_ms)}
              color={renderSpeedColor(stats.avg_render_ms)}
              sub={`P95: ${formatMs(stats.p95_render_ms)}`}
            />
          </div>

          {/* Breakdown */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
            <StatCard
              label="Avg Server Time"
              value={formatMs(stats.avg_server_ms)}
              color="text-cyan-400"
            />
            <StatCard
              label="Avg Network Time"
              value={stats.avg_total_ms && stats.avg_server_ms ? formatMs(stats.avg_total_ms - stats.avg_server_ms) : '-'}
              color="text-purple-400"
            />
            <StatCard
              label="Server % of Total"
              value={stats.avg_total_ms && stats.avg_server_ms ? `${((stats.avg_server_ms / stats.avg_total_ms) * 100).toFixed(0)}%` : '-'}
              color="text-gray-300"
            />
          </div>

          {/* Top slow paths */}
          {topSlowPaths.length > 0 && (
            <div>
              <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3 uppercase">Slowest API Paths</h3>
              <div className="table-container">
                <table className="w-full text-sm font-mono">
                  <thead>
                    <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                      <th className="px-3 py-2 text-left font-medium">Path</th>
                      <th className="px-3 py-2 text-right font-medium">Calls</th>
                      <th className="px-3 py-2 text-right font-medium">Avg</th>
                      <th className="px-3 py-2 text-right font-medium">Max</th>
                    </tr>
                  </thead>
                  <tbody>
                    {topSlowPaths.map(p => (
                      <tr key={p.path} className="border-b border-gray-800/30">
                        <td className="px-3 py-2 text-gray-300 text-xs truncate max-w-[300px]" title={p.path}>
                          {p.path.replace(/\/projects\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, '/projects/…')}
                        </td>
                        <td className="px-3 py-2 text-right text-gray-500 text-xs">{p.count}</td>
                        <td className={`px-3 py-2 text-right text-xs ${speedColor(p.avgMs)}`}>{formatMs(p.avgMs)}</td>
                        <td className={`px-3 py-2 text-right text-xs ${speedColor(p.maxMs)}`}>{formatMs(p.maxMs)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
