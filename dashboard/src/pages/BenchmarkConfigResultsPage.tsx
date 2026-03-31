import { useEffect, useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api } from '../api/client';
import type {
  BenchmarkConfigResults,
  BenchmarkCellRow,
  ConfigCellResult,
  BenchmarkConfigResultSummary,
} from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { BoxWhiskerChart, type BoxGroup } from '../components/charts/BoxWhiskerChart';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import {
  formatBenchmarkMetric,
  formatBenchmarkNumber,
  formatBenchmarkDelta,
} from '../lib/benchmark';

function cellLabel(cell: BenchmarkCellRow): string {
  return `${cell.cloud} / ${cell.region} (${cell.topology})`;
}

interface LanguageRow {
  language: string;
  mean: number;
  p50: number;
  p95: number;
  p99: number;
  stddev: number;
  rps: number;
  sampleCount: number;
}

function computeLanguageRows(results: ConfigCellResult[]): LanguageRow[] {
  return results
    .filter((r) => r.summaries.length > 0)
    .map((r) => {
      // Aggregate across all summaries for this result
      const totalSamples = r.summaries.reduce((s, x) => s + x.included_sample_count, 0);
      return {
        language: r.language,
        mean: weightedAvg(r.summaries, 'mean'),
        p50: weightedAvg(r.summaries, 'p50'),
        p95: weightedAvg(r.summaries, 'p95'),
        p99: weightedAvg(r.summaries, 'p99'),
        stddev: weightedAvg(r.summaries, 'stddev'),
        rps: r.summaries.reduce((s, x) => s + x.rps, 0),
        sampleCount: totalSamples,
      };
    })
    .sort((a, b) => a.mean - b.mean);
}

function weightedAvg(
  summaries: BenchmarkConfigResultSummary[],
  key: keyof Pick<BenchmarkConfigResultSummary, 'mean' | 'p50' | 'p95' | 'p99' | 'stddev'>,
): number {
  const totalSamples = summaries.reduce((s, x) => s + x.included_sample_count, 0);
  if (totalSamples === 0) return 0;
  return summaries.reduce((s, x) => s + x[key] * x.included_sample_count, 0) / totalSamples;
}

function buildBoxGroups(results: ConfigCellResult[]): BoxGroup[] {
  return results
    .filter((r) => r.summaries.length > 0)
    .map((r) => {
      const s = r.summaries[0]; // primary summary
      return {
        label: r.language,
        p5: s.p5,
        p25: s.p25,
        p50: s.p50,
        p75: s.p75,
        p95: s.p95,
      };
    })
    .sort((a, b) => a.p50 - b.p50);
}

interface CrossCellRow {
  language: string;
  cells: Map<string, { mean: number; p50: number; p95: number }>;
}

function buildCrossCellRows(
  results: ConfigCellResult[],
  cells: BenchmarkCellRow[],
): CrossCellRow[] {
  const byLang = new Map<string, CrossCellRow>();
  for (const r of results) {
    if (r.summaries.length === 0 || !r.cell_id) continue;
    let row = byLang.get(r.language);
    if (!row) {
      row = { language: r.language, cells: new Map() };
      byLang.set(r.language, row);
    }
    row.cells.set(r.cell_id, {
      mean: weightedAvg(r.summaries, 'mean'),
      p50: weightedAvg(r.summaries, 'p50'),
      p95: weightedAvg(r.summaries, 'p95'),
    });
  }

  // Only return rows that have data for at least 2 cells
  return Array.from(byLang.values())
    .filter((row) => row.cells.size >= Math.min(2, cells.length))
    .sort((a, b) => {
      const aMean = Math.min(...Array.from(a.cells.values()).map((c) => c.mean));
      const bMean = Math.min(...Array.from(b.cells.values()).map((c) => c.mean));
      return aMean - bMean;
    });
}

