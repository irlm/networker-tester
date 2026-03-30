import { useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkRunSummary } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { formatBenchmarkCount, formatBenchmarkMetric } from '../lib/benchmark';
import {
  deleteBenchmarkComparePreset,
  loadBenchmarkComparePresets,
  saveBenchmarkComparePreset,
  type BenchmarkComparePreset,
  type BenchmarkRunFilterPreset,
} from '../lib/benchmarkPresets';

function joinBenchmarkDetails(parts: Array<string | null | undefined>): string {
  return parts.filter(Boolean).join(' · ');
}

function formatBenchmarkRunEnvironment(run: BenchmarkRunSummary): string {
  return (
    joinBenchmarkDetails([
      run.server_region,
      run.network_type,
      run.baseline_rtt_p50_ms != null
        ? `RTT p50 ${formatBenchmarkMetric(run.baseline_rtt_p50_ms, 'ms')}`
        : null,
    ]) || 'environment fingerprint unavailable'
  );
}

function formatBenchmarkRunMethod(run: BenchmarkRunSummary): string {
  return (
    joinBenchmarkDetails([
      run.execution_plan_source ? `${run.execution_plan_source} plan` : null,
      run.phase_model || null,
    ]) || 'methodology summary unavailable'
  );
}

function summarizePresetFilters(filters?: BenchmarkRunFilterPreset): string {
  if (!filters) return 'no saved apples-to-apples filters';

  return (
    joinBenchmarkDetails([
      filters.targetSearch ? `host ${filters.targetSearch}` : null,
      filters.scenario ? `scenario ${filters.scenario}` : null,
      filters.phaseModel ? `phase ${filters.phaseModel}` : null,
      filters.serverRegion ? `region ${filters.serverRegion}` : null,
      filters.networkType ? `network ${filters.networkType}` : null,
    ]) || 'no saved apples-to-apples filters'
  );
}

function collectBenchmarkOptions(
  benchmarks: BenchmarkRunSummary[],
  getValue: (benchmark: BenchmarkRunSummary) => string | null,
): string[] {
  return Array.from(
    new Set(
      benchmarks
        .map(getValue)
        .filter((value): value is string => Boolean(value && value.trim())),
    ),
  ).sort((left, right) => left.localeCompare(right));
}

