import { useState, useCallback } from 'react';
import { Link } from 'react-router';
import { api } from '../api/client';
import type { BenchmarkRegressionSummary, BenchmarkRegressionWithConfig } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { DataTable } from '../components/common/DataTable';
import { timeAgo } from '../lib/format';

function severityColor(severity: string): string {
  switch (severity) {
    case 'critical': return 'text-red-400 bg-red-500/10';
    case 'warning': return 'text-yellow-400 bg-yellow-500/10';
    default: return 'text-gray-400 bg-gray-500/10';
  }
}

function metricLabel(metric: string): string {
  switch (metric) {
    case 'p50_latency_ms': return 'p50 Latency';
    case 'p50': return 'p50';
    case 'success_rate': return 'Success Rate';
    default: return metric;
  }
}

function formatValue(metric: string, unit: string, value: number): string {
  if (metric === 'success_rate') return `${value.toFixed(1)}%`;
  return `${value.toFixed(2)} ${unit}`;
}

// Every row in this table IS a regression, so the delta is always adverse —
// rendered red regardless of sign (latency deltas are positive, throughput
// and success-rate deltas negative).
function formatDelta(metric: string, delta: number): string {
  const signed = `${delta > 0 ? '+' : ''}${delta.toFixed(1)}`;
  return metric === 'success_rate' ? `${signed} pp` : `${signed}%`;
}

