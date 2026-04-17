import { useEffect, useMemo, useState, useCallback } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api } from '../api/client';
import type {
  BenchmarkConfigResults,
  BenchmarkTestbedRow,
  ConfigTestbedResult,
  BenchmarkConfigResultSummary,
} from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import {
  HorizontalBoxWhiskerChart,
  type HBoxGroup,
} from '../components/charts/HorizontalBoxWhiskerChart';
import { PhaseBreakdown, type PhaseData } from '../components/benchmark/PhaseBreakdown';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import {
  formatBenchmarkMetric,
  formatBenchmarkDelta,
} from '../lib/benchmark';
import { languageColor } from '../lib/languageColors';

// ── Constants ──────────────────────────────────────────────────────────────────

const MAX_EXPANDED = 2;

// ── Helpers ────────────────────────────────────────────────────────────────────

function testbedLabel(testbed: BenchmarkTestbedRow): string {
  const os = testbed.os === 'windows' ? 'Win' : 'Linux';
  return `${testbed.cloud} / ${testbed.region} (${testbed.topology}) [${os}]`;
}

/**
 * Find the "primary" summary for a result: metric_name=latency, payload_bytes null,
 * protocol preference http1 > http2 > http3, lowest case_id as tiebreaker.
 */
function findPrimarySummary(
  summaries: BenchmarkConfigResultSummary[],
): BenchmarkConfigResultSummary | null {
  const candidates = summaries.filter(
    (s) => (s.metric_name === 'latency' || s.metric_name === 'Total ms') && (s.payload_bytes == null),
  );
  if (candidates.length === 0) return summaries[0] ?? null;

  const protocolOrder: Record<string, number> = { http1: 0, http2: 1, http3: 2 };
  candidates.sort((a, b) => {
    const oa = protocolOrder[a.protocol] ?? 99;
    const ob = protocolOrder[b.protocol] ?? 99;
    if (oa !== ob) return oa - ob;
    return a.case_id < b.case_id ? -1 : a.case_id > b.case_id ? 1 : 0;
  });
  return candidates[0];
}

/**
 * Build HBoxGroup[] from active testbed results. Returns groups and a map of
 * language → color for use in PhaseBreakdown.
 */
function buildBoxGroups(
  results: ConfigTestbedResult[],
): { groups: HBoxGroup[]; colorMap: Map<string, string> } {
  const groups: HBoxGroup[] = [];
  const colorMap = new Map<string, string>();

  for (const r of results) {
    if (r.summaries.length === 0) continue;
    const primary = findPrimarySummary(r.summaries);
    if (!primary) {
      // Dev-only: no primary summary for this language — skipped
      continue;
    }
    const color = languageColor(r.language);
    colorMap.set(r.language, color);
    groups.push({
      label: r.language,
      color,
      p5: primary.p5,
      p25: primary.p25,
      p50: primary.p50,
      p75: primary.p75,
      p95: primary.p95,
      mean: primary.mean,
    });
  }

  // sort by p50 ascending (fastest first)
  groups.sort((a, b) => a.p50 - b.p50);
  return { groups, colorMap };
}

/**
 * Extract PhaseData[] from a ConfigTestbedResult.
 * Groups summaries by protocol (and payload if present).
 * Phase breakdown (dns/tcp/tls) is not available in the current API response —
 * ttfb is approximated as mean * 0.6.
 */
