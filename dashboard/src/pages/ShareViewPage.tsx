import { useState, useEffect, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { api } from '../api/client';
import type { LiveAttempt } from '../api/types';
import {
  computeProtocolStats,
  computeTimingBreakdown,
  formatMs,
  formatMetricValue,
  formatBytes,
  successRateClass,
  type ProtocolStats,
  type TimingBreakdown,
} from '../lib/analysis';

interface ShareData {
  resource_type: string;
  resource_id: string | null;
  label: string | null;
  data: unknown;
  shared_by: string;
  expires_at: string;
}

export function ShareViewPage() {
  const { token } = useParams<{ token: string }>();
  const [shareData, setShareData] = useState<ShareData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    api.resolveShareLink(token)
      .then(data => { setShareData(data); setLoading(false); })
      .catch(e => { setError(String(e)); setLoading(false); });
  }, [token]);

  if (loading) {
    return (
      <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
        <div className="text-gray-500 motion-safe:animate-pulse">Loading shared report...</div>
      </div>
    );
  }

  if (error || !shareData) {
    return (
      <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
        <div className="text-center">
          <h1 className="text-xl font-bold text-gray-300 mb-2">Link expired or invalid</h1>
          <p className="text-sm text-gray-600">This share link may have been revoked or has expired.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex flex-col">
      {/* Header */}
      <header className="border-b border-gray-800 px-6 py-4">
        <h1 className="text-lg font-bold text-gray-200">
          AletheDash <span className="text-gray-600 font-normal">- Shared Report</span>
        </h1>
        {shareData.label && (
          <p className="text-sm text-gray-400 mt-1">{shareData.label}</p>
        )}
      </header>

      {/* Content */}
      <main className="flex-1 p-4 md:p-6 max-w-6xl mx-auto w-full">
        {shareData.resource_type === 'run' && (
          <SharedRunView data={shareData.data as LiveAttempt[]} />
        )}
        {shareData.resource_type === 'job' && (
          <SharedJobView data={shareData.data} />
        )}
      </main>

      {/* Footer */}
      <footer className="border-t border-gray-800 px-6 py-4 text-center">
        <p className="text-xs text-gray-600">
          Shared by {shareData.shared_by} · Expires {new Date(shareData.expires_at).toLocaleDateString()}
        </p>
      </footer>
    </div>
  );
}

