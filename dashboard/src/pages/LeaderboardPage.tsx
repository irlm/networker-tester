import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import type { BenchmarkLeaderboardEntry, BenchmarkRun, GroupedLeaderboard } from '../api/types';
import { HorizontalBoxWhiskerChart } from '../components/charts/HorizontalBoxWhiskerChart';
import type { HBoxGroup } from '../components/charts/HorizontalBoxWhiskerChart';
import { usePageTitle } from '../hooks/usePageTitle';
import { languageColor } from '../lib/languageColors';

type Tab = 'grouped' | 'leaderboard' | 'comparison' | 'timeline';

function formatMs(val: number | undefined): string {
  if (val === undefined || val === null) return '--';
  if (val < 1) return `${(val * 1000).toFixed(0)} us`;
  if (val < 1000) return `${val.toFixed(2)} ms`;
  return `${(val / 1000).toFixed(2)} s`;
}

function getRank(index: number): string {
  if (index === 0) return '1st';
  if (index === 1) return '2nd';
  if (index === 2) return '3rd';
  return `${index + 1}th`;
}

function rankColor(index: number): string {
  if (index === 0) return 'text-yellow-400';
  if (index === 1) return 'text-gray-300';
  if (index === 2) return 'text-orange-400';
  return 'text-gray-500';
}

/** Format a group slug like "azure-eastus-loopback" → "Azure / eastus / loopback" */
function formatGroupLabel(group: string): string {
  const parts = group.split('-');
  if (parts.length < 2) return group;
  const [provider, ...rest] = parts;
  return [provider.charAt(0).toUpperCase() + provider.slice(1), ...rest].join(' / ');
}