export function BenchmarkRegressionsPage() {
  const { projectId } = useProject();
  const [regressions, setRegressions] = useState<BenchmarkRegressionWithConfig[]>([]);
  const [summary, setSummary] = useState<BenchmarkRegressionSummary | null>(null);
  const [loading, setLoading] = useState(true);

  usePageTitle('Benchmark Regressions');

  const refresh = useCallback(() => {
    if (!projectId) return;
    // Both calls issued synchronously (usePolling request-source contract).
    api.listBenchmarkRegressions(projectId, 100)
      .then(r => { setRegressions(r); setLoading(false); })
      .catch(() => setLoading(false));
    // Summary is progressive enhancement — a failure keeps the page usable
    // with the generic empty-state copy.
    api.getBenchmarkRegressionSummary(projectId)
      .then(setSummary)
      .catch(() => setSummary(null));
  }, [projectId]);

  usePolling(refresh, 30000);

  if (loading && regressions.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Benchmark Regressions</h2>
        <div className="space-y-3">
          {[1, 2, 3].map(i => (
            <div key={i} className="border border-gray-800 rounded p-4">
              <div className="h-4 w-48 rounded bg-gray-800/60 motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  const criticalCount = regressions.filter(r => r.severity === 'critical').length;
  const warningCount = regressions.filter(r => r.severity === 'warning').length;

  // #810: distinguish "detection has never compared anything" (no baselines
  // exist) from "comparisons ran and found nothing".
  const neverCompared = summary != null && summary.runs_compared === 0;

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Benchmark Regressions"
        subtitle={regressions.length > 0 ? `${criticalCount > 0 ? `${criticalCount} critical` : ''}${criticalCount > 0 && warningCount > 0 ? ' / ' : ''}${warningCount > 0 ? `${warningCount} warning` : ''} — ${regressions.length} total` : undefined}
      />

      {/* Comparison-activity strip — the pipeline's pulse, shown whenever the
          detector has ever had a baseline to compare against. Counts are
          derived from run history (see the API's `semantics` field). */}
      {summary && summary.runs_compared > 0 && (
        <div
          className="flex flex-wrap items-center gap-x-5 gap-y-1 py-3 mb-4 text-xs border-b border-gray-800/50"
          title={summary.semantics}
        >
          <span className="text-gray-400">
            Runs compared <span className="text-gray-200 font-semibold ml-1 tabular-nums">{summary.runs_compared}</span>
          </span>
          {summary.last_comparison_at && (
            <span className="text-gray-400">
              Last comparison <span className="text-gray-200 ml-1">{timeAgo(summary.last_comparison_at)}</span>
            </span>
          )}
          <span className="text-gray-400">
            Comparable configs <span className="text-gray-200 font-semibold ml-1 tabular-nums">{summary.comparable_configs}</span>
          </span>
          {summary.pinned_baseline_configs > 0 && (
            <span className="text-gray-400">
              Pinned baselines <span className="text-gray-200 font-semibold ml-1 tabular-nums">{summary.pinned_baseline_configs}</span>
            </span>
          )}
          <span className="text-gray-400">
            Regressions <span className={`font-semibold ml-1 tabular-nums ${summary.total_regressions > 0 ? 'text-red-400' : 'text-green-400'}`}>{summary.total_regressions}</span>
          </span>
        </div>
      )}

      {regressions.length === 0 ? (
        neverCompared ? (
          <EmptyState
            message="No baselines available yet — detection has never compared anything"
            detail={
              <>
                A comparison needs the same benchmark config to complete twice: the detector compares
                each completed run against the config&apos;s pinned baseline, or its previous completed
                run. Matrix launches create fresh configs per cell, so they never accumulate a baseline
                on their own. To enable detection, <Link to={`/projects/${projectId}/schedules`} className="text-cyan-400 hover:text-cyan-300">schedule a benchmark config</Link> so
                it re-runs (run #2 onward is compared automatically), or open a completed benchmark
                run and pin it as baseline.
              </>
            }
          />
        ) : (
          <EmptyState
            message={summary
              ? `No regressions detected — ${summary.runs_compared} ${summary.runs_compared === 1 ? 'run' : 'runs'} compared against baselines${summary.last_comparison_at ? ` (last: ${timeAgo(summary.last_comparison_at)})` : ''}`
              : 'No regressions detected'}
            detail="When a benchmark run completes, each case is compared against the same case in the baseline run (the config's pinned baseline, or the previous completed run): a p50 more than 10% worse or a success rate below 99% is flagged automatically. Cases with fewer than 10 samples on either side are skipped so noise-level runs are never flagged."
          />
        )
      ) : (
        <DataTable
          columns={[
            { key: 'detected', label: 'Detected', cellClass: 'text-gray-400', render: (r) => timeAgo(r.detected_at) },
            {
              key: 'benchmark',
              label: 'Benchmark',
              render: (r) => (
                <Link
                  to={`/projects/${projectId}/benchmark-configs/${r.config_id}/results`}
                  className="text-gray-200 hover:text-cyan-400"
                >
                  {r.config_name}
                </Link>
              ),
            },
            { key: 'case', label: 'Case', hideBelow: 'md', cellClass: 'text-gray-400', render: (r) => r.case_id },
            { key: 'metric', label: 'Metric', hideBelow: 'lg', cellClass: 'text-gray-400', render: (r) => metricLabel(r.metric) },
            { key: 'baseline', label: 'Baseline', align: 'right', hideBelow: 'md', cellClass: 'text-gray-400', render: (r) => formatValue(r.metric, r.metric_unit, r.baseline_value) },
            { key: 'current', label: 'Current', align: 'right', cellClass: 'text-gray-200', render: (r) => formatValue(r.metric, r.metric_unit, r.current_value) },
            { key: 'delta', label: 'Delta', align: 'right', cellClass: 'text-red-400', render: (r) => formatDelta(r.metric, r.delta_percent) },
            {
              key: 'severity',
              label: 'Severity',
              render: (r) => (
                <span className={`text-xs px-2 py-0.5 rounded ${severityColor(r.severity)}`}>
                  {r.severity}
                </span>
              ),
            },
          ]}
          rows={regressions}
          rowKey={(r) => r.regression_id}
          rowClass={(r) => (r.severity === 'critical' ? 'bg-red-500/5' : undefined)}
        />
      )}
    </div>
  );
}