function SharedRunView({ data }: { data: LiveAttempt[] }) {
  const attempts = useMemo(() => data || [], [data]);
  const protocolStats = useMemo(() => computeProtocolStats(attempts), [attempts]);
  const timingBreakdown = useMemo(() => computeTimingBreakdown(attempts), [attempts]);
  const successCount = attempts.filter(a => a.success).length;
  const failureCount = attempts.length - successCount;

  return (
    <div>
      {/* Summary */}
      <div className="flex flex-wrap items-center gap-x-5 gap-y-1 py-3 mb-6 text-xs border-b border-gray-800/50">
        <span className="text-gray-500">
          Probes <span className="text-gray-200 font-mono font-semibold ml-1">{attempts.length}</span>
        </span>
        <span className="text-gray-500">
          Success <span className="text-green-400 font-mono font-semibold ml-1">{successCount}</span>
        </span>
        <span className="text-gray-500">
          Failed <span className={`font-mono font-semibold ml-1 ${failureCount > 0 ? 'text-red-400' : 'text-gray-600'}`}>{failureCount}</span>
        </span>
        <span className="text-gray-500">
          Rate <span className={`font-mono font-semibold ml-1 ${successRateClass(attempts.length > 0 ? (successCount / attempts.length) * 100 : 100)}`}>
            {attempts.length > 0 ? `${((successCount / attempts.length) * 100).toFixed(0)}%` : '-'}
          </span>
        </span>
      </div>

      {/* Timing Breakdown */}
      {timingBreakdown.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            timing breakdown by protocol
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-4 py-2 text-left">Protocol</th>
                  <th className="px-4 py-2 text-right">N</th>
                  <th className="px-4 py-2 text-right">Avg DNS</th>
                  <th className="px-4 py-2 text-right">Avg TCP</th>
                  <th className="px-4 py-2 text-right">Avg TLS</th>
                  <th className="px-4 py-2 text-right">Avg TTFB</th>
                  <th className="px-4 py-2 text-right">Avg Total</th>
                  <th className="px-4 py-2 text-right">Success</th>
                </tr>
              </thead>
              <tbody>
                {timingBreakdown.map(row => (
                  <SharedTimingRow key={row.protocol} row={row} />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Statistics Summary */}
      {protocolStats.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            statistics summary
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-4 py-2 text-left">Protocol</th>
                  <th className="px-4 py-2 text-left">Metric</th>
                  <th className="px-4 py-2 text-right">N</th>
                  <th className="px-4 py-2 text-right">Min</th>
                  <th className="px-4 py-2 text-right">Mean</th>
                  <th className="px-4 py-2 text-right">p50</th>
                  <th className="px-4 py-2 text-right">p95</th>
                  <th className="px-4 py-2 text-right">p99</th>
                  <th className="px-4 py-2 text-right">Max</th>
                  <th className="px-4 py-2 text-right">StdDev</th>
                  <th className="px-4 py-2 text-right">Success</th>
                </tr>
              </thead>
              <tbody>
                {protocolStats.map(ps => (
                  <SharedStatsRow key={`${ps.protocol}:${ps.payloadBytes}`} ps={ps} />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Individual attempts */}
      <div className="table-container">
        <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
          probe details ({attempts.length} attempts)
        </h3>
        <div className="max-h-96 overflow-y-auto">
          {attempts.map(a => (
            <div key={a.attempt_id} className="px-4 py-2 border-b border-gray-800/30 text-xs">
              <div className="flex items-center gap-3">
                <span className="text-gray-500 font-mono w-8">#{a.sequence_num}</span>
                <span className="text-gray-400 font-mono">{a.protocol}</span>
                {a.success
                  ? <span className="text-green-400">OK</span>
                  : <span className="text-red-400">FAIL</span>
                }
                {a.http && <span className="text-gray-600">TTFB {formatMs(a.http.ttfb_ms)} · Total {formatMs(a.http.total_duration_ms)}</span>}
                {a.udp && <span className="text-gray-600">RTT {formatMs(a.udp.rtt_avg_ms)} · Loss {a.udp.loss_percent.toFixed(1)}%</span>}
                {a.error && <span className="text-red-400/70">{a.error.message}</span>}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function SharedJobView({ data }: { data: unknown }) {
  const job = data as Record<string, unknown>;
  if (!job) return <p className="text-gray-500">No data available.</p>;

  return (
    <div>
      <div className="table-container">
        <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
          test details
        </h3>
        <div className="p-4 space-y-2 text-sm">
          {Object.entries(job).map(([key, value]) => (
            <div key={key} className="flex gap-4">
              <span className="text-gray-500 w-32 shrink-0">{key}</span>
              <span className="text-gray-200 font-mono text-xs break-all">
                {typeof value === 'object' ? JSON.stringify(value, null, 2) : String(value ?? '-')}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function SharedTimingRow({ row }: { row: TimingBreakdown }) {
  const successPct = row.totalCount > 0 ? (row.successCount / row.totalCount) * 100 : 0;
  return (
    <tr className="border-b border-gray-800/30 hover:bg-gray-800/10">
      <td className="px-4 py-2 text-gray-200 font-medium">{row.protocol}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{row.count}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{formatMs(row.avgDns)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{formatMs(row.avgTcp)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{formatMs(row.avgTls)}</td>
      <td className="px-4 py-2 text-gray-200 text-right font-mono">{formatMs(row.avgTtfb)}</td>
      <td className="px-4 py-2 text-gray-100 text-right font-mono font-bold">{formatMs(row.avgTotal)}</td>
      <td className={`px-4 py-2 text-right font-mono ${successRateClass(successPct)}`}>
        {row.successCount}/{row.totalCount}
      </td>
    </tr>
  );
}

function SharedStatsRow({ ps }: { ps: ProtocolStats }) {
  const fmt = (v: number) => formatMetricValue(ps.protocol, v);
  return (
    <tr className="border-b border-gray-800/30 hover:bg-gray-800/10">
      <td className="px-4 py-2 text-gray-200 font-medium">
        {ps.protocol}
        {ps.payloadBytes != null && <span className="text-gray-500 ml-1">({formatBytes(ps.payloadBytes)})</span>}
      </td>
      <td className="px-4 py-2 text-gray-500">{ps.label}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{ps.stats.count}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.min)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.mean)}</td>
      <td className="px-4 py-2 text-gray-100 text-right font-mono font-semibold">{fmt(ps.stats.p50)}</td>
      <td className="px-4 py-2 text-yellow-400 text-right font-mono">{fmt(ps.stats.p95)}</td>
      <td className="px-4 py-2 text-orange-400 text-right font-mono">{fmt(ps.stats.p99)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.max)}</td>
      <td className="px-4 py-2 text-gray-500 text-right font-mono">{fmt(ps.stats.stddev)}</td>
      <td className={`px-4 py-2 text-right font-mono ${successRateClass(ps.successRate)}`}>
        {ps.successRate.toFixed(0)}%
      </td>
    </tr>
  );
}
