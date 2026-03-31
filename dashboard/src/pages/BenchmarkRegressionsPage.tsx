import { useState, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkRegressionWithConfig } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';

function timeAgo(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

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
    case 'success_rate': return 'Success Rate';
    default: return metric;
  }
}

function formatValue(metric: string, value: number): string {
  if (metric === 'success_rate') return `${value.toFixed(1)}%`;
  return `${value.toFixed(2)}ms`;
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
          detail="Regressions are automatically flagged when a benchmark completes and its p50 latency increases by more than 10% or success rate drops below 99% compared to the baseline run. Run benchmarks with a baseline set to enable regression tracking."
        />
      ) : (
        <>
          {/* Mobile card layout */}
          <div className="md:hidden space-y-2">
            {regressions.map(r => (
              <div key={r.regression_id} className="border border-gray-800 rounded p-3">
                <div className="flex items-start justify-between gap-2 mb-2">
                  <div className="min-w-0">
                    <Link
                      to={`/projects/${projectId}/benchmark-configs/${r.config_id}/results`}
                      className="text-gray-200 text-sm font-medium hover:text-cyan-400 truncate block"
                    >
                      {r.config_name}
                    </Link>
                    <p className="text-gray-500 text-xs">{r.language}</p>
                  </div>
                  <span className={`text-xs px-2 py-0.5 rounded ${severityColor(r.severity)}`}>
                    {r.severity}
                  </span>
                </div>
                <div className="flex items-center gap-3 text-xs">
                  <span className="text-gray-400">{metricLabel(r.metric)}</span>
                  <span className="text-gray-500">{formatValue(r.metric, r.baseline_value)}</span>
                  <span className="text-gray-600">-&gt;</span>
                  <span className="text-gray-200">{formatValue(r.metric, r.current_value)}</span>
                  <span className={r.delta_percent > 0 ? 'text-red-400' : 'text-green-400'}>
                    {r.delta_percent > 0 ? '+' : ''}{r.delta_percent.toFixed(1)}%
                  </span>
                </div>
                <p className="text-xs text-gray-600 mt-1">{timeAgo(r.detected_at)}</p>
              </div>
            ))}
          </div>

          {/* Desktop table */}
          <div className="hidden md:block table-container">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="px-4 py-2.5 text-left font-medium">Detected</th>
                  <th className="px-4 py-2.5 text-left font-medium">Benchmark</th>
                  <th className="px-4 py-2.5 text-left font-medium">Language</th>
                  <th className="px-4 py-2.5 text-left font-medium">Metric</th>
                  <th className="px-4 py-2.5 text-right font-medium">Baseline</th>
                  <th className="px-4 py-2.5 text-right font-medium">Current</th>
                  <th className="px-4 py-2.5 text-right font-medium">Delta</th>
                  <th className="px-4 py-2.5 text-left font-medium">Severity</th>
                </tr>
              </thead>
              <tbody>
                {regressions.map(r => (
                  <tr
                    key={r.regression_id}
                    className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${
                      r.severity === 'critical' ? 'bg-red-500/5' : ''
                    }`}
                  >
                    <td className="px-4 py-3 text-gray-500 text-xs">{timeAgo(r.detected_at)}</td>
                    <td className="px-4 py-3">
                      <Link
                        to={`/projects/${projectId}/benchmark-configs/${r.config_id}/results`}
                        className="text-gray-200 hover:text-cyan-400"
                      >
                        {r.config_name}
                      </Link>
                    </td>
                    <td className="px-4 py-3 text-gray-400 font-mono text-xs">{r.language}</td>
                    <td className="px-4 py-3 text-gray-400 text-xs">{metricLabel(r.metric)}</td>
                    <td className="px-4 py-3 text-gray-500 text-xs text-right font-mono">
                      {formatValue(r.metric, r.baseline_value)}
                    </td>
                    <td className="px-4 py-3 text-gray-200 text-xs text-right font-mono">
                      {formatValue(r.metric, r.current_value)}
                    </td>
                    <td className={`px-4 py-3 text-xs text-right font-mono ${
                      r.delta_percent > 0 ? 'text-red-400' : 'text-green-400'
                    }`}>
                      {r.delta_percent > 0 ? '+' : ''}{r.delta_percent.toFixed(1)}%
                    </td>
                    <td className="px-4 py-3">
                      <span className={`text-xs px-2 py-0.5 rounded ${severityColor(r.severity)}`}>
                        {r.severity}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </div>
  );
}
