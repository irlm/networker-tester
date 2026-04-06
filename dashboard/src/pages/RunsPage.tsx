import { useState, useCallback, useMemo, useRef } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api, type RunSummary } from '../api/client';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { useDebounce } from '../hooks/useDebounce';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useRenderLog } from '../hooks/useRenderLog';
import { stableSet } from '../lib/stableUpdate';
import { useProject } from '../hooks/useProject';

const MODE_OPTIONS = ['all', 'http1', 'http2', 'http3', 'dns', 'tls', 'udp', 'pageload', 'browser'] as const;
const RESULT_OPTIONS = ['all', 'success', 'failed', 'mixed'] as const;

export function RunsPage() {
  const { projectId } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const runsFingerprint = useRef('');

  const targetSearch = searchParams.get('host') || '';
  const modeFilter = searchParams.get('mode') || 'all';
  const resultFilter = searchParams.get('result') || 'all';

  // Debounce text search — only hit the API after 300ms of inactivity
  const debouncedHost = useDebounce(targetSearch, 300);
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
  }, [setSearchParams, markRender]);

  const clearAllFilters = useCallback(() => {
    setSearchParams({}, { replace: true });
  }, [setSearchParams]);

  usePageTitle('Runs');

  const loadRuns = useCallback(() => {
    if (!projectId) return;
    const params: { target_host?: string; mode?: string; limit?: number } = { limit: 50 };
    if (debouncedHost.trim()) params.target_host = debouncedHost.trim();
    if (modeFilter !== 'all') params.mode = modeFilter;
    api
      .getRuns(projectId, params)
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
  }, [debouncedHost, modeFilter, projectId, markRender]);

  usePolling(loadRuns, 15000);

  // Precompute formatted dates to avoid Date object creation per render
  const runsWithDates = useMemo(() =>
    runs.map(r => ({
      ...r,
      _startedTime: new Date(r.started_at).toLocaleTimeString(),
      _startedFull: new Date(r.started_at).toLocaleString(),
    })),
    [runs],
  );

  // Client-side result filter
  const filteredRuns = useMemo(() => {
    if (resultFilter === 'all') return runsWithDates;
    return runsWithDates.filter(r => {
      if (resultFilter === 'success') return r.failure_count === 0;
      if (resultFilter === 'failed') return r.success_count === 0 && r.failure_count > 0;
      return r.failure_count > 0 && r.success_count > 0; // mixed
    });
  }, [runsWithDates, resultFilter]);

  const activeFilterCount = [
    targetSearch,
    modeFilter !== 'all',
    resultFilter !== 'all',
  ].filter(Boolean).length;

  if (loading && runs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Test Runs</h2>
        <div className="table-container">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                <th className="px-3 py-2 text-left font-medium">Run ID</th>
                <th className="px-3 py-2 text-left font-medium">Target</th>
                <th className="px-3 py-2 text-left font-medium">Modes</th>
                <th className="px-3 py-2 text-left font-medium">Success</th>
                <th className="px-3 py-2 text-left font-medium">Failed</th>
                <th className="px-3 py-2 text-left font-medium">Started</th>
              </tr>
            </thead>
            <tbody>
              {[1, 2, 3, 4, 5].map(i => (
                <tr key={i} className="border-b border-gray-800/30">
                  <td className="px-3 py-3"><div className="h-3 w-16 bg-gray-800 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-32 bg-gray-800/60 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-20 bg-gray-800/60 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-8 bg-gray-800/40 rounded motion-safe:animate-pulse" /></td>
                  <td className="px-3 py-3"><div className="h-3 w-8 bg-gray-800/40 rounded motion-safe:animate-pulse" /></td>
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
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Test Runs</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load runs</h3>
          <p className="text-red-300 text-sm">Could not fetch test runs. Check your connection and try refreshing.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Test Runs</h2>
      </div>

      {/* ── Filter Bar ── */}
      <FilterBar
        activeCount={activeFilterCount}
        onClearAll={clearAllFilters}
        chips={
          <>
            {targetSearch && (
              <FilterChip label="Host" value={targetSearch} onClear={() => setFilter('host', '')} />
            )}
            {modeFilter !== 'all' && (
              <FilterChip label="Mode" value={modeFilter} onClear={() => setFilter('mode', 'all')} />
            )}
            {resultFilter !== 'all' && (
              <FilterChip label="Result" value={resultFilter} onClear={() => setFilter('result', 'all')} />
            )}
          </>
        }
      >
        {/* Target host search */}
        <input
          id="runs-target-search"
          type="search"
          value={targetSearch}
          onChange={(e) => setFilter('host', e.target.value)}
          placeholder="Filter by host..."
          aria-label="Search by target host"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-40 md:w-56 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
        />

        {/* Mode filter */}
        <select
          value={modeFilter}
          onChange={(e) => setFilter('mode', e.target.value)}
          aria-label="Filter by mode"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {MODE_OPTIONS.map(m => (
            <option key={m} value={m}>
              {m === 'all' ? 'All modes' : m.toUpperCase()}
            </option>
          ))}
        </select>

        {/* Result filter */}
        <select
          value={resultFilter}
          onChange={(e) => setFilter('result', e.target.value)}
          aria-label="Filter by result"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {RESULT_OPTIONS.map(r => (
            <option key={r} value={r}>
              {r === 'all' ? 'All results' : r.charAt(0).toUpperCase() + r.slice(1)}
            </option>
          ))}
        </select>
      </FilterBar>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 mt-4 text-yellow-400 text-sm">
          Failed to refresh runs. Retrying automatically.
        </div>
      )}

      {/* ── Mobile card layout (< md) ── */}
      <div className="md:hidden space-y-2 mt-4">
        {filteredRuns.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">{activeFilterCount > 0 ? 'No runs match filters' : 'No test runs yet'}</p>
            {activeFilterCount === 0 && (
              <p className="text-gray-700 text-xs mt-1">Runs appear here after a test completes. Start one from the Tests page.</p>
            )}
          </div>
        ) : filteredRuns.map((run) => (
          <Link
            key={run.run_id}
            to={`/projects/${projectId}/runs/${run.run_id}`}
            className="block border border-gray-800 rounded p-3"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="text-cyan-400 font-mono text-xs">{run.run_id.slice(0, 8)}</span>
              <div className="flex items-center gap-2 text-xs">
                <span className="text-green-400">{run.success_count} ok</span>
                {run.failure_count > 0 && (
                  <span className="text-red-400">{run.failure_count} fail</span>
                )}
              </div>
            </div>
            <p className="text-gray-300 text-xs truncate mb-1">{run.target_host}</p>
            <div className="flex items-center gap-3 text-xs text-gray-500">
              <span>{run.modes}</span>
              <span>{run._startedTime}</span>
            </div>
          </Link>
        ))}
      </div>

      {/* ── Desktop/iPad table (≥ md) ── */}
      <div className="hidden md:block table-container mt-4">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Run ID</th>
              <th className="px-4 py-2.5 text-left font-medium">Target</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Modes</th>
              <th className="px-4 py-2.5 text-left font-medium">Success</th>
              <th className="px-4 py-2.5 text-left font-medium">Failed</th>
              <th className="px-4 py-2.5 text-left font-medium">Started</th>
            </tr>
          </thead>
          <tbody>
            {filteredRuns.map((run) => (
              <tr
                key={run.run_id}
                className="border-b border-gray-800/50 hover:bg-gray-800/20"
              >
                <td className="px-4 py-3">
                  <Link
                    to={`/projects/${projectId}/runs/${run.run_id}`}
                    className="text-cyan-400 hover:underline font-mono text-xs"
                  >
                    {run.run_id.slice(0, 8)}
                  </Link>
                </td>
                <td className="px-4 py-3 text-gray-300 text-xs truncate max-w-48">{run.target_host}</td>
                <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">{run.modes}</td>
                <td className="px-4 py-3 text-green-400">{run.success_count}</td>
                <td className="px-4 py-3 text-red-400">
                  {run.failure_count > 0 ? run.failure_count : '-'}
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs">
                  {run._startedFull}
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        {filteredRuns.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">{activeFilterCount > 0 ? 'No runs match the current filters' : 'No test runs yet'}</p>
            {activeFilterCount === 0 && (
              <p className="text-gray-700 text-xs mt-1">Runs appear here after a test completes. Start one from the Tests page.</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
