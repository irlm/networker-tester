import { useState, useCallback } from 'react';
import { useParams } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkArtifact } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';
import {
  formatBenchmarkCaseLabel,
  formatBenchmarkCount,
  formatBenchmarkMetric,
  formatBenchmarkNumber,
} from '../lib/benchmark';
import type { BenchmarkHostInfo } from '../api/types';

function joinBenchmarkDetails(parts: Array<string | null | undefined>): string {
  return parts.filter(Boolean).join(' · ');
}

function formatBenchmarkPercent(value: number | null | undefined): string {
  if (!Number.isFinite(value)) return '-';
  return `${formatBenchmarkNumber(value as number)}%`;
}

function formatHostFingerprint(info: BenchmarkHostInfo | null | undefined): string {
  if (!info) return 'unavailable';
  return (
    joinBenchmarkDetails([
      info.os,
      info.arch,
      `${formatBenchmarkCount(info.cpu_cores)} cores`,
      info.region ?? null,
    ]) || 'unavailable'
  );
}

export function BenchmarkDetailPage() {
  const { projectId } = useProject();
  const { runId } = useParams<{ runId: string }>();
  const [artifact, setArtifact] = useState<BenchmarkArtifact | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const shortId = runId?.slice(0, 8) ?? '';
  usePageTitle(runId ? `Benchmark ${shortId}` : 'Benchmark');

  const loadBenchmark = useCallback(() => {
    if (!projectId || !runId) return;
    api
      .getBenchmark(projectId, runId)
      .then((data) => {
        setArtifact(data);
        setError(null);
        setLoading(false);
      })
      .catch((err) => {
        setError(String(err));
        setLoading(false);
      });
  }, [projectId, runId]);

  usePolling(loadBenchmark, 30000, !!runId);

  if (loading && !artifact) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: `Run ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading benchmark run {shortId}...</div>
      </div>
    );
  }

  if (error && !artifact) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: `Run ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load benchmark run</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  if (!artifact) return null;

  const publicationBlockers = artifact.data_quality.publication_blockers ?? [];
  const executionPlan = artifact.methodology.execution_plan;
  const noiseThresholds = artifact.methodology.noise_thresholds;
  const networkBaseline = artifact.environment.network_baseline;
  const environmentCheck = artifact.environment.environment_check;
  const stabilityCheck = artifact.environment.stability_check;

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Benchmarks', to: `/projects/${projectId}/benchmarks` }, { label: `Run ${shortId}` }]} />

      <div className="mb-6">
        <h2 className="text-xl font-bold text-gray-100 mb-1">Benchmark {shortId}</h2>
        <p className="text-sm text-gray-500">
          Target <span className="text-gray-300">{artifact.metadata.target_host}</span> · Scenario{' '}
          <span className="text-gray-300">{artifact.methodology.scenario}</span> · Phase{' '}
          <span className="text-gray-300">{artifact.methodology.sample_phase}</span>
        </p>
      </div>

      {!artifact.data_quality.publication_ready && (
        <div className="border border-yellow-500/30 bg-yellow-500/10 rounded-lg p-4 mb-6">
          <h3 className="text-yellow-200 text-sm font-medium mb-1">Needs another run before publication</h3>
          <p className="text-yellow-100/80 text-sm">
            This benchmark still has quality blockers. Treat the deltas as directional until the blockers below are cleared.
          </p>
          {publicationBlockers.length > 0 && (
            <div className="mt-3">
              <p className="text-xs uppercase tracking-wider text-yellow-300/80 mb-2">publication blockers</p>
              <ul className="space-y-1 text-sm text-yellow-100">
                {publicationBlockers.map((blocker) => (
                  <li key={blocker}>{blocker}</li>
                ))}
              </ul>
            </div>
          )}
          {artifact.data_quality.warnings.length > 0 && (
            <div className="mt-3">
              <p className="text-xs uppercase tracking-wider text-yellow-300/80 mb-2">warnings</p>
              <ul className="space-y-1 text-sm text-yellow-100/80">
                {artifact.data_quality.warnings.map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-x-5 gap-y-1 py-3 mb-6 text-xs border-b border-gray-800/50">
        <span className="text-gray-500">
          Cases <span className="text-gray-200 font-mono font-semibold ml-1">{formatBenchmarkCount(artifact.cases.length)}</span>
        </span>
        <span className="text-gray-500">
          Samples <span className="text-gray-200 font-mono font-semibold ml-1">{formatBenchmarkCount(artifact.samples.length)}</span>
        </span>
        <span className="text-gray-500">
          Concurrency <span className="text-gray-200 font-mono font-semibold ml-1">{artifact.metadata.concurrency}</span>
        </span>
        <span className="text-gray-500">
          Publication{' '}
          <span className={`font-mono font-semibold ml-1 ${artifact.data_quality.publication_ready ? 'text-green-400' : 'text-yellow-400'}`}>
            {artifact.data_quality.publication_ready ? 'ready' : 'not ready'}
          </span>
        </span>
        <span className="text-gray-500">
          Noise <span className="text-gray-200 font-mono font-semibold ml-1">{artifact.data_quality.noise_level}</span>
        </span>
        <span className="text-gray-500">
          Quality tier <span className="text-gray-200 font-mono font-semibold ml-1">{artifact.data_quality.quality_tier ?? 'unknown'}</span>
        </span>
      </div>

      <div className="grid grid-cols-1 xl:grid-cols-2 gap-6 mb-6">
        <div className="border border-gray-800 rounded-lg p-4">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3">methodology</h3>
          <div className="space-y-2 text-sm">
            <p className="text-gray-400">Contract <span className="text-gray-200">{artifact.metadata.contract_version}</span></p>
            <p className="text-gray-400">Modes <span className="text-gray-200">{artifact.metadata.modes.join(', ') || 'not recorded'}</span></p>
            <p className="text-gray-400">Phase model <span className="text-gray-200">{artifact.methodology.phase_model}</span></p>
            <p className="text-gray-400">Measured phase <span className="text-gray-200">{artifact.methodology.sample_phase}</span></p>
            <p className="text-gray-400">Scenario <span className="text-gray-200">{artifact.methodology.scenario}</span></p>
            <p className="text-gray-400">Launches <span className="text-gray-200">{artifact.methodology.launch_count}</span></p>
            <p className="text-gray-400">Observed phases <span className="text-gray-200">{artifact.methodology.phases_present.join(', ') || 'not recorded'}</span></p>
            <p className="text-gray-400">Retries recorded <span className="text-gray-200">{artifact.methodology.retries_recorded ? 'yes' : 'no'}</span></p>
            <p className="text-gray-400">Confidence <span className="text-gray-200">{artifact.methodology.confidence_level != null ? formatBenchmarkPercent(artifact.methodology.confidence_level * 100) : 'not recorded'}</span></p>
          </div>
        </div>

        <div className="border border-gray-800 rounded-lg p-4">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3">execution controls</h3>
          <div className="space-y-2 text-sm">
            <p className="text-gray-400">Plan source <span className="text-gray-200">{executionPlan?.source ?? 'fixed run count'}</span></p>
            <p className="text-gray-400">Sample budget <span className="text-gray-200">{executionPlan ? `${formatBenchmarkCount(executionPlan.min_samples)} min / ${formatBenchmarkCount(executionPlan.max_samples)} max` : 'not recorded'}</span></p>
            <p className="text-gray-400">Min measured duration <span className="text-gray-200">{executionPlan ? formatBenchmarkMetric(executionPlan.min_duration_ms, 'ms') : 'not recorded'}</span></p>
            <p className="text-gray-400">Target relative error <span className="text-gray-200">{executionPlan?.target_relative_error != null ? formatBenchmarkPercent(executionPlan.target_relative_error * 100) : 'not set'}</span></p>
            <p className="text-gray-400">Target absolute error <span className="text-gray-200">{executionPlan?.target_absolute_error != null ? formatBenchmarkMetric(executionPlan.target_absolute_error, artifact.summary.metric_unit) : 'not set'}</span></p>
            <p className="text-gray-400">Pilot sample count <span className="text-gray-200">{executionPlan ? formatBenchmarkCount(executionPlan.pilot_sample_count) : 'not recorded'}</span></p>
            <p className="text-gray-400">Pilot elapsed <span className="text-gray-200">{executionPlan?.pilot_elapsed_ms != null ? formatBenchmarkMetric(executionPlan.pilot_elapsed_ms, 'ms') : 'not recorded'}</span></p>
            <p className="text-gray-400">Noise thresholds <span className="text-gray-200">{noiseThresholds ? `loss ≤ ${formatBenchmarkPercent(noiseThresholds.max_packet_loss_percent)} · jitter ratio ≤ ${formatBenchmarkNumber(noiseThresholds.max_jitter_ratio)} · RTT spread ≤ ${formatBenchmarkNumber(noiseThresholds.max_rtt_spread_ratio)}x` : 'defaults / not recorded'}</span></p>
          </div>
        </div>

        <div className="border border-gray-800 rounded-lg p-4">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3">environment fingerprint</h3>
          <div className="space-y-2 text-sm">
            <p className="text-gray-400">Client <span className="text-gray-200">{formatHostFingerprint(artifact.environment.client_info)}</span></p>
            <p className="text-gray-400">Server <span className="text-gray-200">{formatHostFingerprint(artifact.environment.server_info)}</span></p>
            <p className="text-gray-400">Network baseline <span className="text-gray-200">{networkBaseline ? `${networkBaseline.network_type} · p50 ${formatBenchmarkMetric(networkBaseline.rtt_p50_ms, 'ms')} · p95 ${formatBenchmarkMetric(networkBaseline.rtt_p95_ms, 'ms')}` : 'not recorded'}</span></p>
            <p className="text-gray-400">Environment check <span className="text-gray-200">{environmentCheck ? `${formatBenchmarkCount(environmentCheck.successful_samples)}/${formatBenchmarkCount(environmentCheck.attempted_samples)} samples · loss ${formatBenchmarkPercent(environmentCheck.packet_loss_percent)} · p50 ${formatBenchmarkMetric(environmentCheck.rtt_p50_ms, 'ms')}` : 'not recorded'}</span></p>
            <p className="text-gray-400">Stability check <span className="text-gray-200">{stabilityCheck ? `${formatBenchmarkCount(stabilityCheck.successful_samples)}/${formatBenchmarkCount(stabilityCheck.attempted_samples)} samples · jitter ${formatBenchmarkMetric(stabilityCheck.jitter_ms, 'ms')} · loss ${formatBenchmarkPercent(stabilityCheck.packet_loss_percent)}` : 'not recorded'}</span></p>
            <p className="text-gray-400">Packet capture <span className="text-gray-200">{artifact.environment.packet_capture_enabled ? 'enabled' : 'disabled'}</span></p>
          </div>
        </div>

        <div className="border border-gray-800 rounded-lg p-4">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3">data quality</h3>
          <div className="space-y-2 text-sm">
            <p className="text-gray-400">Sufficiency <span className="text-gray-200">{artifact.data_quality.sufficiency}</span></p>
            <p className="text-gray-400">Noise level <span className="text-gray-200">{artifact.data_quality.noise_level}</span></p>
            <p className="text-gray-400">Relative margin of error <span className="text-gray-200">{artifact.data_quality.relative_margin_of_error != null ? formatBenchmarkPercent(artifact.data_quality.relative_margin_of_error * 100) : 'not recorded'}</span></p>
            <p className="text-gray-400">Sample stability CV <span className="text-gray-200">{formatBenchmarkNumber(artifact.data_quality.sample_stability_cv)}</span></p>
            <p className="text-gray-400">Outliers <span className="text-gray-200">{formatBenchmarkCount(artifact.data_quality.outlier_count ?? 0)} total ({formatBenchmarkCount(artifact.data_quality.low_outlier_count ?? 0)} low / {formatBenchmarkCount(artifact.data_quality.high_outlier_count ?? 0)} high)</span></p>
            <p className="text-gray-400">Raw attempts <span className="text-gray-200">{formatBenchmarkCount(artifact.diagnostics.raw_attempt_count)}</span></p>
            <p className="text-gray-400">Raw failures <span className="text-gray-200">{formatBenchmarkCount(artifact.diagnostics.raw_failure_count)}</span></p>
          </div>
        </div>
      </div>

      <div className="table-container mb-6">
        <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
          launch lifecycle
        </h3>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-gray-800 text-gray-500">
                <th className="px-4 py-2 text-left">Launch</th>
                <th className="px-4 py-2 text-left">Scenario</th>
                <th className="px-4 py-2 text-left">Phase</th>
                <th className="px-4 py-2 text-right">Samples</th>
                <th className="px-4 py-2 text-right">Primary</th>
                <th className="px-4 py-2 text-right">Warmup</th>
                <th className="px-4 py-2 text-right">Failures</th>
              </tr>
            </thead>
            <tbody>
              {artifact.launches.map((launch) => (
                <tr key={launch.launch_index} className="border-b border-gray-800/50">
                  <td className="px-4 py-3 text-gray-300">{launch.launch_index}</td>
                  <td className="px-4 py-3 text-gray-400">{launch.scenario}</td>
                  <td className="px-4 py-3 text-gray-400">{launch.primary_phase}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkCount(launch.sample_count)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkCount(launch.primary_sample_count)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkCount(launch.warmup_sample_count)}</td>
                  <td className="px-4 py-3 text-right text-red-400">{launch.failure_count > 0 ? formatBenchmarkCount(launch.failure_count) : '-'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      <div className="table-container">
        <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
          case summaries
        </h3>
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-gray-800 text-gray-500">
                <th className="px-4 py-2 text-left">Case</th>
                <th className="px-4 py-2 text-right">Included</th>
                <th className="px-4 py-2 text-right">Mean</th>
                <th className="px-4 py-2 text-right">p50</th>
                <th className="px-4 py-2 text-right">p95</th>
                <th className="px-4 py-2 text-right">p99</th>
                <th className="px-4 py-2 text-right">RPS</th>
                <th className="px-4 py-2 text-right">Failures</th>
              </tr>
            </thead>
            <tbody>
              {artifact.summaries.map((summary) => (
                <tr key={summary.case_id} className="border-b border-gray-800/50">
                  <td className="px-4 py-3 text-gray-300">
                    {formatBenchmarkCaseLabel(summary)}
                    <div className="text-gray-600 mt-1">{summary.metric_name}</div>
                  </td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkCount(summary.included_sample_count)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkMetric(summary.mean, summary.metric_unit)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkMetric(summary.p50, summary.metric_unit)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkMetric(summary.p95, summary.metric_unit)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkMetric(summary.p99, summary.metric_unit)}</td>
                  <td className="px-4 py-3 text-right text-gray-300">{formatBenchmarkNumber(summary.rps)}</td>
                  <td className="px-4 py-3 text-right text-red-400">{summary.failure_count > 0 ? formatBenchmarkCount(summary.failure_count) : '-'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