function GroupedTab() {
  const [data, setData] = useState<GroupedLeaderboard | null>(null);
  const [selectedGroup, setSelectedGroup] = useState<string>('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchGrouped = useCallback(async (group?: string) => {
    setLoading(true);
    setError(null);
    try {
      const result = await api.getGroupedLeaderboard(group || undefined);
      setData(result);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : 'Failed to load grouped leaderboard';
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void fetchGrouped();
  }, [fetchGrouped]);

  const handleGroupChange = useCallback((group: string) => {
    setSelectedGroup(group);
    void fetchGrouped(group || undefined);
  }, [fetchGrouped]);

  const hboxGroups: HBoxGroup[] = (data?.languages ?? []).map((lang) => {
    const limited = lang.run_count < 3;
    return {
      label: lang.language,
      sublabel: limited ? `${lang.run_count} runs (limited data)` : `${lang.run_count} runs`,
      color: languageColor(lang.language),
      p5: lang.p5,
      p25: lang.p25,
      p50: lang.p50,
      p75: lang.p75,
      p95: lang.p95,
      mean: lang.mean,
    };
  });

  // Sort table by p50 ascending
  const sortedForTable = [...(data?.languages ?? [])].sort((a, b) => a.p50 - b.p50);

  const showAllWarning = !selectedGroup;
  const groups = data?.groups ?? [];

  if (loading) {
    return (
      <div className="text-center text-gray-500 py-16 motion-safe:animate-pulse">
        Loading benchmark data...
      </div>
    );
  }

  if (error) {
    return (
      <div className="text-center text-red-400 py-16">
        {error}
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {/* Group selector */}
      <div className="flex items-center gap-3">
        <span className="text-xs text-gray-500 uppercase tracking-wider">Group</span>
        <select
          value={selectedGroup}
          onChange={e => handleGroupChange(e.target.value)}
          className="bg-[#111827] border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-200 focus:outline-none focus:border-cyan-600"
        >
          <option value="">All</option>
          {groups.map(g => (
            <option key={g} value={g}>{formatGroupLabel(g)}</option>
          ))}
        </select>
      </div>

      {/* All-groups warning */}
      {showAllWarning && (
        <div className="border-l-4 border-amber-500 bg-[#1a1a2e] text-gray-400 text-sm p-3 rounded">
          Mixed network conditions — results are not directly comparable. Use for general trends only.
        </div>
      )}

      {/* Chart */}
      {hboxGroups.length === 0 ? (
        <div className="text-center text-gray-500 py-16">
          No benchmark data yet
        </div>
      ) : (
        <div className="bg-[#0d1117] border border-gray-800 rounded p-4">
          <HorizontalBoxWhiskerChart
            groups={hboxGroups}
            unit="ms"
            title="Latency distribution by language"
          />
        </div>
      )}

      {/* Data table */}
      {sortedForTable.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800 text-right text-gray-500 text-xs uppercase tracking-wider">
                <th className="py-2 px-3 text-left">Language</th>
                <th className="py-2 px-3">Mean</th>
                <th className="py-2 px-3">p50</th>
                <th className="py-2 px-3">p95</th>
                <th className="py-2 px-3">p99</th>
                <th className="py-2 px-3">RPS</th>
                <th className="py-2 px-3">Runs</th>
              </tr>
            </thead>
            <tbody>
              {sortedForTable.map((lang, i) => {
                const color = languageColor(lang.language);
                const limited = lang.run_count < 3;
                return (
                  <tr
                    key={lang.language}
                    className={`border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors ${i === 0 ? 'bg-cyan-500/[0.04]' : ''}`}
                  >
                    <td className="py-2.5 px-3 font-medium" style={{ color }}>
                      {lang.language}
                    </td>
                    <td className="py-2.5 px-3 text-right font-mono text-gray-300">
                      {formatMs(lang.mean)}
                    </td>
                    <td className="py-2.5 px-3 text-right font-mono text-gray-300">
                      {formatMs(lang.p50)}
                    </td>
                    <td className="py-2.5 px-3 text-right font-mono text-gray-400">
                      {formatMs(lang.p95)}
                    </td>
                    <td className="py-2.5 px-3 text-right font-mono text-gray-500">
                      --
                    </td>
                    <td className="py-2.5 px-3 text-right font-mono text-gray-400">
                      {lang.rps > 0 ? lang.rps.toFixed(0) : '--'}
                    </td>
                    <td className={`py-2.5 px-3 text-right font-mono ${limited ? 'text-amber-500' : 'text-gray-500'}`}>
                      {lang.run_count}
                      {limited && <span className="text-gray-600 text-xs ml-1">*</span>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {sortedForTable.some(l => l.run_count < 3) && (
            <p className="text-xs text-gray-600 mt-2 px-3">
              * fewer than 3 runs — limited data, interpret with caution
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function LeaderboardTab({ entries }: { entries: BenchmarkLeaderboardEntry[] }) {
  if (entries.length === 0) {
    return (
      <div className="text-center text-gray-500 py-16">
        <p className="text-lg">No leaderboard data yet</p>
        <p className="text-sm mt-2">Upload benchmark results to populate the leaderboard.</p>
      </div>
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-gray-800 text-left text-gray-500 text-xs uppercase tracking-wider">
            <th className="py-2 px-3 w-12">Rank</th>
            <th className="py-2 px-3">Language</th>
            <th className="py-2 px-3">Runtime</th>
            <th className="py-2 px-3 text-right">Mean Latency</th>
            <th className="py-2 px-3 text-right">P99 Latency</th>
            <th className="py-2 px-3 text-right">Throughput</th>
            <th className="py-2 px-3">Cloud</th>
            <th className="py-2 px-3">Phase</th>
            <th className="py-2 px-3 text-right">Concurrency</th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry, i) => (
            <tr
              key={`${entry.language}-${entry.runtime}`}
              className={`border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors ${
                i === 0 ? 'bg-yellow-500/[0.06]' : i === 1 ? 'bg-gray-300/[0.03]' : i === 2 ? 'bg-orange-400/[0.03]' : ''
              }`}
            >
              <td className={`py-2.5 px-3 font-mono font-bold ${rankColor(i)}`}>
                <span className={i < 3 ? 'inline-flex items-center gap-1' : ''}>
                  {i === 0 && <span title="Gold">{'\uD83E\uDD47'}</span>}
                  {i === 1 && <span title="Silver">{'\uD83E\uDD48'}</span>}
                  {i === 2 && <span title="Bronze">{'\uD83E\uDD49'}</span>}
                  {getRank(i)}
                </span>
              </td>
              <td className="py-2.5 px-3 font-medium text-gray-200">{entry.language}</td>
              <td className="py-2.5 px-3 text-gray-400">{entry.runtime}</td>
              <td className="py-2.5 px-3 text-right font-mono text-gray-300">
                {formatMs(entry.metrics?.latency_mean_ms)}
              </td>
              <td className="py-2.5 px-3 text-right font-mono text-gray-400">
                {formatMs(entry.metrics?.latency_p99_ms)}
              </td>
              <td className="py-2.5 px-3 text-right font-mono text-gray-400">
                {entry.metrics?.requests_per_sec?.toFixed(0) ?? '--'}
              </td>
              <td className="py-2.5 px-3 text-gray-500">{entry.cloud ?? '--'}</td>
              <td className="py-2.5 px-3 text-gray-500">{entry.phase ?? '--'}</td>
              <td className="py-2.5 px-3 text-right text-gray-500">{entry.concurrency ?? '--'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ComparisonTab({ entries }: { entries: BenchmarkLeaderboardEntry[] }) {
  if (entries.length === 0) {
    return (
      <div className="text-center text-gray-500 py-16">
        No data to compare.
      </div>
    );
  }

  const maxLatency = Math.max(
    ...entries.map(e => e.metrics?.latency_mean_ms ?? 0).filter(v => v > 0),
    1,
  );
  const maxThroughput = Math.max(
    ...entries.map(e => e.metrics?.requests_per_sec ?? 0).filter(v => v > 0),
    1,
  );

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-xs uppercase tracking-wider text-gray-500 mb-3">Mean Latency (lower is better)</h3>
        <div className="space-y-2">
          {entries.map((entry) => {
            const val = entry.metrics?.latency_mean_ms ?? 0;
            const pct = maxLatency > 0 ? (val / maxLatency) * 100 : 0;
            const color = languageColor(entry.language);
            return (
              <div key={`lat-${entry.language}-${entry.runtime}`} className="flex items-center gap-3">
                <span className="w-36 text-sm truncate" style={{ color }}>{entry.language}</span>
                <div className="flex-1 h-6 bg-gray-800/50 overflow-hidden">
                  <div
                    className="h-full transition-all"
                    style={{ width: `${Math.max(pct, 1)}%`, backgroundColor: color, opacity: 0.6 }}
                  />
                </div>
                <span className="w-24 text-right text-sm font-mono text-gray-300">
                  {formatMs(val)}
                </span>
              </div>
            );
          })}
        </div>
      </div>

      <div>
        <h3 className="text-xs uppercase tracking-wider text-gray-500 mb-3">Throughput (higher is better)</h3>
        <div className="space-y-2">
          {entries.map((entry) => {
            const val = entry.metrics?.requests_per_sec ?? 0;
            const pct = maxThroughput > 0 ? (val / maxThroughput) * 100 : 0;
            const color = languageColor(entry.language);
            return (
              <div key={`rps-${entry.language}-${entry.runtime}`} className="flex items-center gap-3">
                <span className="w-36 text-sm truncate" style={{ color }}>{entry.language}</span>
                <div className="flex-1 h-6 bg-gray-800/50 overflow-hidden">
                  <div
                    className="h-full transition-all"
                    style={{ width: `${Math.max(pct, 1)}%`, backgroundColor: color, opacity: 0.6 }}
                  />
                </div>
                <span className="w-24 text-right text-sm font-mono text-gray-300">
                  {val > 0 ? `${val.toFixed(0)} rps` : '--'}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function TimelineTab({ runs }: { runs: BenchmarkRun[] }) {
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [details, setDetails] = useState<Record<string, BenchmarkRun>>({});

  const toggleRun = useCallback(async (runId: string) => {
    if (expandedId === runId) {
      setExpandedId(null);
      return;
    }
    setExpandedId(runId);
    if (!details[runId]) {
      try {
        const run = await api.getLeaderboardRun(runId);
        setDetails(prev => ({ ...prev, [runId]: run }));
      } catch {
        // ignore
      }
    }
  }, [expandedId, details]);

  if (runs.length === 0) {
    return (
      <div className="text-center text-gray-500 py-16">
        No benchmark runs yet.
      </div>
    );
  }

  return (
    <div className="space-y-1">
      {runs.map(run => {
        const expanded = expandedId === run.run_id;
        const detail = details[run.run_id];
        return (
          <div key={run.run_id} className="border border-gray-800 rounded">
            <button
              onClick={() => toggleRun(run.run_id)}
              className="w-full flex items-center gap-3 px-3 py-2.5 text-left hover:bg-gray-800/30 transition-colors"
            >
              <span className="text-gray-500 text-xs">{expanded ? '\u25BC' : '\u25B6'}</span>
              <span className="text-sm text-gray-200 font-medium flex-1">{run.name}</span>
              <span className={`text-xs px-1.5 py-0.5 rounded ${
                run.status === 'completed'
                  ? 'bg-green-500/20 text-green-400'
                  : run.status === 'running'
                    ? 'bg-yellow-500/20 text-yellow-400'
                    : 'bg-gray-500/20 text-gray-400'
              }`}>
                {run.status}
              </span>
              <span className="text-xs text-gray-600 font-mono">
                {new Date(run.started_at).toLocaleDateString()}
              </span>
            </button>
            {expanded && detail?.results && detail.results.length > 0 && (
              <div className="border-t border-gray-800 px-3 py-2 bg-gray-900/30">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="text-gray-600 text-left">
                      <th className="py-1 pr-3">Language</th>
                      <th className="py-1 pr-3">Runtime</th>
                      <th className="py-1 pr-3 text-right">Mean Latency</th>
                      <th className="py-1 pr-3 text-right">P99</th>
                      <th className="py-1 pr-3 text-right">RPS</th>
                      <th className="py-1 pr-3">Cloud</th>
                    </tr>
                  </thead>
                  <tbody>
                    {detail.results.map(r => (
                      <tr key={r.result_id} className="border-t border-gray-800/30">
                        <td className="py-1 pr-3 text-gray-300">{r.language}</td>
                        <td className="py-1 pr-3 text-gray-400">{r.runtime}</td>
                        <td className="py-1 pr-3 text-right font-mono text-gray-300">
                          {formatMs(r.metrics?.latency_mean_ms)}
                        </td>
                        <td className="py-1 pr-3 text-right font-mono text-gray-400">
                          {formatMs(r.metrics?.latency_p99_ms)}
                        </td>
                        <td className="py-1 pr-3 text-right font-mono text-gray-400">
                          {r.metrics?.requests_per_sec?.toFixed(0) ?? '--'}
                        </td>
                        <td className="py-1 pr-3 text-gray-500">{r.cloud ?? '--'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {expanded && detail?.results && detail.results.length === 0 && (
              <div className="border-t border-gray-800 px-3 py-3 text-xs text-gray-500 bg-gray-900/30">
                No results in this run.
              </div>
            )}
            {expanded && !detail && (
              <div className="border-t border-gray-800 px-3 py-3 text-xs text-gray-500 bg-gray-900/30 motion-safe:animate-pulse">
                Loading...
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function LeaderboardPage() {
  usePageTitle('Leaderboard');

  const [tab, setTab] = useState<Tab>('grouped');
  const [entries, setEntries] = useState<BenchmarkLeaderboardEntry[]>([]);
  const [runs, setRuns] = useState<BenchmarkRun[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [lb, r] = await Promise.all([
        api.getLeaderboard(),
        api.getLeaderboardRuns(),
      ]);
      setEntries(lb);
      setRuns(r);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : 'Failed to load leaderboard';
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (tab !== 'grouped') {
      void fetchData();
    }
  }, [fetchData, tab]);

  const tabs: { key: Tab; label: string }[] = [
    { key: 'grouped', label: 'Distribution' },
    { key: 'leaderboard', label: 'Leaderboard' },
    { key: 'comparison', label: 'Comparison' },
    { key: 'timeline', label: 'Timeline' },
  ];

  return (
    <div className="p-4 md:p-6 max-w-6xl">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-lg font-semibold text-gray-100">Leaderboard</h1>
          <p className="text-xs text-gray-500 mt-1">
            Language performance rankings from benchmark runs
          </p>
        </div>
        {tab !== 'grouped' && entries.length > 0 && (
          <span className="text-xs text-gray-600 font-mono">
            {entries.length} {entries.length === 1 ? 'entry' : 'entries'}
          </span>
        )}
      </div>

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-gray-800">
        {tabs.map(t => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`px-3 py-2 text-sm border-b-2 transition-colors ${
              tab === t.key
                ? 'border-cyan-500 text-cyan-400'
                : 'border-transparent text-gray-500 hover:text-gray-300'
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* Content */}
      {tab === 'grouped' && <GroupedTab />}
      {tab !== 'grouped' && (
        <>
          {loading ? (
            <div className="text-center text-gray-500 py-16 motion-safe:animate-pulse">
              Loading leaderboard...
            </div>
          ) : error ? (
            <div className="text-center text-red-400 py-16">
              {error}
            </div>
          ) : (
            <>
              {tab === 'leaderboard' && <LeaderboardTab entries={entries} />}
              {tab === 'comparison' && <ComparisonTab entries={entries} />}
              {tab === 'timeline' && <TimelineTab runs={runs} />}
            </>
          )}
        </>
      )}
    </div>
  );
}
