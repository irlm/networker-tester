import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { DataTable } from '../components/common/DataTable';
import { Link, useSearchParams } from 'react-router';
import type { TestRun, RunStatus, TestKind, EndpointKind } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { RunResult } from '../components/common/RunResult';
import { runDisplayStatus } from '../lib/runStatus';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { StatusFooter } from '../components/common/StatusFooter';
import { usePageTitle } from '../hooks/usePageTitle';
import { useRenderLog } from '../hooks/useRenderLog';
import { useNow } from '../hooks/useNow';
import { timeAgo } from '../lib/format';
import { useProject } from '../hooks/useProject';
import { runKeys, useComparisonGroupsQueries, useTestRunsQuery } from '../features/runs/queries';
import type { RunListParams } from '../features/runs/api';
import {
  clusterComparisonGroups,
  groupAggregateStatus,
  groupFallbackName,
  groupProgress,
  type RunListRow,
} from '../features/runs/list-grouping';
import { computeCellStats, stripCellNameSuffix } from '../features/runs/compare';
import { queryClient } from '../app/queryClient';
import { formatMs } from '../lib/analysis';
import type { LiveAttempt } from '../api/types';
import { PageShell } from '../components/common/PageShell';
import { Button } from '../components/common/Button';
import { buttonClassName } from '../components/common/button-styles';
import { Input, Select } from '../components/common/FormControls';
import { familyOf } from '../components/common/mode-family';

const STATUS_OPTIONS: Array<RunStatus | 'all'> = ['all', 'queued', 'provisioning', 'running', 'completed', 'failed', 'cancelled'];
const ARTIFACT_OPTIONS = ['all', 'yes', 'no'] as const;
const TIME_OPTIONS = [
  { value: 'all', label: 'All time' },
  { value: '1h', label: 'Last hour', milliseconds: 60 * 60 * 1000 },
  { value: '24h', label: 'Last 24 hours', milliseconds: 24 * 60 * 60 * 1000 },
  { value: '7d', label: 'Last 7 days', milliseconds: 7 * 24 * 60 * 60 * 1000 },
  { value: '30d', label: 'Last 30 days', milliseconds: 30 * 24 * 60 * 60 * 1000 },
] as const;
const MODE_FAMILY_OPTIONS = [
  { value: 'all', label: 'Any mode' },
  { value: 'net', label: 'Network' },
  { value: 'http', label: 'HTTP' },
  { value: 'thru', label: 'Throughput' },
  { value: 'page', label: 'Page load' },
  { value: 'app', label: 'Application' },
] as const;

const PAGE_SIZE = 20;

// Kind is a category, not a status — one neutral treatment so green/purple
// stay reserved for success/logo (audit F12: category hues fought the ramp).
const TARGET_BADGE_CLASSES: Record<string, string> = {
  network: 'text-cyan-400 bg-cyan-500/10',
  proxy: 'text-gray-300 bg-gray-500/10',
  runtime: 'text-gray-300 bg-gray-500/10',
};

const TEST_KIND_LABELS: Record<TestKind, string> = {
  network: 'Network test',
  url_probe: 'URL probe',
  sdk_probe: 'SDK probe',
  benchmark: 'Benchmark',
};

function TargetBadge({ kind }: { kind: string | null | undefined }) {
  if (!kind) return <span className="text-faint">-</span>;
  const classes = TARGET_BADGE_CLASSES[kind] || 'text-gray-400 bg-gray-500/10';
  return (
    <span className={`text-xs font-medium px-1.5 py-0.5 rounded ${classes}`}>
      {kind}
    </span>
  );
}

function PurposeBadge({ kind }: { kind: TestKind | undefined }) {
  if (!kind) return <span className="text-faint">-</span>;
  return (
    <span className="text-xs font-medium px-1.5 py-0.5 rounded text-gray-300 bg-gray-500/10">
      {TEST_KIND_LABELS[kind]}
    </span>
  );
}

/**
 * Fastest-so-far chip for a group row: min HTTP-total p50 among completed
 * cells, computed ONLY from attempts already sitting in the react-query cache
 * (the compare page — the group's primary surface — fills it). Fetching every
 * cell's attempts just for a list chip would be N extra requests per group;
 * requiring ≥2 cells with data keeps "fastest" honest.
 */