function extractPhaseData(result: ConfigTestbedResult): PhaseData[] {
  // Group by (protocol, payload_bytes)
  const groupMap = new Map<string, BenchmarkConfigResultSummary[]>();

  for (const s of result.summaries) {
    const key = s.payload_bytes != null
      ? `${s.protocol}::${s.payload_bytes}`
      : s.protocol;
    const arr = groupMap.get(key);
    if (arr) arr.push(s);
    else groupMap.set(key, [s]);
  }

  const phases: PhaseData[] = [];

  for (const [key, summaries] of groupMap) {
    // Use the latency summary if available, otherwise first
    const primary = summaries.find((s) => (s.metric_name === 'latency' || s.metric_name === 'Total ms')) ?? summaries[0];

    // Derive mode label
    let mode: string;
    if (primary.payload_bytes != null && primary.payload_bytes > 0) {
      const kb = primary.payload_bytes / 1024;
      const payloadLabel = kb >= 1024
        ? `${(kb / 1024).toFixed(0)}M`
        : kb >= 1
        ? `${kb.toFixed(0)}k`
        : `${primary.payload_bytes}b`;
      mode = `${primary.protocol} ${payloadLabel}`;
    } else {
      mode = primary.protocol;
    }

    const total_ms = primary.mean;
    // Approximate ttfb as 60% of total; transfer is remainder
    const ttfb_ms = total_ms * 0.6;
    const transfer_ms = total_ms - ttfb_ms;

    phases.push({
      mode,
      dns_ms: null,
      tcp_ms: null,
      tls_ms: null,
      ttfb_ms,
      transfer_ms,
      total_ms,
    });

    // suppress unused key var warning
    void key;
  }

  // Sort modes: http1, http2, http3, then alphabetical
  const protocolOrder = (mode: string) => {
    if (mode.startsWith('http1')) return '0';
    if (mode.startsWith('http2')) return '1';
    if (mode.startsWith('http3')) return '2';
    return mode;
  };

  phases.sort((a, b) => protocolOrder(a.mode).localeCompare(protocolOrder(b.mode)));

  return phases;
}

// ── Cross-testbed helpers ─────────────────────────────────────────────────────

interface CrossTestbedRow {
  language: string;
  testbeds: Map<string, { mean: number; p50: number; p95: number }>;
}

function weightedAvg(
  summaries: BenchmarkConfigResultSummary[],
  key: keyof Pick<BenchmarkConfigResultSummary, 'mean' | 'p50' | 'p95' | 'p99' | 'stddev'>,
): number {
  const totalSamples = summaries.reduce((s, x) => s + x.included_sample_count, 0);
  if (totalSamples === 0) return 0;
  return summaries.reduce((s, x) => s + x[key] * x.included_sample_count, 0) / totalSamples;
}

function buildCrossTestbedRows(
  results: ConfigTestbedResult[],
  testbeds: BenchmarkTestbedRow[],
): CrossTestbedRow[] {
  const byLang = new Map<string, CrossTestbedRow>();
  for (const r of results) {
    if (r.summaries.length === 0 || !r.testbed_id) continue;
    let row = byLang.get(r.language);
    if (!row) {
      row = { language: r.language, testbeds: new Map() };
      byLang.set(r.language, row);
    }
    row.testbeds.set(r.testbed_id, {
      mean: weightedAvg(r.summaries, 'mean'),
      p50: weightedAvg(r.summaries, 'p50'),
      p95: weightedAvg(r.summaries, 'p95'),
    });
  }

  return Array.from(byLang.values())
    .filter((row) => row.testbeds.size >= Math.min(2, testbeds.length))
    .sort((a, b) => {
      const aMean = Math.min(...Array.from(a.testbeds.values()).map((c) => c.mean));
      const bMean = Math.min(...Array.from(b.testbeds.values()).map((c) => c.mean));
      return aMean - bMean;
    });
}

// ── Main component ────────────────────────────────────────────────────────────

