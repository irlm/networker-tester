import { useCallback, useEffect, useMemo, useState } from 'react';
import { PageHeader } from '../components/common/PageHeader';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import {
  listVmHistory,
  type ResourceType,
  type VmLifecycleRow,
} from '../api/vmHistory';

/**
 * Project-scoped VM usage history (v0.27.20).
 *
 * Reads `GET /api/projects/{pid}/vm-history` and renders events in a
 * filterable table. Deliberately minimal for the first UI pass — no
 * detail drawer, no uptime aggregation yet. Follow-up PRs layer on:
 *   * Per-resource timeline drawer (v0.27.21)
 *   * Uptime + estimated cost column (v0.27.22, needs rate-table join)
 *   * Actual billing reconciled totals (v0.27.22+)
 */

const PAGE_SIZE = 100;

type TypeFilter = '' | ResourceType;

const EVENT_BADGE: Record<string, string> = {
  created: 'text-cyan-400 border-cyan-500/30 bg-cyan-500/10',
  started: 'text-green-400 border-green-500/30 bg-green-500/10',
  stopped: 'text-gray-400 border-gray-500/30 bg-gray-500/10',
  auto_shutdown: 'text-amber-400 border-amber-500/30 bg-amber-500/10',
  deleted: 'text-red-400 border-red-500/30 bg-red-500/10',
  error: 'text-red-400 border-red-500/30 bg-red-500/10',
};

const RESOURCE_BADGE: Record<string, string> = {
  tester: 'text-purple-400',
  endpoint: 'text-cyan-400',
  benchmark: 'text-emerald-400',
};

function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function EventBadge({ kind }: { kind: string }) {
  const cls =
    EVENT_BADGE[kind] ?? 'text-gray-400 border-gray-500/30 bg-gray-500/5';
  return (
    <span className={`inline-block text-[11px] px-2 py-0.5 rounded border font-mono ${cls}`}>
      {kind}
    </span>
  );
}

export function VmHistoryPage() {
  usePageTitle('VM History');
  const { projectId } = useProject();

  const [rows, setRows] = useState<VmLifecycleRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [typeFilter, setTypeFilter] = useState<TypeFilter>('');

  const refresh = useCallback(async () => {
    if (!projectId) return;
    setLoading(true);
    setError(null);
    try {
      const resp = await listVmHistory(projectId, {
        resource_type: typeFilter || undefined,
        limit: PAGE_SIZE,
      });
      setRows(resp.events);
      setHasMore(resp.has_more);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load VM history');
    } finally {
      setLoading(false);
    }
  }, [projectId, typeFilter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const loadMore = useCallback(async () => {
    if (!projectId || rows.length === 0) return;
    setLoadingMore(true);
    try {
      const resp = await listVmHistory(projectId, {
        resource_type: typeFilter || undefined,
        limit: PAGE_SIZE,
        offset: rows.length,
      });
      setRows((prev) => [...prev, ...resp.events]);
      setHasMore(resp.has_more);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load more');
    } finally {
      setLoadingMore(false);
    }
  }, [projectId, typeFilter, rows.length]);

  const counts = useMemo(() => {
    const c = { tester: 0, endpoint: 0, benchmark: 0 };
    for (const r of rows) {
      if (r.resource_type === 'tester') c.tester += 1;
      else if (r.resource_type === 'endpoint') c.endpoint += 1;
      else if (r.resource_type === 'benchmark') c.benchmark += 1;
    }
    return c;
  }, [rows]);

  return (
    <div className="p-4 md:p-6 max-w-6xl mx-auto">
      <PageHeader
        title="VM History"
        subtitle="Append-only lifecycle log for every VM this project has run — survives rename, delete, and cloud-account removal."
      />

      {/* Filter bar */}
      <div className="flex flex-wrap items-center gap-3 mb-4 text-xs">
        <span className="text-gray-500">Type</span>
        <div className="flex gap-1">
          {([
            ['', 'All'],
            ['tester', `Tester${counts.tester ? ` (${counts.tester})` : ''}`],
            ['endpoint', `Endpoint${counts.endpoint ? ` (${counts.endpoint})` : ''}`],
            ['benchmark', `Benchmark${counts.benchmark ? ` (${counts.benchmark})` : ''}`],
          ] as [TypeFilter, string][]).map(([val, label]) => (
            <button
              key={val || 'all'}
              type="button"
              onClick={() => setTypeFilter(val)}
              className={`px-2 py-1 rounded border font-mono ${
                typeFilter === val
                  ? 'border-cyan-500 text-cyan-400 bg-cyan-500/10'
                  : 'border-gray-700 text-gray-400 hover:border-gray-500'
              }`}
            >
              {label}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={() => void refresh()}
          className="ml-auto px-2 py-1 rounded border border-gray-700 text-gray-400 hover:text-cyan-400 hover:border-cyan-500"
        >
          Refresh
        </button>
      </div>

      {error && (
        <div
          role="alert"
          className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4"
        >
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {loading && rows.length === 0 ? (
        <p className="text-sm text-gray-500">Loading…</p>
      ) : rows.length === 0 ? (
        <div className="border border-gray-800 rounded p-6 text-center">
          <p className="text-sm text-gray-400">
            No VM lifecycle events recorded yet. Events start appearing here as
            soon as testers are created, started, stopped, or deleted.
          </p>
        </div>
      ) : (
        <div className="table-container">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-gray-500 border-b border-gray-800">
                <th className="px-3 py-2 text-left">When</th>
                <th className="px-3 py-2 text-left">Event</th>
                <th className="px-3 py-2 text-left">Resource</th>
                <th className="px-3 py-2 text-left">Type</th>
                <th className="px-3 py-2 text-left">Cloud / Region</th>
                <th className="px-3 py-2 text-left">VM size</th>
                <th className="px-3 py-2 text-left">VM name</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr
                  key={r.event_id}
                  className="border-b border-gray-800/30 hover:bg-gray-800/20"
                >
                  <td className="px-3 py-1.5 text-gray-400 font-mono whitespace-nowrap">
                    {formatTime(r.event_time)}
                  </td>
                  <td className="px-3 py-1.5">
                    <EventBadge kind={r.event_type} />
                  </td>
                  <td className="px-3 py-1.5 text-gray-200 font-mono truncate max-w-xs" title={r.resource_id}>
                    {r.resource_name ?? <span className="text-gray-600">(unnamed)</span>}
                  </td>
                  <td className="px-3 py-1.5">
                    <span className={`font-mono ${RESOURCE_BADGE[r.resource_type] ?? 'text-gray-400'}`}>
                      {r.resource_type}
                    </span>
                  </td>
                  <td className="px-3 py-1.5 text-gray-400 font-mono">
                    {r.cloud}
                    {r.region ? ` · ${r.region}` : ''}
                  </td>
                  <td className="px-3 py-1.5 text-gray-400 font-mono">
                    {r.vm_size ?? '—'}
                  </td>
                  <td className="px-3 py-1.5 text-gray-500 font-mono truncate max-w-xs" title={r.vm_resource_id ?? undefined}>
                    {r.vm_name ?? '—'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {hasMore && (
            <div className="p-3 text-center border-t border-gray-800">
              <button
                type="button"
                disabled={loadingMore}
                onClick={() => void loadMore()}
                className="px-4 py-1.5 text-xs rounded border border-gray-700 text-gray-400 hover:text-cyan-400 hover:border-cyan-500 disabled:opacity-50"
              >
                {loadingMore ? 'Loading…' : 'Load more'}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default VmHistoryPage;