function fastestCachedCell(runs: TestRun[]): { label: string; p50: number } | null {
  let best: { label: string; p50: number } | null = null;
  let cellsWithStats = 0;
  for (const run of runs) {
    if (run.status !== 'completed') continue;
    const attempts = queryClient.getQueryData<LiveAttempt[]>(runKeys.attempts(run.id));
    if (!attempts || attempts.length === 0) continue;
    const p50 = computeCellStats(attempts).total?.p50 ?? null;
    if (p50 === null) continue;
    cellsWithStats += 1;
    if (!best || p50 < best.p50) {
      best = { label: stripCellNameSuffix(run.config_name ?? run.id.slice(0, 8)), p50 };
    }
  }
  return cellsWithStats >= 2 ? best : null;
}

function matchesModeFamily(modes: string[] | undefined, family: string): boolean {
  if (family === 'all') return true;
  if (family === 'app') return modes?.some(mode => mode.toLowerCase() === 'apibench') ?? false;
  return modes?.some(mode => familyOf(mode) === family) ?? false;
}

function RunNameSearch({ value, onCommit }: { value: string; onCommit: (value: string) => void }) {
  const [draft, setDraft] = useState(value);
  // Track what WE last committed so an external value change (tab switch,
  // back/forward, filter reset) can resync the draft — while our own debounce
  // round-trip through the URL leaves the input (and its focus/caret) alone.
  // Keying the component on the URL value remounted the input on every
  // debounce commit and dropped focus mid-typing.
  const lastCommitted = useRef(value);

  useEffect(() => {
    if (value !== lastCommitted.current) {
      lastCommitted.current = value;
      setDraft(value);
    }
  }, [value]);

  useEffect(() => {
    const nextQuery = draft.trim();
    if (nextQuery === value) return;
    const timer = window.setTimeout(() => {
      lastCommitted.current = nextQuery;
      onCommit(nextQuery);
    }, 250);
    return () => window.clearTimeout(timer);
  }, [draft, value, onCommit]);

  return (
    <Input
      type="search"
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      placeholder="Search run names"
      aria-label="Search runs by name"
      className="w-full sm:!w-56 py-1.5"
    />
  );
}

