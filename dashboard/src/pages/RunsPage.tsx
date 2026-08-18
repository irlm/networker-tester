import { useState, useCallback, useMemo } from 'react';
import { DataTable } from '../components/common/DataTable';
import { Link, useSearchParams } from 'react-router';
import type { TestRun, RunStatus, EndpointKind, TestConfig, TestConfigListItem } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { RunResult } from '../components/common/RunResult';
import { runDisplayStatus } from '../lib/runStatus';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { StatusFooter } from '../components/common/StatusFooter';
import { usePageTitle } from '../hooks/usePageTitle';
import { useRenderLog } from '../hooks/useRenderLog';
import { timeAgo } from '../lib/format';
import { useProject } from '../hooks/useProject';
import { useTestConfigsQuery, useTestRunsQuery } from '../features/runs/queries';
import type { RunListParams } from '../features/runs/api';
import { PageShell } from '../components/common/PageShell';
import { Button } from '../components/common/Button';
import { buttonClassName } from '../components/common/button-styles';
import { Select } from '../components/common/FormControls';

const STATUS_OPTIONS: Array<RunStatus | 'all'> = ['all', 'queued', 'provisioning', 'running', 'completed', 'failed', 'cancelled'];
const ARTIFACT_OPTIONS = ['all', 'yes', 'no'] as const;

const PAGE_SIZE = 20;

// Kind is a category, not a status — one neutral treatment so green/purple
// stay reserved for success/logo (audit F12: category hues fought the ramp).
const KIND_BADGE_CLASSES: Record<string, string> = {
  network: 'text-cyan-400 bg-cyan-500/10',
  proxy: 'text-gray-300 bg-gray-500/10',
  runtime: 'text-gray-300 bg-gray-500/10',
};

function KindBadge({ kind }: { kind: string | null | undefined }) {
  if (!kind) return <span className="text-faint">-</span>;
  const classes = KIND_BADGE_CLASSES[kind] || 'text-gray-400 bg-gray-500/10';
  return (
    <span className={`text-xs font-medium px-1.5 py-0.5 rounded ${classes}`}>
      {kind}
    </span>
  );
}