export function BenchmarksPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const [benchmarks, setBenchmarks] = useState<BenchmarkRunSummary[]>([]);
  const [selectedRunIds, setSelectedRunIds] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [targetSearch, setTargetSearch] = useState('');
  const [scenarioFilter, setScenarioFilter] = useState('');
  const [phaseModelFilter, setPhaseModelFilter] = useState('');
  const [serverRegionFilter, setServerRegionFilter] = useState('');
  const [networkTypeFilter, setNetworkTypeFilter] = useState('');
  const [presets, setPresets] = useState<BenchmarkComparePreset[]>([]);
  const [presetName, setPresetName] = useState('');
  const [presetStatus, setPresetStatus] = useState<string | null>(null);

  usePageTitle('Benchmarks');

  const loadBenchmarks = useCallback(() => {
    if (!projectId) return;
    const params: { target_host?: string; limit?: number } = { limit: 50 };
    if (targetSearch.trim()) params.target_host = targetSearch.trim();

    api
      .getBenchmarks(projectId, params)
      .then((data) => {
        setBenchmarks(data);
        setSelectedRunIds((current) =>
          current.filter((runId) => data.some((benchmark) => benchmark.run_id === runId)),
        );
        setError(null);
        setLoading(false);
      })
      .catch((err) => {
        setError(String(err));
        setLoading(false);
      });
  }, [projectId, targetSearch]);

  usePolling(loadBenchmarks, 15000);

  const toggleRun = (runId: string) => {
    setSelectedRunIds((current) =>
      current.includes(runId)
        ? current.filter((id) => id !== runId)
        : [...current, runId].slice(-4),
    );
  };

  const compareSelected = () => {
    if (selectedRunIds.length < 2) return;
    const params = new URLSearchParams({ runs: selectedRunIds.join(',') });
    navigate(`/projects/${projectId}/benchmarks/compare?${params.toString()}`);
  };

  const selectedBenchmarks = useMemo(
    () => benchmarks.filter((benchmark) => selectedRunIds.includes(benchmark.run_id)),
    [benchmarks, selectedRunIds],
  );

  const filteredBenchmarks = useMemo(
    () =>
      benchmarks.filter((benchmark) => {
        if (scenarioFilter && benchmark.scenario !== scenarioFilter) return false;
        if (phaseModelFilter && benchmark.phase_model !== phaseModelFilter) return false;
        if (serverRegionFilter && benchmark.server_region !== serverRegionFilter) return false;
        if (networkTypeFilter && benchmark.network_type !== networkTypeFilter) return false;
        return true;
      }),
    [
      benchmarks,
      networkTypeFilter,
      phaseModelFilter,
      scenarioFilter,
      serverRegionFilter,
    ],
  );

  const filterOptions = useMemo(
    () => ({
      scenarios: collectBenchmarkOptions(benchmarks, (benchmark) => benchmark.scenario),
      phaseModels: collectBenchmarkOptions(benchmarks, (benchmark) => benchmark.phase_model),
      serverRegions: collectBenchmarkOptions(benchmarks, (benchmark) => benchmark.server_region),
      networkTypes: collectBenchmarkOptions(benchmarks, (benchmark) => benchmark.network_type),
    }),
    [benchmarks],
  );

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

  const visibleSelectedCount = useMemo(
    () =>
      filteredBenchmarks.reduce(
        (count, benchmark) => count + (selectedRunIds.includes(benchmark.run_id) ? 1 : 0),
        0,
      ),
    [filteredBenchmarks, selectedRunIds],
  );

  const hiddenSelectedCount = selectedRunIds.length - visibleSelectedCount;
  const hasActiveFilters = Boolean(
    targetSearch ||
      scenarioFilter ||
      phaseModelFilter ||
      serverRegionFilter ||
      networkTypeFilter,
  );

  const compareSelectionSummary = useMemo(() => {
    if (selectedBenchmarks.length < 2) return null;

    const hosts = new Set(selectedBenchmarks.map((benchmark) => benchmark.target_host));
    const scenarios = new Set(selectedBenchmarks.map((benchmark) => benchmark.scenario));
    const phaseModels = new Set(
      selectedBenchmarks.map((benchmark) => benchmark.phase_model).filter(Boolean),
    );
    const regions = new Set(
      selectedBenchmarks.map((benchmark) => benchmark.server_region).filter(Boolean),
    );
    const networkTypes = new Set(
      selectedBenchmarks.map((benchmark) => benchmark.network_type).filter(Boolean),
    );
    const issues = [
      hosts.size > 1 ? `${hosts.size} targets` : null,
      scenarios.size > 1 ? `${scenarios.size} scenarios` : null,
      phaseModels.size > 1 ? `${phaseModels.size} phase models` : null,
      regions.size > 1 ? `${regions.size} server regions` : null,
      networkTypes.size > 1 ? `${networkTypes.size} network types` : null,
    ].filter(Boolean) as string[];

    return {
      likelyComparable: issues.length === 0,
      issues,
      warnings: selectedBenchmarks.reduce(
        (count, benchmark) => count + benchmark.warnings.length,
        0,
      ),
      publicationBlockers: selectedBenchmarks.reduce(
        (count, benchmark) => count + benchmark.publication_blocker_count,
        0,
      ),
    };
  }, [selectedBenchmarks]);

  const applyPreset = (preset: BenchmarkComparePreset) => {
    setTargetSearch(preset.filters?.targetSearch ?? '');
    setScenarioFilter(preset.filters?.scenario ?? '');
    setPhaseModelFilter(preset.filters?.phaseModel ?? '');
    setServerRegionFilter(preset.filters?.serverRegion ?? '');
    setNetworkTypeFilter(preset.filters?.networkType ?? '');
    setSelectedRunIds(preset.runIds);
    setPresetName(preset.name);
    setPresetStatus(`Loaded preset ${preset.name}.`);
  };

  const openPresetCompare = (preset: BenchmarkComparePreset) => {
    const params = new URLSearchParams({ runs: preset.runIds.join(',') });
    if (preset.baselineRunId) params.set('baseline', preset.baselineRunId);
    navigate(`/projects/${projectId}/benchmarks/compare?${params.toString()}`);
  };

  const saveCurrentPreset = async () => {
    if (!projectId || selectedRunIds.length < 2) return;

    const defaultName =
      presetName.trim() ||
      joinBenchmarkDetails([
        selectedBenchmarks[0]?.scenario ?? 'selection',
        selectedBenchmarks[0]?.phase_model ?? 'compare',
        new Date().toLocaleDateString(),
      ]);

    const nextPresets = await saveBenchmarkComparePreset(projectId, {
      name: defaultName,
      runIds: selectedRunIds,
      baselineRunId: selectedRunIds[0] ?? null,
      filters: {
        targetSearch,
        scenario: scenarioFilter,
        phaseModel: phaseModelFilter,
        serverRegion: serverRegionFilter,
        networkType: networkTypeFilter,
      },
    });

    setPresets(nextPresets);
    setPresetName('');
    setPresetStatus(`Saved preset ${defaultName}.`);
  };

  const removePreset = async (presetId: string) => {
    if (!projectId) return;
    const nextPresets = await deleteBenchmarkComparePreset(projectId, presetId);
    setPresets(nextPresets);
  };

  const resetFilters = () => {
    setTargetSearch('');
    setScenarioFilter('');
    setPhaseModelFilter('');
    setServerRegionFilter('');
    setNetworkTypeFilter('');
    setPresetStatus(null);
  };

  if (loading && benchmarks.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Benchmark Runs</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading benchmark runs...</div>
      </div>
    );
  }

  if (error && benchmarks.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Benchmark Runs</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load benchmark runs</h3>
          <p className="text-red-300 text-sm">
            Could not fetch benchmark runs. Check your connection and try refreshing.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex flex-col md:flex-row md:items-center md:justify-between gap-3 mb-4 md:mb-6">
        <div>
          <h2 className="text-lg md:text-xl font-bold text-gray-100">Benchmark Runs</h2>
          <p className="text-xs text-gray-500 mt-1">
            Select two to four runs. Keep target, scenario, phase model, and network fingerprint aligned for the cleanest compare result.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <label htmlFor="benchmarks-target-search" className="sr-only">
            Search by target host
          </label>
          <input
            id="benchmarks-target-search"
            type="search"
            value={targetSearch}
            onChange={(event) => setTargetSearch(event.target.value)}
            placeholder="Filter by host..."
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-40 md:w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
          <button
            onClick={compareSelected}
            disabled={selectedRunIds.length < 2}
            className="px-3 py-1.5 rounded border border-gray-700 text-sm text-gray-200 disabled:text-gray-600 disabled:border-gray-800 disabled:cursor-not-allowed hover:border-cyan-500 transition-colors"
          >
            Compare
          </button>
        </div>
      </div>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh benchmark runs. Retrying automatically.
        </div>
      )}

      <div className="border border-gray-800 rounded-lg p-4 mb-4 bg-[var(--bg-surface)]/40">
        <div className="flex flex-col lg:flex-row lg:items-end lg:justify-between gap-4">
          <div>
            <h3 className="text-sm font-semibold text-gray-200">Apples-to-Apples Filters</h3>
            <p className="text-xs text-gray-500 mt-1">
              Narrow the shortlist to consistent scenario, phase model, region, and network traits before you build a compare set.
            </p>
          </div>
          <div className="text-xs text-gray-500">
            showing {formatBenchmarkCount(filteredBenchmarks.length)} of {formatBenchmarkCount(benchmarks.length)} loaded runs
          </div>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-3 mt-4">
          <label className="text-xs text-gray-500">
            Scenario
            <select
              value={scenarioFilter}
              onChange={(event) => setScenarioFilter(event.target.value)}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
            >
              <option value="">All scenarios</option>
              {filterOptions.scenarios.map((scenario) => (
                <option key={scenario} value={scenario}>
                  {scenario}
                </option>
              ))}
            </select>
          </label>

          <label className="text-xs text-gray-500">
            Phase model
            <select
              value={phaseModelFilter}
              onChange={(event) => setPhaseModelFilter(event.target.value)}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
            >
              <option value="">All phase models</option>
              {filterOptions.phaseModels.map((phaseModel) => (
                <option key={phaseModel} value={phaseModel}>
                  {phaseModel}
                </option>
              ))}
            </select>
          </label>

          <label className="text-xs text-gray-500">
            Server region
            <select
              value={serverRegionFilter}
              onChange={(event) => setServerRegionFilter(event.target.value)}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
            >
              <option value="">All server regions</option>
              {filterOptions.serverRegions.map((serverRegion) => (
                <option key={serverRegion} value={serverRegion}>
                  {serverRegion}
                </option>
              ))}
            </select>
          </label>

          <label className="text-xs text-gray-500">
            Network type
            <select
              value={networkTypeFilter}
              onChange={(event) => setNetworkTypeFilter(event.target.value)}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
            >
              <option value="">All network types</option>
              {filterOptions.networkTypes.map((networkType) => (
                <option key={networkType} value={networkType}>
                  {networkType}
                </option>
              ))}
            </select>
          </label>
        </div>

        <div className="flex flex-col md:flex-row md:items-center md:justify-between gap-2 mt-4 text-xs">
          <div className="text-gray-500">
            {hiddenSelectedCount > 0
              ? `${formatBenchmarkCount(hiddenSelectedCount)} selected runs are hidden by the current local filters but still included in compare.`
              : 'Local filters only change the visible shortlist. Saved compare selections remain intact.'}
          </div>
          <button
            onClick={resetFilters}
            disabled={!hasActiveFilters}
            className="self-start md:self-auto px-3 py-1.5 rounded border border-gray-700 text-gray-300 disabled:text-gray-600 disabled:border-gray-800 disabled:cursor-not-allowed hover:border-cyan-500 transition-colors"
          >
            Reset filters
          </button>
        </div>
      </div>

      <div className="border border-gray-800 rounded-lg p-4 mb-4 bg-[var(--bg-surface)]/40">
        <div className="flex flex-col xl:flex-row xl:items-end xl:justify-between gap-4">
          <div>
            <h3 className="text-sm font-semibold text-gray-200">Saved Compare Presets</h3>
            <p className="text-xs text-gray-500 mt-1">
              Save the current two-to-four-run selection together with the active shortlist filters so you can reopen the same comparison quickly.
            </p>
          </div>
          <div className="flex flex-col sm:flex-row gap-2 xl:min-w-[24rem]">
            <label htmlFor="benchmark-preset-name" className="sr-only">
              Preset name
            </label>
            <input
              id="benchmark-preset-name"
              type="text"
              value={presetName}
              onChange={(event) => setPresetName(event.target.value)}
              placeholder="Preset name..."
              className="flex-1 bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-300 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
            <button
              onClick={saveCurrentPreset}
              disabled={selectedRunIds.length < 2}
              className="px-3 py-2 rounded border border-gray-700 text-sm text-gray-200 disabled:text-gray-600 disabled:border-gray-800 disabled:cursor-not-allowed hover:border-cyan-500 transition-colors"
            >
              Save current selection
            </button>
          </div>
        </div>

        {presetStatus && (
          <p className="text-xs text-cyan-300/80 mt-3">{presetStatus}</p>
        )}

        {presets.length === 0 ? (
          <div className="mt-4 rounded border border-dashed border-gray-800 p-4 text-xs text-gray-500">
            No saved presets yet. Build a compare-ready selection, then store it here.
          </div>
        ) : (
          <div className="space-y-2 mt-4">
            {presets.map((preset) => {
              const presetBaseline = preset.baselineRunId ?? preset.runIds[0];
              const activePreset =
                preset.runIds.join(',') === selectedRunIds.join(',') &&
                presetBaseline === (selectedRunIds[0] ?? null);

              return (
                <div
                  key={preset.id}
                  className={`rounded-lg border p-3 ${
                    activePreset
                      ? 'border-cyan-500/40 bg-cyan-500/5'
                      : 'border-gray-800 bg-[var(--bg-base)]/40'
                  }`}
                >
                  <div className="flex flex-col lg:flex-row lg:items-start lg:justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <h4 className="text-sm font-medium text-gray-200">{preset.name}</h4>
                        {activePreset && (
                          <span className="text-[11px] uppercase tracking-wide text-cyan-300">
                            active
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-gray-500 mt-1">
                        {formatBenchmarkCount(preset.runIds.length)} runs · baseline {presetBaseline.slice(0, 8)} · saved {new Date(preset.createdAt).toLocaleString()}
                      </p>
                      <p className="text-xs text-gray-600 mt-2">
                        {summarizePresetFilters(preset.filters)}
                      </p>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <button
                        onClick={() => applyPreset(preset)}
                        className="px-3 py-1.5 rounded border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
                      >
                        Apply
                      </button>
                      <button
                        onClick={() => openPresetCompare(preset)}
                        className="px-3 py-1.5 rounded border border-cyan-500/50 text-xs text-cyan-200 hover:border-cyan-400 transition-colors"
                      >
                        Open compare
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

      {compareSelectionSummary && (
        <div
          className={`border rounded-lg p-4 mb-4 ${
            compareSelectionSummary.likelyComparable
              ? 'border-cyan-500/30 bg-cyan-500/5'
              : 'border-yellow-500/30 bg-yellow-500/10'
          }`}
        >
          <p
            className={`text-sm ${
              compareSelectionSummary.likelyComparable
                ? 'text-cyan-100'
                : 'text-yellow-200'
            }`}
          >
            {compareSelectionSummary.likelyComparable
              ? `Selection looks compare-ready across ${formatBenchmarkCount(selectedBenchmarks.length)} runs.`
              : `Selection will likely gate some comparisons because it spans ${compareSelectionSummary.issues.join(', ')}.`}
          </p>
          <p
            className={`text-xs mt-1 ${
              compareSelectionSummary.likelyComparable
                ? 'text-cyan-300/80'
                : 'text-yellow-300/80'
            }`}
          >
            warnings {formatBenchmarkCount(compareSelectionSummary.warnings)} · publication blockers {formatBenchmarkCount(compareSelectionSummary.publicationBlockers)}
          </p>
        </div>
      )}

      <div className="hidden md:block table-container">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium w-10">Pick</th>
              <th className="px-4 py-2.5 text-left font-medium">Run ID</th>
              <th className="px-4 py-2.5 text-left font-medium">Target</th>
              <th className="px-4 py-2.5 text-left font-medium">Scenario</th>
              <th className="px-4 py-2.5 text-left font-medium">Cases</th>
              <th className="px-4 py-2.5 text-left font-medium">Samples</th>
              <th className="px-4 py-2.5 text-left font-medium">Quality</th>
              <th className="px-4 py-2.5 text-left font-medium">Generated</th>
            </tr>
          </thead>
          <tbody>
            {filteredBenchmarks.map((benchmark) => {
              const selected = selectedRunIds.includes(benchmark.run_id);
              return (
                <tr key={benchmark.run_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                  <td className="px-4 py-3">
                    <input
                      type="checkbox"
                      checked={selected}
                      onChange={() => toggleRun(benchmark.run_id)}
                      className="accent-cyan-400"
                      aria-label={`Select benchmark ${benchmark.run_id}`}
                    />
                  </td>
                  <td className="px-4 py-3">
                    <Link
                      to={`/projects/${projectId}/benchmarks/${benchmark.run_id}`}
                      className="text-cyan-400 hover:underline font-mono text-xs"
                    >
                      {benchmark.run_id.slice(0, 8)}
                    </Link>
                  </td>
                  <td className="px-4 py-3 text-gray-300 text-xs">
                    <div className="truncate max-w-48">{benchmark.target_host}</div>
                    <div
                      className="text-gray-600 mt-1 truncate max-w-56"
                      title={formatBenchmarkRunEnvironment(benchmark)}
                    >
                      {formatBenchmarkRunEnvironment(benchmark)}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs">
                    <div>{benchmark.scenario} / {benchmark.primary_phase}</div>
                    <div
                      className="text-gray-600 mt-1 truncate max-w-56"
                      title={formatBenchmarkRunMethod(benchmark)}
                    >
                      {formatBenchmarkRunMethod(benchmark)}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-300">
                    {formatBenchmarkCount(benchmark.total_cases)}
                  </td>
                  <td className="px-4 py-3 text-gray-300">
                    {formatBenchmarkCount(benchmark.total_samples)}
                  </td>
                  <td className="px-4 py-3 text-xs">
                    <div className={benchmark.publication_ready ? 'text-green-400' : 'text-yellow-400'}>
                      {benchmark.publication_ready ? 'ready' : benchmark.sufficiency}
                    </div>
                    <div className="text-gray-600 mt-1">
                      warnings {formatBenchmarkCount(benchmark.warnings.length)} · blockers {formatBenchmarkCount(benchmark.publication_blocker_count)}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs">
                    {new Date(benchmark.generated_at).toLocaleString()}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>

        {filteredBenchmarks.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">
              {benchmarks.length === 0 ? 'No benchmark runs yet' : 'No runs match the current filters'}
            </p>
            <p className="text-gray-700 text-xs mt-1">
              {benchmarks.length === 0
                ? 'Benchmark runs appear here after benchmark-mode tests are persisted.'
                : 'Reset one or more shortlist filters to widen the compare candidate pool.'}
            </p>
          </div>
        )}
      </div>

      <div className="md:hidden space-y-2">
        {filteredBenchmarks.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">
              {benchmarks.length === 0 ? 'No benchmark runs yet' : 'No runs match the current filters'}
            </p>
            <p className="text-gray-700 text-xs mt-1">
              {benchmarks.length === 0
                ? 'Benchmark runs appear here after benchmark-mode tests are persisted.'
                : 'Reset one or more shortlist filters to widen the compare candidate pool.'}
            </p>
          </div>
        ) : (
          filteredBenchmarks.map((benchmark) => (
            <div key={benchmark.run_id} className="border border-gray-800 rounded p-3">
              <div className="flex items-center justify-between gap-3 mb-2">
                <Link
                  to={`/projects/${projectId}/benchmarks/${benchmark.run_id}`}
                  className="text-cyan-400 hover:underline font-mono text-xs"
                >
                  {benchmark.run_id.slice(0, 8)}
                </Link>
                <input
                  type="checkbox"
                  checked={selectedRunIds.includes(benchmark.run_id)}
                  onChange={() => toggleRun(benchmark.run_id)}
                  className="accent-cyan-400"
                  aria-label={`Select benchmark ${benchmark.run_id}`}
                />
              </div>
              <p className="text-gray-300 text-xs truncate mb-1">{benchmark.target_host}</p>
              <p
                className="text-gray-600 text-[11px] truncate mb-2"
                title={formatBenchmarkRunEnvironment(benchmark)}
              >
                {formatBenchmarkRunEnvironment(benchmark)}
              </p>
              <div className="flex items-center gap-3 text-xs text-gray-500">
                <span>{benchmark.scenario}</span>
                <span>{formatBenchmarkCount(benchmark.total_cases)} cases</span>
                <span>{benchmark.publication_ready ? 'ready' : benchmark.sufficiency}</span>
              </div>
              <p
                className="text-gray-600 text-[11px] mt-2 truncate"
                title={formatBenchmarkRunMethod(benchmark)}
              >
                {formatBenchmarkRunMethod(benchmark)}
              </p>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
