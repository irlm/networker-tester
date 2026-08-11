import { useState, useCallback } from 'react';
import { Link } from 'react-router';
import { api } from '../api/client';
import type { BenchmarkRegressionWithConfig } from '../api/types';
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
  const [loading, setLoading] = useState(true);

  usePageTitle('Benchmark Regressions');

  const refresh = useCallback(() => {
    if (!projectId) return;
    api.listBenchmarkRegressions(projectId, 100)
      .then(r => { setRegressions(r); setLoading(false); })
      .catch(() => setLoading(false));
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

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Benchmark Regressions"
        subtitle={regressions.length > 0 ? `${criticalCount > 0 ? `${criticalCount} critical` : ''}${criticalCount > 0 && warningCount > 0 ? ' / ' : ''}${warningCount > 0 ? `${warningCount} warning` : ''} — ${regressions.length} total` : undefined}
      />

      {regressions.length === 0 ? (
        <EmptyState
          message="No regressions detected"
          detail="When a benchmark run completes, each case is compared against the same case in the baseline run (the config's pinned baseline, or the previous completed run): a p50 more than 10% worse or a success rate below 99% is flagged automatically. Cases with fewer than 10 samples on either side are skipped so noise-level runs are never flagged. Run a benchmark config at least twice to enable regression tracking."
        />
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
            { key: 'case', label: 'Case', hideBelow: 'md', cellClass: 'text-gray-400 font-mono', render: (r) => r.case_id },
            { key: 'metric', label: 'Metric', hideBelow: 'lg', cellClass: 'text-gray-400', render: (r) => metricLabel(r.metric) },
            { key: 'baseline', label: 'Baseline', align: 'right', hideBelow: 'md', cellClass: 'text-gray-400 font-mono', render: (r) => formatValue(r.metric, r.metric_unit, r.baseline_value) },
            { key: 'current', label: 'Current', align: 'right', cellClass: 'text-gray-200 font-mono', render: (r) => formatValue(r.metric, r.metric_unit, r.current_value) },
            { key: 'delta', label: 'Delta', align: 'right', cellClass: 'font-mono text-red-400', render: (r) => formatDelta(r.metric, r.delta_percent) },
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
        />
      )}
    </div>
  );
}