export function RunsPage() {
  const { projectId } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [page, setPage] = useState(0);

  const statusFilter = searchParams.get('status') || 'all';
  const endpointKindFilter = searchParams.get('endpoint_kind') || 'all';
  const artifactFilter = searchParams.get('has_artifact') || 'all';
  // Queued runs are real runs the user cares about — show them by default so
  // the list never lies with "No runs yet" while jobs are actually piling up.
  // Opt-out via ?show_queued=0.
  const showQueued = searchParams.get('show_queued') !== '0';
  const comparisonGroupId = searchParams.get('comparison_group');
  // ?q=<substring> — client-side config-name filter. Drives the admin canary
  // panel's "canary runs" link (?q=soak-canary); harmless when absent.
  const nameQuery = (searchParams.get('q') || '').trim().toLowerCase();

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
      // Default is now "show" — store '0' to explicitly hide.
      if (prev.get('show_queued') === '0') {
        next.delete('show_queued');
      } else {
        next.set('show_queued', '0');
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

  const params = useMemo<RunListParams>(() => {
    const next: RunListParams = { limit: 200 };
    if (statusFilter !== 'all') next.status = statusFilter;
    if (endpointKindFilter !== 'all') next.endpoint_kind = endpointKindFilter;
    if (artifactFilter === 'yes') next.has_artifact = true;
    if (artifactFilter === 'no') next.has_artifact = false;
    if (comparisonGroupId) next.comparison_group_id = comparisonGroupId;
    return next;
  }, [statusFilter, endpointKindFilter, artifactFilter, comparisonGroupId]);

  const [paused, setPaused] = useState(false);
  const runsQuery = useTestRunsQuery(projectId, params, { polling: !paused });
  const configsQuery = useTestConfigsQuery(projectId);
  const runs = useMemo<TestRun[]>(() => runsQuery.data ?? [], [runsQuery.data]);
  // Older control planes returned full configs from the list endpoint. Keep
  // the tolerant enrichment while the feature transport normalizes rollout.
  const configs = useMemo<Array<TestConfigListItem | TestConfig>>(
    () => (configsQuery.data ?? []) as Array<TestConfigListItem | TestConfig>,
    [configsQuery.data],
  );

  // Build a config-id → {kind, name, modes} map so runs list can show endpoint_kind
  // even when the backend doesn't denormalize it into the TestRun row.
  const configMap = useMemo(() => {
    const m = new Map<string, { name: string; endpoint_kind?: EndpointKind; modes?: string[] }>();
    for (const c of configs) {
      const endpoint_kind = 'endpoint_kind' in c
        ? c.endpoint_kind
        : (c.endpoint as { kind?: EndpointKind } | undefined)?.kind;
      const modes = 'modes' in c
        ? c.modes
        : (c as TestConfig).workload?.modes;
      m.set(c.id, { name: c.name, endpoint_kind, modes });
    }
    return m;
  }, [configs]);

  // Merge denormalized fields from the config map so the backend's sparse
  // TestRun payload (no endpoint_kind, no config_name) still drives the UI.
  const runsEnriched = useMemo(() => {
    return runs.map((r) => {
      const cfg = configMap.get(r.test_config_id);
      return {
        ...r,
        config_name: r.config_name || cfg?.name,
        endpoint_kind: r.endpoint_kind || cfg?.endpoint_kind,
        modes: r.modes || cfg?.modes,
      };
    });
  }, [runs, configMap]);

  // Filter out queued unless opted in. Scoped to a comparison group, show
  // everything (the user just launched the group and expects to see its runs).
  const filteredRuns = useMemo(() => {
    if (showQueued || statusFilter === 'queued' || comparisonGroupId) return runsEnriched;
    return runsEnriched.filter(r => r.status !== 'queued');
  }, [runsEnriched, showQueued, statusFilter, comparisonGroupId]);

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
    let source = endpointKindFilter !== 'all'
      ? filteredRuns.filter(r => r.endpoint_kind === endpointKindFilter)
      : filteredRuns;
    if (nameQuery) {
      source = source.filter(r => (r.config_name || '').toLowerCase().includes(nameQuery));
    }
    return source.map(r => ({
      ...r,
      _createdAgo: timeAgo(r.created_at),
      _createdIso: new Date(r.created_at).toISOString(),
    }));
  }, [filteredRuns, endpointKindFilter, nameQuery]);

  // Pagination
  const totalPages = Math.max(1, Math.ceil(runsWithDates.length / PAGE_SIZE));
  const safePage = Math.min(page, totalPages - 1);
  const pageStart = safePage * PAGE_SIZE;
  const pageEnd = Math.min(pageStart + PAGE_SIZE, runsWithDates.length);
  const pageRuns = runsWithDates.slice(pageStart, pageEnd);

  const clearComparisonGroup = useCallback(() => {
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      next.delete('comparison_group');
      return next;
    }, { replace: true });
    setPage(0);
  }, [setSearchParams]);

  const activeFilterCount = [
    statusFilter !== 'all',
    endpointKindFilter !== 'all',
    artifactFilter !== 'all',
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

  const kindTabs: Array<{ key: EndpointKind | 'all'; label: string }> = [
    { key: 'all', label: 'All' },
    { key: 'network', label: 'Network' },
    { key: 'proxy', label: 'Proxy' },
    { key: 'runtime', label: 'Runtime' },
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
                  : `border-transparent hover:text-gray-300 ${count === 0 ? 'text-faint' : 'text-gray-400'}`
              }`}
            >
              {tab.label}
              <span className={`ml-1.5 tabular-nums ${active ? 'text-cyan-400' : 'text-faint'}`}>
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
            {comparisonGroupId && (
              <FilterChip label="Group" value={comparisonGroupId.slice(0, 8)} onClear={clearComparisonGroup} />
            )}
          </>
        }
      >
        <Select
          value={statusFilter}
          onChange={(e) => setFilter('status', e.target.value)}
          aria-label="Filter by status"
          className="w-auto py-1.5"
        >
          {STATUS_OPTIONS.map(s => (
            <option key={s} value={s}>
              {s === 'all' ? 'Any status' : s.charAt(0).toUpperCase() + s.slice(1)}
            </option>
          ))}
        </Select>

        <Select
          value={artifactFilter}
          onChange={(e) => setFilter('has_artifact', e.target.value)}
          aria-label="Filter by benchmark artifact"
          className="w-auto py-1.5"
        >
          {ARTIFACT_OPTIONS.map(a => (
            <option key={a} value={a}>
              {a === 'all' ? 'Any type' : a === 'yes' ? 'Benchmarks only' : 'Simple only'}
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
            render: (run) => (
              <>
                <Link
                  to={`/projects/${projectId}/runs/${run.id}`}
                  className="text-cyan-400 hover:underline"
                >
                  {run.id.slice(0, 8)}
                </Link>
                {run.artifact_id && (
                  <span className="ml-2 text-xs text-gray-300 bg-gray-500/10 px-1.5 py-0.5 rounded">benchmark</span>
                )}
              </>
            ),
          },
          {
            // Name gets the width — it's the most scannable column; Modes
            // truncates with a tooltip instead of wrapping to 3 lines.
            key: 'name',
            label: 'Name',
            cellClass: 'text-gray-300 truncate max-w-72',
            titleOf: (run) => run.config_name || undefined,
            render: (run) => run.config_name || run.test_config_id.slice(0, 8),
          },
          {
            key: 'type',
            label: 'Type',
            hideBelow: 'lg',
            render: (run) => <KindBadge kind={run.endpoint_kind} />,
          },
          {
            key: 'status',
            label: 'Status',
            render: (run) => <StatusBadge status={runDisplayStatus(run)} />,
          },
          {
            key: 'result',
            label: 'Result',
            render: (run) => <RunResult ok={run.success_count} fail={run.failure_count} />,
          },
          {
            key: 'modes',
            label: 'Modes',
            hideBelow: 'lg',
            cellClass: 'text-gray-400 truncate max-w-40',
            titleOf: (run) => run.modes?.join(', ') || undefined,
            render: (run) => run.modes?.join(', ') || '-',
          },
          {
            key: 'created',
            label: 'Created',
            cellClass: 'text-gray-400',
            titleOf: (run) => run._createdIso,
            render: (run) => run._createdAgo,
          },
        ]}
        rows={pageRuns}
        rowKey={(run) => run.id}
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

      {/* Pagination footer */}
      {runsWithDates.length > 0 && (
        <div className="flex items-center justify-between mt-4 text-xs text-gray-400">
          <span>
            Showing {pageStart + 1}-{pageEnd} of {runsWithDates.length} runs
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
