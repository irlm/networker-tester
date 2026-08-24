import { useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { useSearchParams } from 'react-router';
import { KpiTile } from '../components/common/KpiTile';
import { rampTextClass } from '../lib/severity';
import { errorMessage } from '../api/http';
import { DataTable } from '../components/common/DataTable';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { StatusFooter } from '../components/common/StatusFooter';
import { Button } from '../components/common/Button';
import { ErrorState, LoadingState } from '../components/common/AsyncState';
import { Input, Select } from '../components/common/FormControls';
import { PageShell } from '../components/common/PageShell';
import { usePerfLogsQuery, usePerfLogStatsQuery } from '../features/perf-logs/queries';
import { usePageTitle } from '../hooks/usePageTitle';
import { formatMsCompact as formatMs } from '../lib/format';

type Tab = 'logs' | 'stats';
type Kind = 'all' | 'api' | 'render';
type TimeRange = '5m' | '15m' | '30m' | '1h' | '6h' | '24h' | '7d';

const TIME_RANGES: ReadonlyArray<{ value: TimeRange; label: string; windowMs: number }> = [
  { value: '5m', label: 'Last 5 minutes', windowMs: 5 * 60_000 },
  { value: '15m', label: 'Last 15 minutes', windowMs: 15 * 60_000 },
  { value: '30m', label: 'Last 30 minutes', windowMs: 30 * 60_000 },
  { value: '1h', label: 'Last hour', windowMs: 60 * 60_000 },
  { value: '6h', label: 'Last 6 hours', windowMs: 6 * 60 * 60_000 },
  { value: '24h', label: 'Last 24 hours', windowMs: 24 * 60 * 60_000 },
  { value: '7d', label: 'Last 7 days', windowMs: 7 * 24 * 60 * 60_000 },
];

function isTimeRange(value: string | null): value is TimeRange {
  return TIME_RANGES.some(range => range.value === value);
}

// Latency renders on the shared cyan magnitude ramp (severity-v2) — red
// only at breach. Thresholds unchanged from the old local ramp.
function speedColor(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return 'text-gray-400';
  return rampTextClass(ms, { mid: 50, high: 200, breach: 500 });
}

function renderSpeedColor(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return 'text-gray-400';
  return rampTextClass(ms, { mid: 16, high: 50, breach: 100 });
}

function kindBadgeClass(kind: string): string {
  if (kind === 'api') {
    return 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5';
  }
  return 'border-green-500/30 text-green-400 bg-green-500/5';
}

const PAGE_SIZE = 50;
const TABS: Tab[] = ['logs', 'stats'];

export function PerfLogPage() {
  const [tab, setTab] = useState<Tab>('logs');
  const [searchParams, setSearchParams] = useSearchParams();
  const [kindFilter, setKindFilter] = useState<Kind>('all');
  const [pathFilter, setPathFilter] = useState('');
  const [page, setPage] = useState(0);
  const [paused, setPaused] = useState(false);
  const tabRefs = useRef<Record<Tab, HTMLButtonElement | null>>({ logs: null, stats: null });
  const rangeParam = searchParams.get('range');
  const timeRange: TimeRange = isTimeRange(rangeParam) ? rangeParam : '5m';
  const selectedRange = TIME_RANGES.find(range => range.value === timeRange) ?? TIME_RANGES[0];

  const queryParams = useMemo(() => ({
    kind: kindFilter === 'all' ? undefined : kindFilter,
    path: pathFilter.trim() || undefined,
    windowMs: selectedRange.windowMs,
    limit: 200,
  }), [kindFilter, pathFilter, selectedRange.windowMs]);
  const logsQuery = usePerfLogsQuery(queryParams, tab === 'logs' && !paused);
  const statsQuery = usePerfLogStatsQuery(selectedRange.windowMs, tab === 'stats' && !paused);
  const logs = useMemo(() => logsQuery.data ?? [], [logsQuery.data]);
  const stats = statsQuery.data;

  usePageTitle('Performance Log');

  const setTimeRange = (nextRange: TimeRange) => {
    setSearchParams(previous => {
      const next = new URLSearchParams(previous);
      if (nextRange === '5m') next.delete('range');
      else next.set('range', nextRange);
      return next;
    }, { replace: true });
    setPage(0);
  };

  const activeLogFilterCount = [timeRange !== '5m', kindFilter !== 'all', pathFilter].filter(Boolean).length;
  const activeFilterCount = tab === 'logs' ? activeLogFilterCount : Number(timeRange !== '5m');
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
    // Sorted by TOTAL time contributed, not by average.
    //
    // Average ranks the one slow call above the endpoint polled three hundred
    // times, and the second is almost always what there is to fix: a 42 ms
    // /version polled every 5 s costs far more wall clock than a 300 ms report
    // nobody opens. totalMs was already being accumulated here and simply was
    // not shown or sorted on.
    return [...byPath.entries()]
      .map(([path, data]) => ({ path, ...data, avgMs: data.totalMs / data.count }))
      .sort((a, b) => b.totalMs - a.totalMs)
      .slice(0, 10);
  }, [logs]);

  const activeTabClass = 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5';
  const inactiveTabClass = 'border-gray-700 text-gray-400 hover:border-gray-600 hover:text-gray-300';
  const tabClass = 'px-3 py-1.5 text-xs rounded border transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-400/70';
  const handleTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, current: Tab) => {
    const currentIndex = TABS.indexOf(current);
    let next: Tab | undefined;
    if (event.key === 'ArrowRight') next = TABS[(currentIndex + 1) % TABS.length];
    if (event.key === 'ArrowLeft') next = TABS[(currentIndex - 1 + TABS.length) % TABS.length];
    if (event.key === 'Home') next = TABS[0];
    if (event.key === 'End') next = TABS[TABS.length - 1];
    if (!next) return;
    event.preventDefault();
    setTab(next);
    tabRefs.current[next]?.focus();
  };
  const tabActions = (
    <div className="flex items-center gap-2" role="tablist" aria-label="Performance log views">
      <button
        type="button"
        id="performance-log-tab"
        role="tab"
        aria-selected={tab === 'logs'}
        aria-controls="performance-log-panel"
        tabIndex={tab === 'logs' ? 0 : -1}
        ref={node => { tabRefs.current.logs = node; }}
        onClick={() => setTab('logs')}
        onKeyDown={event => handleTabKeyDown(event, 'logs')}
        className={`${tabClass} ${tab === 'logs' ? activeTabClass : inactiveTabClass}`}
      >
        Logs ({logs.length})
      </button>
      <button
        type="button"
        id="performance-stats-tab"
        role="tab"
        aria-selected={tab === 'stats'}
        aria-controls="performance-stats-panel"
        tabIndex={tab === 'stats' ? 0 : -1}
        ref={node => { tabRefs.current.stats = node; }}
        onClick={() => setTab('stats')}
        onKeyDown={event => handleTabKeyDown(event, 'stats')}
        className={`${tabClass} ${tab === 'stats' ? activeTabClass : inactiveTabClass}`}
      >
        Stats
      </button>
    </div>
  );

  const clearFilters = () => {
    setTimeRange('5m');
    if (tab === 'logs') {
      setKindFilter('all');
      setPathFilter('');
    }
  };

  return (
    <PageShell
      title="Performance Log"
      subtitle="Inspect recent API and render timings without scanning the full retention window."
      action={tabActions}
    >
      <FilterBar
        activeCount={activeFilterCount}
        onClearAll={clearFilters}
        chips={
          <>
            {timeRange !== '5m' && (
              <FilterChip label="Time" value={selectedRange.label} onClear={() => setTimeRange('5m')} />
            )}
            {tab === 'logs' && kindFilter !== 'all' && (
              <FilterChip label="Kind" value={kindFilter} onClear={() => { setKindFilter('all'); setPage(0); }} />
            )}
            {tab === 'logs' && pathFilter && (
              <FilterChip label="Path" value={pathFilter} onClear={() => { setPathFilter(''); setPage(0); }} />
            )}
          </>
        }
      >
        <Select
          aria-label="Time range"
          value={timeRange}
          onChange={event => setTimeRange(event.target.value as TimeRange)}
          className="w-full sm:w-auto"
        >
          {TIME_RANGES.map(range => (
            <option key={range.value} value={range.value}>{range.label}</option>
          ))}
        </Select>
        {tab === 'logs' && (
          <>
            <Select
              aria-label="Log kind"
              value={kindFilter}
              onChange={event => { setKindFilter(event.target.value as Kind); setPage(0); }}
              className="w-full sm:w-auto"
            >
              <option value="all">All types</option>
              <option value="api">API only</option>
              <option value="render">Render only</option>
            </Select>
            <Input
              type="search"
              aria-label="Filter logs by path"
              value={pathFilter}
              onChange={event => { setPathFilter(event.target.value); setPage(0); }}
              placeholder="Filter by path…"
              className="w-full sm:w-56"
            />
          </>
        )}
      </FilterBar>

      {tab === 'logs' && (
        <div id="performance-log-panel" role="tabpanel" aria-labelledby="performance-log-tab" className="mt-4">
          {logsQuery.isPending && logs.length === 0 && (
            <LoadingState label="Loading performance data…" />
          )}
          {logsQuery.isError && logs.length === 0 && (
            <ErrorState
              title="Failed to load performance logs"
              message={errorMessage(logsQuery.error)}
              onRetry={() => { void logsQuery.refetch(); }}
            />
          )}
          {logsQuery.isError && logs.length > 0 && (
            <div className="mb-4">
              <ErrorState
                title="Could not refresh performance logs"
                message={`${errorMessage(logsQuery.error)} Showing the last successful response.`}
                onRetry={() => { void logsQuery.refetch(); }}
              />
            </div>
          )}

          <DataTable
            columns={[
              {
                key: 'time',
                label: 'Time',
                cellClass: 'text-gray-400',
                render: log => new Date(log.logged_at).toLocaleString(),
              },
              {
                key: 'kind',
                label: 'Kind',
                render: log => (
                  <span className={`text-xs uppercase px-1.5 py-0.5 rounded border ${kindBadgeClass(log.kind)}`}>
                    {log.kind}
                  </span>
                ),
              },
              {
                key: 'path',
                label: 'Path / Component',
                cellClass: 'text-gray-300 truncate max-w-[250px]',
                titleOf: log => log.path || log.component || '',
                render: log => {
                  // Shorten UUID paths for readability
                  const displayPath = log.path?.replace(
                    /\/projects\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
                    '/projects/…'
                  );
                  return log.kind === 'api' ? (
                    <><span className="text-gray-400">{log.method} </span>{displayPath}</>
                  ) : (
                    <><span className="text-gray-400">{log.component}</span> <span className="text-faint">{log.trigger}</span></>
                  );
                },
              },
              {
                key: 'status',
                label: 'Status',
                render: log => (
                  log.kind === 'api' ? (
                    <span className={log.status && log.status < 300 ? 'text-green-400' : log.status && log.status < 400 ? 'text-yellow-400' : 'text-red-400'}>
                      {log.status || '-'}
                    </span>
                  ) : (
                    <span className="text-faint">{log.item_count ?? '-'}</span>
                  )
                ),
              },
              {
                key: 'total',
                label: 'Total',
                align: 'right',
                render: log => (
                  <span className={log.kind === 'api' ? speedColor(log.total_ms) : renderSpeedColor(log.render_ms)}>
                    {log.kind === 'api' ? formatMs(log.total_ms) : formatMs(log.render_ms)}
                  </span>
                ),
              },
              {
                key: 'server',
                label: 'Server',
                align: 'right',
                cellClass: 'text-cyan-400',
                render: log => (log.kind === 'api' ? formatMs(log.server_ms) : '-'),
              },
              {
                key: 'netrender',
                label: 'Net/Render',
                align: 'right',
                cellClass: 'text-purple-400',
                render: log => (log.kind === 'api' ? formatMs(log.network_ms) : formatMs(log.render_ms)),
              },
              {
                key: 'source',
                label: 'Source',
                hideBelow: 'lg',
                cellClass: 'text-faint',
                render: log => log.source || '-',
              },
            ]}
            rows={pagedLogs}
            rowKey={log => String(log.id)}
            empty={
              <span className="text-gray-400 text-sm">
                No performance logs in {selectedRange.label.toLowerCase()}. Logs are flushed every 30 seconds.
              </span>
            }
          />
          {totalPages > 1 && (
            <div className="flex items-center justify-between px-3 py-2 border-t border-gray-800/50 text-xs text-gray-400">
              <span>{logs.length} rows &middot; page {currentPage + 1} of {totalPages}</span>
              <div className="flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => setPage(0)}
                  disabled={currentPage === 0}
                  aria-label="First page"
                >
                  &laquo;
                </Button>
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => setPage(p => Math.max(0, p - 1))}
                  disabled={currentPage === 0}
                >
                  &lsaquo; Prev
                </Button>
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))}
                  disabled={currentPage >= totalPages - 1}
                >
                  Next &rsaquo;
                </Button>
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => setPage(totalPages - 1)}
                  disabled={currentPage >= totalPages - 1}
                  aria-label="Last page"
                >
                  &raquo;
                </Button>
              </div>
            </div>
          )}

          {/* Mounted only on the logs tab (stats has no poll to control);
              never more than one footer per rendered page — it binds
              document-level r/p keys. */}
          <StatusFooter
            paused={paused}
            onPauseToggle={() => setPaused(p => !p)}
            onRefresh={() => { void logsQuery.refetch(); }}
            lastUpdatedAt={logsQuery.dataUpdatedAt || null}
            intervalMs={15000}
            pills={
              <span className="text-faint">
                {selectedRange.label} · {logs.length} row{logs.length !== 1 ? 's' : ''}
                {activeFilterCount > 0 && ` · ${activeFilterCount} filter${activeFilterCount !== 1 ? 's' : ''}`}
              </span>
            }
          />
        </div>
      )}

      {tab === 'stats' && (
        <div id="performance-stats-panel" role="tabpanel" aria-labelledby="performance-stats-tab" className="mt-4 space-y-6">
          {statsQuery.isPending && !stats && <LoadingState label="Loading performance statistics…" />}
          {statsQuery.isError && !stats && (
            <ErrorState
              title="Failed to load performance statistics"
              message={errorMessage(statsQuery.error)}
              onRetry={() => { void statsQuery.refetch(); }}
            />
          )}
          {statsQuery.isError && stats && (
            <ErrorState
              title="Could not refresh performance statistics"
              message={`${errorMessage(statsQuery.error)} Showing the last successful response.`}
              onRetry={() => { void statsQuery.refetch(); }}
            />
          )}
          {stats && (
            <>
          {/* Summary cards.

              p95 leads and the average is the footnote, not the other way
              round. An average is the one statistic that cannot show a latency
              problem — it is dragged down by the many fast polls and says
              nothing about the tail, which is what anyone complaining is
              actually experiencing. These numbers were already in the stats
              response; only the emphasis changed. */}
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
            <KpiTile
              label="API Requests"
              value={String(stats.api_count)}
              health="info"
              sub={stats.api_count > 0
                ? `${stats.slow_api_count} slow >200ms (${((stats.slow_api_count / stats.api_count) * 100).toFixed(1)}%)`
                : 'no requests in range'}
            />
            <KpiTile
              label="API p95"
              value={formatMs(stats.p95_total_ms)}
              valueClass={speedColor(stats.p95_total_ms)}
              health={stats.slow_api_count > 0 ? 'warn' : 'ok'}
              sub={`avg ${formatMs(stats.avg_total_ms)}`}
            />
            <KpiTile
              label="Renders"
              value={String(stats.render_count)}
              health={stats.janky_render_count > 0 ? 'warn' : 'ok'}
              sub={stats.render_count > 0
                ? `${stats.janky_render_count} janky >16ms (${((stats.janky_render_count / stats.render_count) * 100).toFixed(1)}%)`
                : 'no renders in range'}
            />
            <KpiTile
              label="Render p95"
              value={formatMs(stats.p95_render_ms)}
              valueClass={renderSpeedColor(stats.p95_render_ms)}
              health={stats.janky_render_count > 0 ? 'warn' : 'ok'}
              sub={`avg ${formatMs(stats.avg_render_ms)}`}
            />
          </div>

          {/* Where the average time goes. Server vs network is only meaningful
              as a split, so the share is the headline and the millisecond
              figures support it. */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
            <KpiTile
              label="Avg Server Time"
              value={formatMs(stats.avg_server_ms)}
              health="info"
            />
            <KpiTile
              label="Avg Network Time"
              value={stats.avg_total_ms && stats.avg_server_ms ? formatMs(stats.avg_total_ms - stats.avg_server_ms) : '-'}
              sub="round trips, TLS, queueing — and transfer"
            />
            <KpiTile
              label="Server % of Total"
              value={stats.avg_total_ms && stats.avg_server_ms ? `${((stats.avg_server_ms / stats.avg_total_ms) * 100).toFixed(0)}%` : '-'}
              sub={stats.avg_total_ms && stats.avg_server_ms
                ? `${(100 - (stats.avg_server_ms / stats.avg_total_ms) * 100).toFixed(0)}% is off-server`
                : undefined}
            />
          </div>

          {/* Top slow paths */}
          {topSlowPaths.length > 0 && (
            <div>
              <h3 className="text-xs text-gray-400 tracking-wider font-medium mb-1 uppercase">Where the time goes</h3>
              <p className="mb-3 text-xs text-faint">
                Ranked by total time contributed, not by average — a cheap endpoint polled often
                outweighs an expensive one called rarely. Based on the latest loaded API rows in
                this time range.
              </p>
              <div className="table-container">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-gray-800/50 text-gray-400 text-xs bg-[var(--bg-surface)]">
                      <th className="px-3 py-2 text-left font-medium">Path</th>
                      <th className="px-3 py-2 text-right font-medium">Calls</th>
                      <th className="px-3 py-2 text-right font-medium">Avg</th>
                      <th className="px-3 py-2 text-right font-medium">Max</th>
                      <th className="px-3 py-2 text-right font-medium">Total</th>
                    </tr>
                  </thead>
                  <tbody>
                    {topSlowPaths.map(p => (
                      <tr key={p.path} className="border-b border-gray-800/30">
                        <td className="px-3 py-2 text-gray-300 text-xs truncate max-w-[300px]" title={p.path}>
                          {p.path.replace(/\/projects\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, '/projects/…')}
                        </td>
                        <td className="px-3 py-2 text-right text-gray-400 text-xs">{p.count}</td>
                        <td className={`px-3 py-2 text-right text-xs ${speedColor(p.avgMs)}`}>{formatMs(p.avgMs)}</td>
                        <td className={`px-3 py-2 text-right text-xs ${speedColor(p.maxMs)}`}>{formatMs(p.maxMs)}</td>
                        <td className="px-3 py-2 text-right text-xs text-gray-200 tabular-nums">
                          {p.totalMs >= 1000 ? `${(p.totalMs / 1000).toFixed(1)}s` : formatMs(p.totalMs)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
            </>
          )}
        </div>
      )}
    </PageShell>
  );
}