export function RunsPage() {
  const { projectId } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [page, setPage] = useState(0);
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const pendingSearchParams = useRef(searchParams);

  const statusFilter = (searchParams.get('status') || 'all') as RunStatus | 'all';
  const testKindFilter = (searchParams.get('test_kind') || 'all') as TestKind | 'all';
  const endpointKindFilter = (searchParams.get('endpoint_kind') || 'all') as EndpointKind | 'all';
  const modeFamilyFilter = searchParams.get('mode_family') || 'all';
  const timeFilter = searchParams.get('time') || 'all';
  const artifactFilter = searchParams.get('has_artifact') || 'all';
  // Queued runs are real runs the user cares about — show them by default so
  // the list never lies with "No runs yet" while jobs are actually piling up.
  // Opt-out via ?show_queued=0.
  const showQueued = searchParams.get('show_queued') !== '0';
  const comparisonGroupId = searchParams.get('comparison_group');
  const routeNameQuery = searchParams.get('q') || '';
  const nameQuery = routeNameQuery.trim().toLowerCase();

  const markRender = useRenderLog('RunsPage');
  const now = useNow(60_000);

  useEffect(() => {
    pendingSearchParams.current = searchParams;
  }, [searchParams]);

  // React Router does not queue consecutive functional search-param updates.
  // Keep the pending URL in a ref so fast tab/select interactions compose
  // instead of one filter silently restoring or dropping another.
  const updateSearchParams = useCallback((update: (next: URLSearchParams) => void) => {
    const next = new URLSearchParams(pendingSearchParams.current);
    update(next);
    pendingSearchParams.current = next;
    setSearchParams(next, { replace: true });
    setPage(0);
  }, [setSearchParams]);

  const setFilter = useCallback((key: string, value: string) => {
    markRender(`filter:${key}`);
    updateSearchParams(next => {
      if (!value || (value === 'all' && key !== 'q')) {
        next.delete(key);
      } else {
        next.set(key, value);
      }
    });
  }, [updateSearchParams, markRender]);

  const commitNameQuery = useCallback((value: string) => setFilter('q', value), [setFilter]);

  const toggleShowQueued = useCallback(() => {
    updateSearchParams(next => {
      // Default is now "show" — store '0' to explicitly hide.
      if (next.get('show_queued') === '0') {
        next.delete('show_queued');
      } else {
        next.set('show_queued', '0');
      }
    });
  }, [updateSearchParams]);

  const clearAllFilters = useCallback(() => {
    updateSearchParams(next => {
      for (const key of [...next.keys()]) next.delete(key);
    });
  }, [updateSearchParams]);

  usePageTitle('Runs');

  const params = useMemo<RunListParams>(() => {
    const next: RunListParams = { limit: 200 };
    if (statusFilter !== 'all') next.status = statusFilter;
    if (testKindFilter !== 'all') next.test_kind = testKindFilter;
    if (endpointKindFilter !== 'all') next.endpoint_kind = endpointKindFilter;
    if (routeNameQuery.trim()) next.q = routeNameQuery.trim();
    if (artifactFilter === 'yes') next.has_artifact = true;
    if (artifactFilter === 'no') next.has_artifact = false;
    if (comparisonGroupId) next.comparison_group_id = comparisonGroupId;
    const selectedTime = TIME_OPTIONS.find(option => option.value === timeFilter);
    if (selectedTime && 'milliseconds' in selectedTime) {
      next.since = new Date(now - selectedTime.milliseconds).toISOString();
    }
    return next;
  }, [statusFilter, testKindFilter, endpointKindFilter, artifactFilter, comparisonGroupId, routeNameQuery, timeFilter, now]);

  const [paused, setPaused] = useState(false);
  const runsQuery = useTestRunsQuery(projectId, params, { polling: !paused });
  const runs = useMemo<TestRun[]>(() => runsQuery.data ?? [], [runsQuery.data]);

  // During a rolling deploy, a cached browser can briefly receive an older run
  // shape. Keep a conservative display fallback; the new API always returns
  // test_kind, endpoint_kind, and modes directly, avoiding a second config list.
  const runsEnriched = useMemo(() => {
    return runs.map((r) => {
      return {
        ...r,
        test_kind: r.test_kind
          || (r.modes?.some(mode => mode.toLowerCase() === 'sdkprobe') ? 'sdk_probe'
            : r.artifact_id ? 'benchmark' : r.config_name?.startsWith('Diag: ') ? 'url_probe' : 'network'),
      };
    });
  }, [runs]);

  // Filter out queued unless opted in. Scoped to a comparison group, show
  // everything (the user just launched the group and expects to see its runs).
  const visibleQueuedRuns = useMemo(() => {
    if (showQueued || statusFilter === 'queued' || comparisonGroupId) return runsEnriched;
    return runsEnriched.filter(r => r.status !== 'queued');
  }, [runsEnriched, showQueued, statusFilter, comparisonGroupId]);

  // Apply secondary mode filtering to the server-filtered run window.
  const secondaryFilteredRuns = useMemo(() => {
    const selectedTime = TIME_OPTIONS.find(option => option.value === timeFilter);
    const cutoff = selectedTime && 'milliseconds' in selectedTime
      ? now - selectedTime.milliseconds
      : null;
    return visibleQueuedRuns.filter(run => {
      if (endpointKindFilter !== 'all' && run.endpoint_kind !== endpointKindFilter) return false;
      if (!matchesModeFamily(run.modes, modeFamilyFilter)) return false;
      if (nameQuery && !(run.config_name || '').toLowerCase().includes(nameQuery)) return false;
      if (cutoff !== null && new Date(run.created_at).getTime() < cutoff) return false;
      return true;
    });
  }, [visibleQueuedRuns, endpointKindFilter, modeFamilyFilter, nameQuery, timeFilter, now]);

  // Precompute formatted dates after the purpose tab is applied.
  const runsWithDates = useMemo(() => {
    const source = testKindFilter !== 'all'
      ? secondaryFilteredRuns.filter(r => r.test_kind === testKindFilter)
      : secondaryFilteredRuns;
    return source.map(r => ({
      ...r,
      _createdAgo: timeAgo(r.created_at),
      _createdIso: new Date(r.created_at).toISOString(),
    }));
  }, [secondaryFilteredRuns, testKindFilter]);

  // Cluster comparison-group cells into one expandable row each (#803) —
  // EXCEPT when the list is already scoped to one group (?comparison_group=):
  // that is the "view as list" surface and stays flat.
  type RunRow = (typeof runsWithDates)[number];
  const displayRows = useMemo<RunListRow<RunRow>[]>(() => {
    if (comparisonGroupId) return runsWithDates.map((run) => ({ kind: 'run' as const, run }));
    return clusterComparisonGroups(runsWithDates);
  }, [runsWithDates, comparisonGroupId]);

  // Pagination counts a collapsed group as ONE row unit; expanding a group
  // reveals its cells in place without re-paginating.
  const totalPages = Math.max(1, Math.ceil(displayRows.length / PAGE_SIZE));
  const safePage = Math.min(page, totalPages - 1);
  const pageStart = safePage * PAGE_SIZE;
  const pageEnd = Math.min(pageStart + PAGE_SIZE, displayRows.length);
  const pageUnits = useMemo(() => displayRows.slice(pageStart, pageEnd), [displayRows, pageStart, pageEnd]);

  // Authoritative cell counts + group names for the visible group rows. A 404
  // (deleted group — cells survive via SET NULL) degrades to in-window counts.
  const pageGroupIds = useMemo(
    () => pageUnits.filter((row) => row.kind === 'group').map((row) => row.groupId),
    [pageUnits],
  );
  const groupQueries = useComparisonGroupsQueries(pageGroupIds);
  const groupDetails = new Map(pageGroupIds.map((id, i) => [id, groupQueries[i]]));

  const pageRows = useMemo(
    () =>
      pageUnits.flatMap((row) =>
        row.kind === 'group' && expandedGroups.has(row.groupId)
          ? [row, ...row.runs.map((run) => ({ kind: 'run' as const, run, inGroup: true }))]
          : [row],
      ),
    [pageUnits, expandedGroups],
  );

  const toggleGroup = useCallback((groupId: string) => {
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  }, []);

  // Plain functions (not memoized): groupDetails is rebuilt from useQueries
  // results every render anyway, and a page holds at most PAGE_SIZE groups.
  const groupName = (row: { groupId: string; runs: RunRow[] }): string =>
    groupDetails.get(row.groupId)?.data?.name
    ?? groupFallbackName(row.runs)
    ?? `Group ${row.groupId.slice(0, 8)}`;

  const groupCellCount = (groupId: string): number | null => {
    const query = groupDetails.get(groupId);
    // Loading or 404 → null: the row shows the in-window count with a "+".
    return query?.data ? query.data.cells.length : null;
  };

  const clearComparisonGroup = useCallback(() => {
    updateSearchParams(next => {
      next.delete('comparison_group');
    });
  }, [updateSearchParams]);

  const activeFilterCount = [
    statusFilter !== 'all',
    testKindFilter !== 'all',
    endpointKindFilter !== 'all',
    modeFamilyFilter !== 'all',
    timeFilter !== 'all',
    artifactFilter !== 'all',
    !!nameQuery,
    !!comparisonGroupId,
  ].filter(Boolean).length;

  if (runsQuery.isPending && runs.length === 0) {
    return (
      <PageShell title="Runs">
        <div className="table-container">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-400 text-xs bg-[var(--bg-surface)]">
                <th className="px-3 py-2 text-left font-medium">Run</th>
                <th className="px-3 py-2 text-left font-medium">Name</th>
                <th className="px-3 py-2 text-left font-medium">Purpose</th>
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
      </PageShell>
    );
  }

  if (runsQuery.isError && runs.length === 0) {
    return (
      <PageShell title="Runs">
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load runs</h3>
          <p className="text-red-300 text-sm">Could not fetch test runs. Check your connection and try refreshing.</p>
        </div>
      </PageShell>
    );
  }

  const purposeTabs: Array<{ key: TestKind | 'all'; label: string }> = [
    { key: 'all', label: 'All' },
    { key: 'network', label: 'Network tests' },
    { key: 'url_probe', label: 'URL probes' },
    { key: 'sdk_probe', label: 'SDK probes' },
    { key: 'benchmark', label: 'Benchmarks' },
  ];

  return (
    <PageShell
      title="Runs"
      action={
        <Link
          to={`/projects/${projectId}/tests/new`}
          className={buttonClassName({ variant: 'primary', className: 'flex-shrink-0' })}
        >
          New Run
        </Link>
      }
    >

      {/* Product-purpose tabs are the primary way to segment run history. */}
      <div className="flex items-center gap-1 mb-4 border-b border-gray-800/50 overflow-x-auto" aria-label="Run purpose">
        {purposeTabs.map(tab => {
          const active = testKindFilter === tab.key;
          return (
            <button
              key={tab.key}
              type="button"
              onClick={() => setFilter('test_kind', tab.key)}
              className={`px-3 py-2 text-xs font-medium border-b-2 transition-colors whitespace-nowrap ${
                active
                  ? 'border-cyan-500 text-gray-100'
                  : 'border-transparent text-gray-400 hover:text-gray-300'
              }`}
            >
              {tab.label}
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
            {timeFilter !== 'all' && (
              <FilterChip label="Time" value={TIME_OPTIONS.find(option => option.value === timeFilter)?.label ?? timeFilter} onClear={() => setFilter('time', 'all')} />
            )}
            {endpointKindFilter !== 'all' && (
              <FilterChip label="Target" value={endpointKindFilter} onClear={() => setFilter('endpoint_kind', 'all')} />
            )}
            {modeFamilyFilter !== 'all' && (
              <FilterChip label="Mode" value={MODE_FAMILY_OPTIONS.find(option => option.value === modeFamilyFilter)?.label ?? modeFamilyFilter} onClear={() => setFilter('mode_family', 'all')} />
            )}
            {artifactFilter !== 'all' && (
              <FilterChip label="Artifact" value={artifactFilter === 'yes' ? 'Present' : 'None'} onClear={() => setFilter('has_artifact', 'all')} />
            )}
            {nameQuery && (
              <FilterChip label="Search" value={routeNameQuery.trim()} onClear={() => setFilter('q', '')} />
            )}
            {comparisonGroupId && (
              <>
                <FilterChip label="Group" value={comparisonGroupId.slice(0, 8)} onClear={clearComparisonGroup} />
                {/* Cross-cell pivots for the filtered group (#794). */}
                <Link
                  to={`/projects/${projectId}/benchmarks/compare/${comparisonGroupId}`}
                  className="text-xs text-cyan-400 hover:text-cyan-300"
                >
                  Compare &rarr;
                </Link>
              </>
            )}
          </>
        }
      >
        <RunNameSearch value={routeNameQuery} onCommit={commitNameQuery} />

        <Select
          value={timeFilter}
          onChange={(e) => setFilter('time', e.target.value)}
          aria-label="Filter by time range"
          className="w-full sm:!w-auto py-1.5"
        >
          {TIME_OPTIONS.map(option => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </Select>

        <Select
          value={statusFilter}
          onChange={(e) => setFilter('status', e.target.value)}
          aria-label="Filter by status"
          className="w-full sm:!w-auto py-1.5"
        >
          {STATUS_OPTIONS.map(s => (
            <option key={s} value={s}>
              {s === 'all' ? 'Any status' : s.charAt(0).toUpperCase() + s.slice(1)}
            </option>
          ))}
        </Select>

        <Select
          value={endpointKindFilter}
          onChange={(e) => setFilter('endpoint_kind', e.target.value)}
          aria-label="Filter by target type"
          className="w-full sm:!w-auto py-1.5"
        >
          <option value="all">Any target</option>
          <option value="network">Direct URL</option>
          <option value="proxy">Proxy</option>
          <option value="runtime">Runtime</option>
          <option value="pending">Provisioned target</option>
        </Select>

        <Select
          value={modeFamilyFilter}
          onChange={(e) => setFilter('mode_family', e.target.value)}
          aria-label="Filter by mode family"
          className="w-full sm:!w-auto py-1.5"
        >
          {MODE_FAMILY_OPTIONS.map(option => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </Select>

        <Select
          value={artifactFilter}
          onChange={(e) => setFilter('has_artifact', e.target.value)}
          aria-label="Filter by artifact"
          className="w-full sm:!w-auto py-1.5"
        >
          {ARTIFACT_OPTIONS.map(a => (
            <option key={a} value={a}>
              {a === 'all' ? 'Any artifact' : a === 'yes' ? 'Has artifact' : 'No artifact'}
            </option>
          ))}
        </Select>

        <label className="flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={showQueued}
            onChange={toggleShowQueued}
            className="rounded border-gray-600 bg-transparent text-cyan-500 focus:ring-cyan-500 focus:ring-offset-0 w-3.5 h-3.5"
          />
          Include queued
        </label>
      </FilterBar>

      {runsQuery.isError && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 mt-4 text-yellow-400 text-sm">
          Failed to refresh runs. Retrying automatically.
        </div>
      )}

      {/* One table for every viewport (DataTable handles responsive
          column drops + horizontal scroll — the old md:hidden card list
          duplicated every row and drifted from the table). */}
      <DataTable
        className="mt-4"
        columns={[
          {
            key: 'run',
            label: 'Run',
            render: (row) =>
              row.kind === 'group' ? (
                <button
                  type="button"
                  onClick={() => toggleGroup(row.groupId)}
                  aria-expanded={expandedGroups.has(row.groupId)}
                  aria-label={`${expandedGroups.has(row.groupId) ? 'Collapse' : 'Expand'} group ${groupName(row)}`}
                  className="flex items-center gap-1.5 text-cyan-400 hover:text-cyan-300"
                >
                  <span
                    aria-hidden="true"
                    className="text-gray-400 text-xs transition-transform"
                    style={{ transform: expandedGroups.has(row.groupId) ? 'rotate(90deg)' : '' }}
                  >
                    {'▶'}
                  </span>
                  {row.groupId.slice(0, 8)}
                </button>
              ) : (
                <>
                  {row.inGroup && <span className="text-faint mr-1.5" aria-hidden="true">{'└'}</span>}
                  <Link
                    to={`/projects/${projectId}/runs/${row.run.id}`}
                    className="text-cyan-400 hover:underline"
                  >
                    {row.run.id.slice(0, 8)}
                  </Link>
                </>
              ),
          },
          {
            // Name gets the width — it's the most scannable column; Modes
            // truncates with a tooltip instead of wrapping to 3 lines.
            key: 'name',
            label: 'Name',
            cellClass: 'text-gray-300 truncate max-w-72',
            titleOf: (row) => (row.kind === 'group' ? groupName(row) : row.run.config_name || undefined),
            render: (row) =>
              row.kind === 'group' ? (
                <span className="flex flex-col gap-0.5 min-w-0">
                  <Link
                    to={`/projects/${projectId}/benchmarks/compare/${row.groupId}`}
                    className="text-gray-100 font-medium hover:text-cyan-300 truncate"
                  >
                    {groupName(row)}
                  </Link>
                  <Link
                    to={`/projects/${projectId}/runs?comparison_group=${row.groupId}`}
                    // py-1/-my-1 keeps the row compact while the hit target
                    // meets WCAG 2.5.8's 24px minimum (axe target-size).
                    className="text-faint text-xs hover:text-cyan-400 w-fit inline-block py-1.5 -my-1.5"
                  >
                    view as list
                  </Link>
                </span>
              ) : (
                row.run.config_name || row.run.test_config_id.slice(0, 8)
              ),
          },
          {
            key: 'purpose',
            label: 'Purpose',
            render: (row) =>
              row.kind === 'group' ? (
                <span className="text-xs font-medium px-1.5 py-0.5 rounded text-gray-300 bg-gray-500/10">
                  Comparison
                </span>
              ) : (
                <PurposeBadge kind={row.run.test_kind} />
              ),
          },
          {
            key: 'target',
            label: 'Target',
            hideBelow: 'lg',
            render: (row) =>
              row.kind === 'group' ? <span className="text-faint">-</span> : <TargetBadge kind={row.run.endpoint_kind} />,
          },
          {
            key: 'status',
            label: 'Status',
            render: (row) => (
              <StatusBadge
                status={row.kind === 'group' ? groupAggregateStatus(row.runs) : runDisplayStatus(row.run)}
              />
            ),
          },
          {
            key: 'result',
            label: 'Result',
            render: (row) => {
              if (row.kind !== 'group') {
                return <RunResult ok={row.run.success_count} fail={row.run.failure_count} />;
              }
              const progress = groupProgress(row.runs, groupCellCount(row.groupId));
              const fastest = fastestCachedCell(row.runs);
              return (
                <span className="flex flex-col gap-0.5">
                  <span className="tabular-nums text-gray-300 whitespace-nowrap">
                    {progress.terminal}/{progress.total}
                    {progress.approximate ? '+' : ''}
                    {progress.failed > 0 && (
                      <span className="text-red-400"> &middot; {progress.failed} failed</span>
                    )}
                  </span>
                  {fastest && (
                    <span className="text-xs text-cyan-300 whitespace-nowrap">
                      fastest: {fastest.label} &middot; {formatMs(fastest.p50)}
                    </span>
                  )}
                </span>
              );
            },
          },
          {
            key: 'modes',
            label: 'Modes',
            hideBelow: 'lg',
            cellClass: 'text-gray-400 truncate max-w-40',
            titleOf: (row) => (row.kind === 'group' ? undefined : row.run.modes?.join(', ') || undefined),
            render: (row) =>
              row.kind === 'group'
                ? `${row.runs.length} cell${row.runs.length !== 1 ? 's' : ''} in window`
                : row.run.modes?.join(', ') || '-',
          },
          {
            key: 'created',
            label: 'Created',
            cellClass: 'text-gray-400',
            titleOf: (row) => (row.kind === 'group' ? row.runs[0]?._createdIso : row.run._createdIso),
            render: (row) => (row.kind === 'group' ? row.runs[0]?._createdAgo : row.run._createdAgo),
          },
        ]}
        rows={pageRows}
        rowKey={(row) => (row.kind === 'group' ? `group-${row.groupId}` : row.run.id)}
        rowClass={(row) => (row.kind === 'run' && row.inGroup ? 'bg-gray-800/10' : undefined)}
        empty={
          <>
            <p className="text-gray-400 text-sm">{activeFilterCount > 0 ? 'No runs match the current filters' : 'No runs yet'}</p>
            {activeFilterCount === 0 && (
              <Link to={`/projects/${projectId}/tests/new`} className="text-cyan-400 text-xs mt-1 inline-block">
                Start a network test
              </Link>
            )}
          </>
        }
      />

      <StatusFooter
        paused={paused}
        onPauseToggle={() => setPaused(p => !p)}
        onRefresh={() => { void runsQuery.refetch(); }}
        lastUpdatedAt={runsQuery.dataUpdatedAt || null}
        intervalMs={15000}
        pills={
          <span className="text-faint">
            {runsWithDates.length} run{runsWithDates.length !== 1 ? 's' : ''}
            {activeFilterCount > 0 && ` · ${activeFilterCount} filter${activeFilterCount !== 1 ? 's' : ''}`}
          </span>
        }
      />

      {/* Pagination footer — a collapsed comparison group counts as one row. */}
      {displayRows.length > 0 && (
        <div className="flex items-center justify-between mt-4 text-xs text-gray-400">
          <span>
            Showing {pageStart + 1}-{pageEnd} of {displayRows.length} rows
          </span>
          <div className="flex items-center gap-2">
            <Button
              onClick={() => setPage(p => Math.max(0, p - 1))}
              disabled={safePage === 0}
              size="xs"
            >
              Previous
            </Button>
            <span className="tabular-nums text-gray-400">
              {safePage + 1} / {totalPages}
            </span>
            <Button
              onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))}
              disabled={safePage >= totalPages - 1}
              size="xs"
            >
              Next
            </Button>
          </div>
        </div>
      )}
    </PageShell>
  );
}
