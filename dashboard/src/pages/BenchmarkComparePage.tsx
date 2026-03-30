import { useEffect, useMemo, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkComparisonReport } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import {
  formatBenchmarkCaseLabel,
  formatBenchmarkCount,
  formatBenchmarkDelta,
  formatBenchmarkInterval,
  formatBenchmarkMetric,
  formatBenchmarkRatio,
} from '../lib/benchmark';
import {
  deleteBenchmarkComparePreset,
  loadBenchmarkComparePresets,
  saveBenchmarkComparePreset,
  type BenchmarkComparePreset,
} from '../lib/benchmarkPresets';

function formatRunEnvironmentSummary(run: BenchmarkComparisonReport['runs'][number]): string {
  const parts = [];
  if (run.environment.network_type) parts.push(run.environment.network_type);
  if (run.environment.server_region) parts.push(run.environment.server_region);
  if (run.environment.baseline_rtt_p50_ms != null) parts.push(`RTT p50 ${formatBenchmarkMetric(run.environment.baseline_rtt_p50_ms, 'ms')}`);
  return parts.join(' · ') || 'environment fingerprint unavailable';
}

function getSharedRunValue(values: Array<string | null | undefined>): string {
  const uniqueValues = Array.from(
    new Set(values.filter((value): value is string => Boolean(value && value.trim()))),
  );
  return uniqueValues.length === 1 ? uniqueValues[0] : '';
}

