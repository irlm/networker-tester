import { useState, useCallback, useMemo, useRef } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { TestRun, RunStatus, EndpointKind } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useRenderLog } from '../hooks/useRenderLog';
import { stableSet } from '../lib/stableUpdate';
import { timeAgo } from '../lib/format';
import { useProject } from '../hooks/useProject';

const STATUS_OPTIONS: Array<RunStatus | 'all'> = ['all', 'queued', 'running', 'completed', 'failed', 'cancelled'];
const ARTIFACT_OPTIONS = ['all', 'yes', 'no'] as const;

const PAGE_SIZE = 20;

const KIND_BADGE_CLASSES: Record<string, string> = {
  network: 'text-cyan-400 bg-cyan-500/10',
  proxy: 'text-purple-400 bg-purple-500/10',
  runtime: 'text-green-400 bg-green-500/10',
};

function KindBadge({ kind }: { kind: string | null | undefined }) {
  if (!kind) return <span className="text-gray-600">-</span>;
  const classes = KIND_BADGE_CLASSES[kind] || 'text-gray-400 bg-gray-500/10';
  return (
    <span className={`text-[10px] font-medium px-1.5 py-0.5 rounded ${classes}`}>
      {kind}
    </span>
  );
}

export function RunsPage() {
  const { projectId } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [runs, setRuns] = useState<TestRun[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const runsFingerprint = useRef('');

  const statusFilter = searchParams.get('status') || 'all';
  const endpointKindFilter = searchParams.get('endpoint_kind') || 'all';
  const artifactFilter = searchParams.get('has_artifact') || 'all';
  const showQueued = searchParams.get('show_queued') === '1';

  const markRender = useRenderLog('RunsPage');

  const setFilter = useCallback((key: string, value: string) => {
    markRender(`filter:${key}`);
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (!value || value === 'all') {
        next.delete(key);
      } else {
        next.set(key, value);
      }
      return next;
    }, { replace: true });
    setPage(0);
  }, [setSearchParams, markRender]);

  const toggleShowQueued = useCallback(() => {
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (prev.get('show_queued') === '1') {
        next.delete('show_queued');
      } else {
        next.set('show_queued', '1');
      }
      return next;
    }, { replace: true });
    setPage(0);
  }, [setSearchParams]);

  const clearAllFilters = useCallback(() => {
    setSearchParams({}, { replace: true });
    setPage(0);
  }, [setSearchParams]);

  usePageTitle('Runs');

  const loadRuns = useCallback(() => {
    if (!projectId) return;
    const params: {
      status?: string;
      endpoint_kind?: string;
      has_artifact?: boolean;
      limit?: number;
    } = { limit: 200 };
    if (statusFilter !== 'all') params.status = statusFilter;
    if (endpointKindFilter !== 'all') params.endpoint_kind = endpointKindFilter;
    if (artifactFilter === 'yes') params.has_artifact = true;
    if (artifactFilter === 'no') params.has_artifact = false;
    api
      .listTestRuns(projectId, params)
      .then((data) => {
        const changed = stableSet(setRuns, data, runsFingerprint);
        if (changed) markRender('api:runs', data.length);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [statusFilter, endpointKindFilter, artifactFilter, projectId, markRender]);

  usePolling(loadRuns, 15000);

  // Filter out queued unless opted in
  const filteredRuns = useMemo(() => {
    if (showQueued || statusFilter === 'queued') return runs;
    return runs.filter(r => r.status !== 'queued');
  }, [runs, showQueued, statusFilter]);

  // Kind counts for tabs
  const kindCounts = useMemo(() => {
    const counts: Record<string, number> = { all: filteredRuns.length, network: 0, proxy: 0, runtime: 0 };
    for (const r of filteredRuns) {
      const k = r.endpoint_kind;
      if (k && k in counts) counts[k]++;
    }
    return counts;
  }, [filteredRuns]);

  // Precompute formatted dates + apply kind tab filter
  const runsWithDates = useMemo(() => {
    const source = endpointKindFilter !== 'all'
      ? filteredRuns.filter(r => r.endpoint_kind === endpointKindFilter)
      : filteredRuns;
    return source.map(r => ({
      ...r,
      _createdAgo: timeAgo(r.created_at),
      _createdIso: new Date(r.created_at).toISOString(),
    }));
  }, [filteredRuns, endpointKindFilter]);

  // Pagination
  const totalPages = Math.max(1, Math.ceil(runsWithDates.length / PAGE_SIZE));
  const safePage = Math.min(page, totalPages - 1);
  const pageStart = safePage * PAGE_SIZE;
  const pageEnd = Math.min(pageStart + PAGE_SIZE, runsWithDates.length);
  const pageRuns = runsWithDates.slice(pageStart, pageEnd);

  const activeFilterCount = [
    statusFilter !== 'all',
    endpointKindFilter !== 'all',
    artifactFilter !== 'all',
  ].filter(Boolean).length;

  if (loading && runs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Runs</h2>
        <div className="table-container">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                <th className="px-3 py-2 text-left font-medium">Run</th>
                <th className="px-3 py-2 text-left font-medium">Name</th>
                <th className="px-3 py-2 text-left font-medium">Kind</th>
                <th className="px-3 py-2 text-left font-medium">Status</th>
                <th className="px-3 py-2 text-left font-medium">Result</th>
                <th className="px-3 py-2 text-left font-medium">Created</th>
              </tr>
            </thead>
            <tbody>
              {[1, 2, 3, 4, 5].map(i => (
                <tr key={i} className="border-b border-gray-800/30">
                  <td className="px-3 py-3"><div className="h-3 w-16 bg-gray-800 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-32 bg-gray-800/60 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-16 bg-gray-800/60 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-20 bg-gray-800/40 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-16 bg-gray-800/40 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-24 bg-gray-800/40 rounded motion-safe:animate-pulse" /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    );
  }

  if (error && runs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Runs</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load runs</h3>
          <p className="text-red-300 text-sm">Could not fetch test runs. Check your connection and try refreshing.</p>
        </div>
      </div>
    );
  }

  const kindTabs: Array<{ key: EndpointKind | 'all'; label: string }> = [
    { key: 'all', label: 'All' },
    { key: 'network', label: 'Network' },
    { key: 'proxy', label: 'Proxy' },
    { key: 'runtime', label: 'Runtime' },
  ];

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Runs</h2>
        <Link
          to={`/projects/${projectId}/tests/new`}
          className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-1.5 rounded text-sm transition-colors flex-shrink-0"
        >
          New Run
        </Link>
      </div>

      {/* Kind tabs */}
      <div className="flex items-center gap-1 mb-4 border-b border-gray-800/50">
        {kindTabs.map(tab => {
          const active = endpointKindFilter === tab.key;
          const count = kindCounts[tab.key] ?? 0;
          return (
            <button
              key={tab.key}
              onClick={() => setFilter('endpoint_kind', tab.key)}
              className={`px-3 py-2 text-xs font-medium border-b-2 transition-colors ${
                active
                  ? 'border-cyan-500 text-gray-100'
                  : 'border-transparent text-gray-500 hover:text-gray-300'
              }`}
            >
              {tab.label}
              <span className={`ml-1.5 tabular-nums ${active ? 'text-cyan-400' : 'text-gray-600'}`}>
                {count}
              </span>
            </button>
          );
        })}
      </div>

      {/* Filter Bar */}
      <FilterBar
        activeCount={activeFilterCount}
        onClearAll={clearAllFilters}
        chips={
          <>
            {statusFilter !== 'all' && (
              <FilterChip label="Status" value={statusFilter} onClear={() => setFilter('status', 'all')} />
            )}
            {artifactFilter !== 'all' && (
              <FilterChip label="Benchmark" value={artifactFilter === 'yes' ? 'Yes' : 'No'} onClear={() => setFilter('has_artifact', 'all')} />
            )}
          </>
        }
      >
        <select
          value={statusFilter}
          onChange={(e) => setFilter('status', e.target.value)}
          aria-label="Filter by status"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {STATUS_OPTIONS.map(s => (
            <option key={s} value={s}>
              {s === 'all' ? 'Any status' : s.charAt(0).toUpperCase() + s.slice(1)}
            </option>
          ))}
        </select>

        <select
          value={artifactFilter}
          onChange={(e) => setFilter('has_artifact', e.target.value)}
          aria-label="Filter by benchmark artifact"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {ARTIFACT_OPTIONS.map(a => (
            <option key={a} value={a}>
              {a === 'all' ? 'Any type' : a === 'yes' ? 'Benchmarks only' : 'Simple only'}
            </option>
          ))}
        </select>

        <label className="flex items-center gap-1.5 text-xs text-gray-500 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={showQueued}
            onChange={toggleShowQueued}
            className="rounded border-gray-600 bg-transparent text-cyan-500 focus:ring-cyan-500 focus:ring-offset-0 w-3.5 h-3.5"
          />
          Include queued
        </label>
      </FilterBar>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 mt-4 text-yellow-400 text-sm">
          Failed to refresh runs. Retrying automatically.
        </div>
      )}

      {/* Mobile card layout (< md) */}
      <div className="md:hidden space-y-2 mt-4">
        {pageRuns.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">{activeFilterCount > 0 ? 'No runs match filters' : 'No runs yet'}</p>
            {activeFilterCount === 0 && (
              <Link to={`/projects/${projectId}/tests/new`} className="text-cyan-400 text-xs mt-2 inline-block">
                Start a network test
              </Link>
            )}
          </div>
        ) : pageRuns.map((run) => (
          <Link
            key={run.id}
            to={`/projects/${projectId}/runs/${run.id}`}
            className="block border border-gray-800 rounded p-3"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="text-cyan-400 font-mono text-xs">{run.id.slice(0, 8)}</span>
              <StatusBadge status={run.status} />
            </div>
            <p className="text-gray-300 text-xs truncate mb-1">
              {run.config_name || run.test_config_id.slice(0, 8)}
            </p>
            <div className="flex items-center gap-3 text-xs text-gray-500">
              {run.endpoint_kind && <KindBadge kind={run.endpoint_kind} />}
              {run.artifact_id && <span className="text-purple-400">benchmark</span>}
              <span className="text-green-400">{run.success_count} ok</span>
              {run.failure_count > 0 && <span className="text-red-400">{run.failure_count} fail</span>}
              <span title={run._createdIso}>{run._createdAgo}</span>
            </div>
          </Link>
        ))}
      </div>

      {/* Desktop/iPad table (>= md) */}
      <div className="hidden md:block table-container mt-4">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Run</th>
              <th className="px-4 py-2.5 text-left font-medium">Name</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Type</th>
              <th className="px-4 py-2.5 text-left font-medium">Status</th>
              <th className="px-4 py-2.5 text-left font-medium">Result</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Modes</th>
              <th className="px-4 py-2.5 text-left font-medium">Created</th>
            </tr>
          </thead>
          <tbody>
            {pageRuns.map((run) => (
              <tr
                key={run.id}
                className="border-b border-gray-800/50 hover:bg-gray-800/20"
              >
                <td className="px-4 py-3">
                  <Link
                    to={`/projects/${projectId}/runs/${run.id}`}
                    className="text-cyan-400 hover:underline font-mono text-xs"
                  >
                    {run.id.slice(0, 8)}
                  </Link>
                  {run.artifact_id && (
                    <span className="ml-2 text-[10px] text-purple-400 bg-purple-500/10 px-1.5 py-0.5 rounded">benchmark</span>
                  )}
                </td>
                <td className="px-4 py-3 text-gray-300 text-xs truncate max-w-48">
                  {run.config_name || run.test_config_id.slice(0, 8)}
                </td>
                <td className="px-4 py-3 hidden lg:table-cell">
                  <KindBadge kind={run.endpoint_kind} />
                </td>
                <td className="px-4 py-3">
                  <StatusBadge status={run.status} />
                </td>
                <td className="px-4 py-3">
                  <span className="text-green-400">{run.success_count}</span>
                  {' / '}
                  <span className={run.failure_count > 0 ? 'text-red-400' : 'text-gray-600'}>
                    {run.failure_count}
                  </span>
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">
                  {run.modes?.join(', ') || '-'}
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs" title={run._createdIso}>
                  {run._createdAgo}
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        {runsWithDates.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">{activeFilterCount > 0 ? 'No runs match the current filters' : 'No runs yet'}</p>
            {activeFilterCount === 0 && (
              <Link to={`/projects/${projectId}/tests/new`} className="text-cyan-400 text-xs mt-1 inline-block">
                Start a network test
              </Link>
            )}
          </div>
        )}
      </div>

      {/* Pagination footer */}
      {runsWithDates.length > 0 && (
        <div className="flex items-center justify-between mt-4 text-xs text-gray-500">
          <span>
            Showing {pageStart + 1}-{pageEnd} of {runsWithDates.length} runs
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setPage(p => Math.max(0, p - 1))}
              disabled={safePage === 0}
              className="px-2.5 py-1 rounded border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              Previous
            </button>
            <span className="tabular-nums text-gray-400">
              {safePage + 1} / {totalPages}
            </span>
            <button
              onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))}
              disabled={safePage >= totalPages - 1}
              className="px-2.5 py-1 rounded border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