export function BenchmarkConfigResultsPage() {
  const { projectId } = useProject();
  const { configId } = useParams<{ configId: string }>();
  const [data, setData] = useState<BenchmarkConfigResults | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeCell, setActiveCell] = useState<string | null>(null);

  usePageTitle(data ? `Results: ${data.config.name}` : 'Benchmark Results');

  useEffect(() => {
    if (!projectId || !configId) return;
    api
      .getBenchmarkConfigResults(projectId, configId)
      .then((res) => {
        setData(res);
        setError(null);
        setLoading(false);
        // Default to first cell
        if (res.cells.length > 0 && !activeCell) {
          setActiveCell(res.cells[0].cell_id);
        }
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [projectId, configId, activeCell]);

  const cellMap = useMemo(() => {
    if (!data) return new Map<string, BenchmarkCellRow>();
    return new Map(data.cells.map((c) => [c.cell_id, c]));
  }, [data]);

  const activeCellResults = useMemo(() => {
    if (!data || !activeCell) return [];
    return data.results.filter((r) => r.cell_id === activeCell);
  }, [data, activeCell]);

  const languageRows = useMemo(() => computeLanguageRows(activeCellResults), [activeCellResults]);
  const boxGroups = useMemo(() => buildBoxGroups(activeCellResults), [activeCellResults]);
  const crossCellRows = useMemo(
    () => (data ? buildCrossCellRows(data.results, data.cells) : []),
    [data],
  );

  const hasMultipleCells = (data?.cells.length ?? 0) > 1;

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
            {data.cells.length} cell{data.cells.length !== 1 ? 's' : ''} /{' '}
            {data.results.length} result{data.results.length !== 1 ? 's' : ''}
          </span>
        </div>
      </div>

      {data.results.length === 0 && (
        <div className="text-sm text-gray-500 py-8">
          No results yet. Results will appear here as benchmark languages complete.
        </div>
      )}

      {/* Cell tabs */}
      {data.cells.length > 0 && data.results.length > 0 && (
        <div className="border-b border-gray-700">
          <nav className="flex gap-1 -mb-px">
            {data.cells.map((cell) => (
              <button
                key={cell.cell_id}
                onClick={() => setActiveCell(cell.cell_id)}
                className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
                  activeCell === cell.cell_id
                    ? 'border-cyan-400 text-cyan-400'
                    : 'border-transparent text-gray-400 hover:text-gray-300 hover:border-gray-600'
                }`}
              >
                {cellLabel(cell)}
              </button>
            ))}
            {hasMultipleCells && (
              <button
                onClick={() => setActiveCell('__cross_cell__')}
                className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
                  activeCell === '__cross_cell__'
                    ? 'border-cyan-400 text-cyan-400'
                    : 'border-transparent text-gray-400 hover:text-gray-300 hover:border-gray-600'
                }`}
              >
                Cross-Cell Comparison
              </button>
            )}
          </nav>
        </div>
      )}

      {/* Per-cell language table */}
      {activeCell && activeCell !== '__cross_cell__' && languageRows.length > 0 && (
        <div className="space-y-4">
          <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider">
            Language Comparison &mdash; {cellMap.get(activeCell) ? cellLabel(cellMap.get(activeCell)!) : 'Unknown Cell'}
          </h2>

          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-700 text-gray-400 text-left">
                  <th className="py-2 pr-4 font-medium">Language</th>
                  <th className="py-2 pr-4 font-medium text-right">Mean</th>
                  <th className="py-2 pr-4 font-medium text-right">p50</th>
                  <th className="py-2 pr-4 font-medium text-right">p95</th>
                  <th className="py-2 pr-4 font-medium text-right">p99</th>
                  <th className="py-2 pr-4 font-medium text-right">StdDev</th>
                  <th className="py-2 pr-4 font-medium text-right">RPS</th>
                  <th className="py-2 font-medium text-right">Samples</th>
                </tr>
              </thead>
              <tbody>
                {languageRows.map((row, i) => (
                  <tr
                    key={row.language}
                    className={`border-b border-gray-800 ${i === 0 ? 'text-cyan-300' : 'text-gray-300'}`}
                  >
                    <td className="py-2 pr-4 font-mono">
                      {i === 0 && <span className="text-yellow-400 mr-1" title="Fastest">&#9733;</span>}
                      {row.language}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">
                      {formatBenchmarkMetric(row.mean, 'ms')}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">
                      {formatBenchmarkMetric(row.p50, 'ms')}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">
                      {formatBenchmarkMetric(row.p95, 'ms')}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">
                      {formatBenchmarkMetric(row.p99, 'ms')}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">
                      {formatBenchmarkMetric(row.stddev, 'ms')}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">
                      {formatBenchmarkNumber(row.rps, 1)}
                    </td>
                    <td className="py-2 text-right font-mono text-gray-500">
                      {row.sampleCount}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Box-and-whisker chart */}
          {boxGroups.length > 0 && (
            <div className="mt-4">
              <BoxWhiskerChart
                groups={boxGroups}
                unit="ms"
                title="Latency Distribution by Language"
              />
            </div>
          )}
        </div>
      )}

      {/* Cross-cell comparison */}
      {activeCell === '__cross_cell__' && hasMultipleCells && crossCellRows.length > 0 && (
        <div className="space-y-4">
          <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider">
            Cross-Cell Comparison
          </h2>

          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-700 text-gray-400 text-left">
                  <th className="py-2 pr-4 font-medium">Language</th>
                  {data.cells.map((cell) => (
                    <th key={cell.cell_id} className="py-2 pr-4 font-medium text-right">
                      {cellLabel(cell)}
                    </th>
                  ))}
                  {data.cells.length === 2 && (
                    <>
                      <th className="py-2 pr-4 font-medium text-right">Delta</th>
                      <th className="py-2 font-medium text-center">Winner</th>
                    </>
                  )}
                </tr>
              </thead>
              <tbody>
                {crossCellRows.map((row) => {
                  const cellValues = data.cells.map((c) => row.cells.get(c.cell_id));
                  const means = cellValues.map((v) => v?.mean ?? Infinity);
                  const minMean = Math.min(...means);
                  const maxMean = Math.max(...means.filter((m) => m !== Infinity));
                  const deltaPercent =
                    data.cells.length === 2 && minMean > 0
                      ? ((maxMean - minMean) / minMean) * 100
                      : null;
                  const winnerIdx = means.indexOf(minMean);
                  const winnerCell =
                    data.cells.length === 2 && winnerIdx >= 0
                      ? data.cells[winnerIdx]
                      : null;

                  return (
                    <tr key={row.language} className="border-b border-gray-800 text-gray-300">
                      <td className="py-2 pr-4 font-mono">{row.language}</td>
                      {data.cells.map((cell) => {
                        const v = row.cells.get(cell.cell_id);
                        const isBest = v && v.mean === minMean;
                        return (
                          <td
                            key={cell.cell_id}
                            className={`py-2 pr-4 text-right font-mono ${isBest ? 'text-cyan-300' : ''}`}
                          >
                            {v ? formatBenchmarkMetric(v.mean, 'ms') : '-'}
                          </td>
                        );
                      })}
                      {data.cells.length === 2 && (
                        <>
                          <td className="py-2 pr-4 text-right font-mono text-yellow-400">
                            {deltaPercent !== null ? formatBenchmarkDelta(deltaPercent) : '-'}
                          </td>
                          <td className="py-2 text-center font-mono text-sm">
                            {winnerCell ? (
                              <span className="text-cyan-400">
                                {winnerCell.cloud}
                              </span>
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

      {activeCell === '__cross_cell__' && crossCellRows.length === 0 && (
        <div className="text-sm text-gray-500 py-4">
          Cross-cell comparison requires results from at least two cells for the same language.
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