export function BenchmarkComparePage() {
  const { projectId } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const runIds = useMemo(
    () =>
      (searchParams.get('runs') ?? '')
        .split(',')
        .map((value) => value.trim())
        .filter(Boolean),
    [searchParams],
  );
  const canCompare = Boolean(projectId) && runIds.length >= 2;
  const baselineFromQuery = searchParams.get('baseline');
  const [report, setReport] = useState<BenchmarkComparisonReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [presets, setPresets] = useState<BenchmarkComparePreset[]>([]);
  const [presetName, setPresetName] = useState('');
  const [presetStatus, setPresetStatus] = useState<string | null>(null);

  usePageTitle('Benchmark Compare');

  useEffect(() => {
    if (!projectId || !canCompare) return;

    const baselineRunId = baselineFromQuery && runIds.includes(baselineFromQuery)
      ? baselineFromQuery
      : runIds[0];

    api
      .compareBenchmarks(projectId, runIds, baselineRunId)
      .then((data) => {
        setReport(data);
        setError(null);
        setLoading(false);
      })
      .catch((err) => {
        setError(String(err));
        setLoading(false);
      });
  }, [baselineFromQuery, canCompare, projectId, runIds]);

  useEffect(() => {
    let cancelled = false;

    if (!projectId) return () => {
      cancelled = true;
    };

    void loadBenchmarkComparePresets(projectId).then((next) => {
      if (!cancelled) {
        setPresets(next);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [projectId]);

  const setBaseline = (runId: string) => {
    const next = new URLSearchParams(searchParams);
    next.set('baseline', runId);
    setSearchParams(next);
  };

  const saveCurrentPreset = async () => {
    if (!projectId || !report || runIds.length < 2) return;

    const defaultName =
      presetName.trim() ||
      `${getSharedRunValue(report.runs.map((run) => run.scenario)) || 'benchmark'} compare ${new Date().toLocaleDateString()}`;

    const nextPresets = await saveBenchmarkComparePreset(projectId, {
      name: defaultName,
      runIds,
      baselineRunId: report.baseline_run_id,
      filters: {
        targetSearch: getSharedRunValue(report.runs.map((run) => run.target_host)),
        scenario: getSharedRunValue(report.runs.map((run) => run.scenario)),
        phaseModel: getSharedRunValue(report.runs.map((run) => run.phase_model)),
        serverRegion: getSharedRunValue(
          report.runs.map((run) => run.environment.server_region),
        ),
        networkType: getSharedRunValue(
          report.runs.map((run) => run.environment.network_type),
        ),
      },
    });

    setPresets(nextPresets);
    setPresetName('');
    setPresetStatus(`Saved preset ${defaultName}.`);
  };

  const applyPreset = (preset: BenchmarkComparePreset) => {
    const next = new URLSearchParams();
    next.set('runs', preset.runIds.join(','));
    if (preset.baselineRunId) next.set('baseline', preset.baselineRunId);
    setSearchParams(next);
    setPresetName(preset.name);
    setPresetStatus(`Loaded preset ${preset.name}.`);
  };

  const removePreset = async (presetId: string) => {
    if (!projectId) return;
    const nextPresets = await deleteBenchmarkComparePreset(projectId, presetId);
    setPresets(nextPresets);
  };

  if (!canCompare) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: 'Compare' }]} />
        <div className="border border-gray-800 rounded-lg p-6 text-center">
          <p className="text-gray-400 text-sm">Select at least two benchmark runs from the benchmark list first.</p>
          <Link to={`/projects/${projectId}/benchmarks`} className="text-cyan-400 text-sm mt-2 inline-block">
            Back to benchmark runs
          </Link>
        </div>
      </div>
    );
  }

  if (loading && !report) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: 'Compare' }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading benchmark comparison...</div>
      </div>
    );
  }

  if (error && !report) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: 'Compare' }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to compare benchmark runs</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  if (!report) return null;

  const totalCandidateRows = report.cases.reduce(
    (count, caseComparison) => count + caseComparison.candidates.length,
    0,
  );
  const activePresetKey = `${runIds.join(',')}::${report.baseline_run_id}`;

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: 'Compare' }]} />

      <div className="flex flex-col md:flex-row md:items-center md:justify-between gap-4 mb-6">
        <div>
          <h2 className="text-xl font-bold text-gray-100">Benchmark Comparison</h2>
          <p className="text-sm text-gray-500 mt-1">
            Cross-run comparison computed from included measured samples with 95% confidence intervals.
          </p>
        </div>
        <label className="text-sm text-gray-400 flex items-center gap-2">
          Baseline
          <select
            value={report.baseline_run_id}
            onChange={(event) => setBaseline(event.target.value)}
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
          >
            {report.runs.map((run) => (
              <option key={run.run_id} value={run.run_id}>
                {run.target_host} [{run.run_id.slice(0, 8)}]
              </option>
            ))}
          </select>
        </label>
      </div>

      <div className="border border-cyan-500/20 bg-cyan-500/5 rounded-lg p-4 mb-6">
        <p className="text-sm text-cyan-100">{report.comparability_policy}</p>
        <p className="text-xs text-cyan-300/80 mt-2">
          gated comparisons {formatBenchmarkCount(report.gated_candidate_count)} of {formatBenchmarkCount(totalCandidateRows)}
        </p>
      </div>

      <div className="border border-gray-800 rounded-lg p-4 mb-6 bg-[var(--bg-surface)]/40">
        <div className="flex flex-col xl:flex-row xl:items-end xl:justify-between gap-4">
          <div>
            <h3 className="text-sm font-semibold text-gray-200">Saved Compare Presets</h3>
            <p className="text-xs text-gray-500 mt-1">
              Store the current run set, baseline, and any shared shortlist hints so you can reopen the same comparison without rebuilding it.
            </p>
          </div>
          <div className="flex flex-col sm:flex-row gap-2 xl:min-w-[24rem]">
            <label htmlFor="compare-preset-name" className="sr-only">
              Preset name
            </label>
            <input
              id="compare-preset-name"
              type="text"
              value={presetName}
              onChange={(event) => setPresetName(event.target.value)}
              placeholder="Preset name..."
              className="flex-1 bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-300 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
            <button
              onClick={saveCurrentPreset}
              disabled={runIds.length < 2}
              className="px-3 py-2 rounded border border-gray-700 text-sm text-gray-200 disabled:text-gray-600 disabled:border-gray-800 disabled:cursor-not-allowed hover:border-cyan-500 transition-colors"
            >
              Save current compare
            </button>
          </div>
        </div>

        {presetStatus && (
          <p className="text-xs text-cyan-300/80 mt-3">{presetStatus}</p>
        )}

        {presets.length === 0 ? (
          <div className="mt-4 rounded border border-dashed border-gray-800 p-4 text-xs text-gray-500">
            No saved compare presets yet.
          </div>
        ) : (
          <div className="space-y-2 mt-4">
            {presets.map((preset) => {
              const presetBaseline = preset.baselineRunId ?? preset.runIds[0];
              const presetKey = `${preset.runIds.join(',')}::${presetBaseline}`;
              const isActive = presetKey === activePresetKey;
              const filterSummary =
                [
                  preset.filters?.targetSearch ? `host ${preset.filters.targetSearch}` : null,
                  preset.filters?.scenario ? `scenario ${preset.filters.scenario}` : null,
                  preset.filters?.phaseModel ? `phase ${preset.filters.phaseModel}` : null,
                  preset.filters?.serverRegion ? `region ${preset.filters.serverRegion}` : null,
                  preset.filters?.networkType ? `network ${preset.filters.networkType}` : null,
                ]
                  .filter(Boolean)
                  .join(' · ') || 'no shared shortlist hints saved';

              return (
                <div
                  key={preset.id}
                  className={`rounded-lg border p-3 ${
                    isActive
                      ? 'border-cyan-500/40 bg-cyan-500/5'
                      : 'border-gray-800 bg-[var(--bg-base)]/40'
                  }`}
                >
                  <div className="flex flex-col lg:flex-row lg:items-start lg:justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <h4 className="text-sm font-medium text-gray-200">{preset.name}</h4>
                        {isActive && (
                          <span className="text-[11px] uppercase tracking-wide text-cyan-300">
                            active
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-gray-500 mt-1">
                        {formatBenchmarkCount(preset.runIds.length)} runs · baseline {presetBaseline.slice(0, 8)} · saved {new Date(preset.createdAt).toLocaleString()}
                      </p>
                      <p className="text-xs text-gray-600 mt-2">{filterSummary}</p>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <button
                        onClick={() => applyPreset(preset)}
                        className="px-3 py-1.5 rounded border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
                      >
                        Apply
                      </button>
                      <button
                        onClick={() => removePreset(preset.id)}
                        className="px-3 py-1.5 rounded border border-gray-700 text-xs text-gray-400 hover:border-red-500 hover:text-red-300 transition-colors"
                      >
                        Delete
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-6">
        {report.runs.map((run) => (
          <div
            key={run.run_id}
            className={`border rounded-lg p-4 ${run.run_id === report.baseline_run_id ? 'border-cyan-500/60 bg-cyan-500/5' : 'border-gray-800'}`}
          >
            <div className="flex items-center justify-between mb-2">
              <span className="text-xs font-mono text-cyan-400">{run.run_id.slice(0, 8)}</span>
              <span className={`text-xs ${run.publication_ready ? 'text-green-400' : 'text-yellow-400'}`}>
                {run.publication_ready ? 'ready' : run.sufficiency}
              </span>
            </div>
            <p className="text-sm text-gray-200">{run.target_host}</p>
            <p className="text-xs text-gray-500 mt-1">
              {run.scenario} / {run.primary_phase}
            </p>
            <p className="text-xs text-gray-600 mt-1 truncate" title={run.phase_model}>
              {run.phase_model}
            </p>
            <p className="text-xs text-gray-600 mt-2">
              noise {run.noise_level} · warnings {formatBenchmarkCount(run.warning_count)}
            </p>
            <p className="text-xs text-gray-600 mt-1">
              {formatRunEnvironmentSummary(run)}
            </p>
          </div>
        ))}
      </div>

      <div className="table-container">
        <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
          uncertainty-aware case comparison
        </h3>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-gray-800 text-gray-500">
                <th className="px-4 py-2 text-left">Case</th>
                <th className="px-4 py-2 text-left">Candidate</th>
                <th className="px-4 py-2 text-right">Baseline p50</th>
                <th className="px-4 py-2 text-right">Baseline CI</th>
                <th className="px-4 py-2 text-right">Candidate p50</th>
                <th className="px-4 py-2 text-right">Candidate CI</th>
                <th className="px-4 py-2 text-left">Comparability</th>
                <th className="px-4 py-2 text-right">Delta</th>
                <th className="px-4 py-2 text-right">Ratio</th>
                <th className="px-4 py-2 text-right">Verdict</th>
              </tr>
            </thead>
            <tbody>
              {report.cases.flatMap((caseComparison) =>
                caseComparison.candidates.map((candidate) => (
                  <tr
                    key={`${caseComparison.case_id}:${candidate.run.run_id}`}
                    className="border-b border-gray-800/50"
                  >
                    <td className="px-4 py-3 text-gray-300">
                      {formatBenchmarkCaseLabel(caseComparison)}
                      <div className="text-gray-600 mt-1">{caseComparison.metric_name}</div>
                    </td>
                    <td className="px-4 py-3 text-gray-300">
                      {candidate.run.target_host}
                      <div className="text-gray-600 mt-1 font-mono">{candidate.run.run_id.slice(0, 8)}</div>
                    </td>
                    <td className="px-4 py-3 text-right text-gray-300">
                      {formatBenchmarkMetric(caseComparison.baseline.distribution.median, caseComparison.metric_unit)}
                    </td>
                    <td className="px-4 py-3 text-right text-gray-500">
                      {formatBenchmarkInterval(
                        caseComparison.baseline.distribution.ci95_lower,
                        caseComparison.baseline.distribution.ci95_upper,
                        caseComparison.metric_unit,
                      )}
                    </td>
                    <td className="px-4 py-3 text-right text-gray-300">
                      {formatBenchmarkMetric(candidate.run.distribution.median, caseComparison.metric_unit)}
                    </td>
                    <td className="px-4 py-3 text-right text-gray-500">
                      {formatBenchmarkInterval(
                        candidate.run.distribution.ci95_lower,
                        candidate.run.distribution.ci95_upper,
                        caseComparison.metric_unit,
                      )}
                    </td>
                    <td className="px-4 py-3 text-left">
                      <div className={candidate.comparable ? 'text-green-400' : 'text-yellow-300'}>
                        {candidate.comparable ? 'comparable' : 'gated'}
                      </div>
                      {!candidate.comparable && candidate.comparability_notes.length > 0 && (
                        <div className="text-gray-600 mt-1 max-w-xs">
                          {candidate.comparability_notes.join(' · ')}
                        </div>
                      )}
                    </td>
                    <td className={`px-4 py-3 text-right ${candidate.verdict === 'better' ? 'text-green-400' : candidate.verdict === 'worse' ? 'text-red-400' : 'text-yellow-400'}`}>
                      {formatBenchmarkDelta(candidate.percent_delta)}
                    </td>
                    <td className="px-4 py-3 text-right text-gray-300">
                      {formatBenchmarkRatio(candidate.ratio)}
                    </td>
                    <td className={`px-4 py-3 text-right ${candidate.verdict === 'better' ? 'text-green-400' : candidate.verdict === 'worse' ? 'text-red-400' : 'text-yellow-400'}`}>
                      {candidate.verdict}
                    </td>
                  </tr>
                )),
              )}
            </tbody>
          </table>
        </div>

        {report.cases.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">No comparable benchmark cases were found across the selected runs.</p>
          </div>
        )}
      </div>
    </div>
  );
}