export function BenchmarkConfigResultsPage() {
  const { projectId } = useProject();
  const { configId } = useParams<{ configId: string }>();
  const [data, setData] = useState<BenchmarkConfigResults | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeTestbed, setActiveTestbed] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string[]>([]);
  const [hideIncomplete, setHideIncomplete] = useState(true);

  usePageTitle(data ? `Results: ${data.config.name}` : 'Benchmark Results');

  useEffect(() => {
    if (!projectId || !configId) return;
    api
      .getBenchmarkConfigResults(projectId, configId)
      .then((res) => {
        setData(res);
        setError(null);
        setLoading(false);
        if (res.testbeds.length > 0 && !activeTestbed) {
          setActiveTestbed(res.testbeds[0].testbed_id);
        }
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [projectId, configId, activeTestbed]);

  const testbedMap = useMemo(() => {
    if (!data) return new Map<string, BenchmarkTestbedRow>();
    return new Map(data.testbeds.map((c) => [c.testbed_id, c]));
  }, [data]);

  const activeTestbedResults = useMemo(() => {
    if (!data || !activeTestbed) return [];
    return data.results.filter((r) => r.testbed_id === activeTestbed);
  }, [data, activeTestbed]);

  // Determine complete vs incomplete
  const completeResults = useMemo(
    () => activeTestbedResults.filter((r) => r.summaries.length > 0),
    [activeTestbedResults],
  );
  const incompleteCount = activeTestbedResults.length - completeResults.length;
  const allIncomplete = activeTestbedResults.length > 0 && completeResults.length === 0;

  // Effective hide toggle: default off when all are incomplete
  const effectiveHideIncomplete = allIncomplete ? false : hideIncomplete;

  const visibleResults = useMemo(
    () => (effectiveHideIncomplete ? completeResults : activeTestbedResults),
    [effectiveHideIncomplete, completeResults, activeTestbedResults],
  );

  const { groups: boxGroups, colorMap } = useMemo(
    () => buildBoxGroups(visibleResults),
    [visibleResults],
  );

  // Result lookup by language for the active testbed
  const resultByLanguage = useMemo(() => {
    const map = new Map<string, ConfigTestbedResult>();
    for (const r of activeTestbedResults) {
      map.set(r.language, r);
    }
    return map;
  }, [activeTestbedResults]);

  const crossTestbedRows = useMemo(
    () => (data ? buildCrossTestbedRows(data.results, data.testbeds) : []),
    [data],
  );

  const hasMultipleTestbeds = (data?.testbeds.length ?? 0) > 1;

  // Toggle expand/collapse (FIFO max 2)
  const handleClickGroup = useCallback((label: string) => {
    setExpanded((prev) => {
      if (prev.includes(label)) {
        return prev.filter((l) => l !== label);
      }
      const next = [...prev, label];
      if (next.length > MAX_EXPANDED) {
        next.shift(); // remove oldest
      }
      return next;
    });
  }, []);

  // Expanded set for chart highlighting
  const expandedSet = useMemo(() => new Set(expanded), [expanded]);

  if (loading) {
    return (
      <div className="px-6 py-8 text-sm text-gray-500 motion-safe:animate-pulse">
        Loading benchmark results...
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="px-6 py-8 text-sm text-red-400">
        {error || 'Failed to load benchmark results'}
      </div>
    );
  }

  const config = data.config;
  const durationSecs =
    config.started_at && config.finished_at
      ? Math.round(
          (new Date(config.finished_at).getTime() - new Date(config.started_at).getTime()) / 1000,
        )
      : null;

  return (
    <div className="px-6 py-4 space-y-6 max-w-[1400px]">
      <Breadcrumb
        items={[
          { label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` },
          { label: config.name },
        ]}
      />

      {/* Header */}
      <div className="space-y-1">
        <h1 className="text-xl font-semibold text-gray-100">{config.name}</h1>
        <div className="flex gap-4 text-sm text-gray-400">
          {config.template && <span>Template: {config.template}</span>}
          <span>Status: {config.status}</span>
          {durationSecs !== null && (
            <span>
              Duration: {Math.floor(durationSecs / 60)}m {durationSecs % 60}s
            </span>
          )}
          <span>
            {data.testbeds.length} testbed{data.testbeds.length !== 1 ? 's' : ''} /{' '}
            {data.results.length} result{data.results.length !== 1 ? 's' : ''}
          </span>
        </div>
      </div>

      {data.results.length === 0 && (
        <div className="text-sm text-gray-500 py-8">
          No results yet. Results will appear here as benchmark languages complete.
        </div>
      )}

      {/* Testbed tabs */}
      {data.testbeds.length > 0 && data.results.length > 0 && (
        <div className="border-b border-gray-700">
          <nav className="flex gap-1 -mb-px">
            {data.testbeds.map((testbed) => (
              <button
                key={testbed.testbed_id}
                onClick={() => {
                  setActiveTestbed(testbed.testbed_id);
                  setExpanded([]);
                }}
                className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
                  activeTestbed === testbed.testbed_id
                    ? 'border-cyan-400 text-cyan-400'
                    : 'border-transparent text-gray-400 hover:text-gray-300 hover:border-gray-600'
                }`}
              >
                {testbedLabel(testbed)}
              </button>
            ))}
            {hasMultipleTestbeds && (
              <button
                onClick={() => {
                  setActiveTestbed('__cross_testbed__');
                  setExpanded([]);
                }}
                className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
                  activeTestbed === '__cross_testbed__'
                    ? 'border-cyan-400 text-cyan-400'
                    : 'border-transparent text-gray-400 hover:text-gray-300 hover:border-gray-600'
                }`}
              >
                Cross-Testbed Comparison
              </button>
            )}
          </nav>
        </div>
      )}

      {/* Per-testbed chart + phase breakdown */}
      {activeTestbed && activeTestbed !== '__cross_testbed__' && activeTestbedResults.length > 0 && (
        <div className="space-y-4">
          {/* Section header with hide-incomplete toggle */}
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider">
              Language Comparison &mdash;{' '}
              {testbedMap.get(activeTestbed) ? testbedLabel(testbedMap.get(activeTestbed)!) : 'Unknown Testbed'}
            </h2>
            <div className="flex items-center gap-3 text-xs text-gray-500">
              {allIncomplete && (
                <span className="text-yellow-600">All results incomplete — showing all</span>
              )}
              {!allIncomplete && incompleteCount > 0 && (
                <button
                  onClick={() => setHideIncomplete((v) => !v)}
                  className="hover:text-gray-300 transition-colors"
                >
                  {effectiveHideIncomplete
                    ? `Show ${incompleteCount} incomplete`
                    : `Hide ${incompleteCount} incomplete`}
                </button>
              )}
            </div>
          </div>

          {visibleResults.length === 0 && (
            <div className="text-sm text-gray-500 py-4">
              No complete results yet.
            </div>
          )}

          {/* HorizontalBoxWhiskerChart */}
          {boxGroups.length > 0 && (
            <div>
              <HorizontalBoxWhiskerChart
                groups={boxGroups}
                unit="ms"
                title="Latency Distribution — click a row to expand phase breakdown"
                onClickGroup={handleClickGroup}
                expandedGroups={expandedSet}
              />
              <p className="text-xs text-gray-600 mt-1 ml-[70px]">
                Click a language row to expand phase breakdown (max {MAX_EXPANDED} at a time).
              </p>
            </div>
          )}

          {/* Phase breakdowns for expanded languages */}
          {expanded.length > 0 && (
            <div className="space-y-3 ml-[70px]">
              {expanded.map((lang, idx) => {
                const result = resultByLanguage.get(lang);
                if (!result) return null;
                const color = colorMap.get(lang) ?? languageColor(lang);
                const modes = extractPhaseData(result);

                // Comparison: the other expanded language (if 2 expanded)
                let comparison: { otherLanguage: string; otherColor: string; otherModes: PhaseData[] } | undefined;
                if (expanded.length === MAX_EXPANDED) {
                  const otherLang = expanded[1 - idx];
                  const otherResult = resultByLanguage.get(otherLang);
                  if (otherResult) {
                    comparison = {
                      otherLanguage: otherLang,
                      otherColor: colorMap.get(otherLang) ?? languageColor(otherLang),
                      otherModes: extractPhaseData(otherResult),
                    };
                  }
                }

                return (
                  <div key={lang}>
                    <div className="flex items-center justify-between mb-1">
                      <span className="text-xs font-mono font-semibold" style={{ color }}>
                        {lang}
                      </span>
                      <button
                        onClick={() => handleClickGroup(lang)}
                        className="text-xs text-gray-600 hover:text-gray-400 transition-colors"
                      >
                        collapse
                      </button>
                    </div>
                    <PhaseBreakdown
                      color={color}
                      modes={modes}
                      comparison={comparison}
                    />
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* Cross-testbed comparison */}
      {activeTestbed === '__cross_testbed__' && hasMultipleTestbeds && crossTestbedRows.length > 0 && (
        <div className="space-y-4">
          <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider">
            Cross-Testbed Comparison
          </h2>

          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-700 text-gray-400 text-left">
                  <th className="py-2 pr-4 font-medium">Language</th>
                  {data.testbeds.map((testbed) => (
                    <th key={testbed.testbed_id} className="py-2 pr-4 font-medium text-right">
                      {testbedLabel(testbed)}
                    </th>
                  ))}
                  {data.testbeds.length === 2 && (
                    <>
                      <th className="py-2 pr-4 font-medium text-right">Delta</th>
                      <th className="py-2 font-medium text-center">Winner</th>
                    </>
                  )}
                </tr>
              </thead>
              <tbody>
                {crossTestbedRows.map((row) => {
                  const testbedValues = data.testbeds.map((c) => row.testbeds.get(c.testbed_id));
                  const means = testbedValues.map((v) => v?.mean ?? Infinity);
                  const minMean = Math.min(...means);
                  const maxMean = Math.max(...means.filter((m) => m !== Infinity));
                  const deltaPercent =
                    data.testbeds.length === 2 && minMean > 0
                      ? ((maxMean - minMean) / minMean) * 100
                      : null;
                  const winnerIdx = means.indexOf(minMean);
                  const winnerTestbed =
                    data.testbeds.length === 2 && winnerIdx >= 0
                      ? data.testbeds[winnerIdx]
                      : null;

                  return (
                    <tr key={row.language} className="border-b border-gray-800 text-gray-300">
                      <td className="py-2 pr-4 font-mono">{row.language}</td>
                      {data.testbeds.map((testbed) => {
                        const v = row.testbeds.get(testbed.testbed_id);
                        const isBest = v && v.mean === minMean;
                        return (
                          <td
                            key={testbed.testbed_id}
                            className={`py-2 pr-4 text-right font-mono ${isBest ? 'text-cyan-300' : ''}`}
                          >
                            {v ? formatBenchmarkMetric(v.mean, 'ms') : '-'}
                          </td>
                        );
                      })}
                      {data.testbeds.length === 2 && (
                        <>
                          <td className="py-2 pr-4 text-right font-mono text-yellow-400">
                            {deltaPercent !== null ? formatBenchmarkDelta(deltaPercent) : '-'}
                          </td>
                          <td className="py-2 text-center font-mono text-sm">
                            {winnerTestbed ? (
                              <span className="text-cyan-400">{winnerTestbed.cloud}</span>
                            ) : (
                              '-'
                            )}
                          </td>
                        </>
                      )}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {activeTestbed === '__cross_testbed__' && crossTestbedRows.length === 0 && (
        <div className="text-sm text-gray-500 py-4">
          Cross-testbed comparison requires results from at least two testbeds for the same language.
        </div>
      )}

      {/* Link to full pipeline comparison */}
      {data.results.length >= 2 && (
        <div className="pt-4 border-t border-gray-800">
          <Link
            to={`/projects/${projectId}/benchmarks/compare?runs=${data.results.map((r) => r.run_id).join(',')}`}
            className="text-sm text-cyan-400 hover:text-cyan-300"
          >
            Open full pipeline comparison view &rarr;
          </Link>
        </div>
      )}
    </div>
  );
}
