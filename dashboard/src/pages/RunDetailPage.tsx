import { useState, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { api, type RunSummary } from '../api/client';
import type { LiveAttempt } from '../api/types';
import { useProject } from '../hooks/useProject';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ShareDialog } from '../components/ShareDialog';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import {
  computeProtocolStats,
  computeTimingBreakdown,
  computeStats,
  primaryMetricValue,
  formatMs,
  formatMetricValue,
  formatBytes,
  successRateClass,
  type ProtocolStats,
  type TimingBreakdown,
} from '../lib/analysis';
import { TOOLTIP_STYLE } from '../lib/chart';
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  CartesianGrid,
} from 'recharts';

export function RunDetailPage() {
  const { projectId, isProjectAdmin } = useProject();
  const { runId } = useParams<{ runId: string }>();
  const [run, setRun] = useState<RunSummary | null>(null);
  const [attempts, setAttempts] = useState<LiveAttempt[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedProtocols, setExpandedProtocols] = useState<Set<string>>(new Set());
  const [showShareDialog, setShowShareDialog] = useState(false);

  const shortId = runId?.slice(0, 8) ?? '';
  usePageTitle(runId ? `Run ${shortId}` : 'Run');

  usePolling(
    () => {
      if (!runId || !projectId) return;
      api
        .getRunAttempts(projectId, runId)
        .then((data) => {
          setAttempts(data as unknown as LiveAttempt[]);
          setError(null);
          setLoading(false);
        })
        .catch((e) => { setError(String(e)); setLoading(false); });
      api
        .getRun(projectId, runId!)
        .then((data) => setRun(data as unknown as RunSummary))
        .catch(() => {});
    },
    15000,
    !!runId
  );

  // ── Analysis (shared with HTML report logic) ──
  const protocolStats = useMemo(() => computeProtocolStats(attempts), [attempts]);
  const timingBreakdown = useMemo(() => computeTimingBreakdown(attempts), [attempts]);

  const ttfbDistribution = useMemo(() => {
    const values = attempts
      .filter((a) => a.success && a.http?.ttfb_ms != null)
      .map((a) => a.http!.ttfb_ms);
    if (values.length < 3) return [];
    const sorted = [...values].sort((a, b) => a - b);
    const min = sorted[0];
    const max = sorted[sorted.length - 1];
    const range = max - min || 1;
    const bucketCount = Math.min(15, Math.max(5, Math.ceil(values.length / 3)));
    const bucketSize = range / bucketCount;
    const buckets: { range: string; count: number }[] = [];
    for (let i = 0; i < bucketCount; i++) {
      const from = min + i * bucketSize;
      const to = from + bucketSize;
      buckets.push({ range: `${from.toFixed(1)}-${to.toFixed(1)}`, count: 0 });
    }
    for (const v of values) {
      const idx = Math.min(Math.floor((v - min) / bucketSize), bucketCount - 1);
      buckets[idx].count++;
    }
    return buckets;
  }, [attempts]);

  // Per-protocol metric chart data
  const protocolChartData = useMemo(() => {
    return protocolStats.map((ps) => ({
      name: ps.payloadBytes
        ? `${ps.protocol} (${formatBytes(ps.payloadBytes)})`
        : ps.protocol,
      p50: Number(ps.stats.p50.toFixed(2)),
      p95: Number(ps.stats.p95.toFixed(2)),
      mean: Number(ps.stats.mean.toFixed(2)),
    }));
  }, [protocolStats]);

  const toggleProtocol = (protocol: string) => {
    setExpandedProtocols((prev) => {
      const next = new Set(prev);
      if (next.has(protocol)) { next.delete(protocol); } else { next.add(protocol); }
      return next;
    });
  };

  if (loading && attempts.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading run {shortId}...</div>
      </div>
    );
  }

  if (error && attempts.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load run</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  const successCount = attempts.filter((a) => a.success).length;
  const failureCount = attempts.length - successCount;

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />

      {/* Header */}
      <div className="mb-6 flex items-start justify-between">
        <div>
          <h2 className="text-xl font-bold text-gray-100 mb-1">Run {shortId}</h2>
          <p className="text-sm text-gray-500">
            {run?.target_host && <>Target: <span className="text-gray-300">{run.target_host}</span> · </>}
            {run?.modes && <>Modes: <span className="text-gray-300">{run.modes}</span> · </>}
            {attempts.length} attempts
          </p>
        </div>
        {isProjectAdmin && runId && (
          <button
            onClick={() => setShowShareDialog(true)}
            className="px-3 py-1.5 text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 rounded transition-colors border border-gray-700"
          >
            Share
          </button>
        )}
      </div>

      {showShareDialog && runId && (
        <ShareDialog
          projectId={projectId}
          resourceType="run"
          resourceId={runId}
          onClose={() => setShowShareDialog(false)}
        />
      )}

      {/* Inline metrics */}
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

      {/* ── Timing Breakdown Table (mirrors HTML report) ── */}
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
                {timingBreakdown.map((row) => (
                  <TimingRow key={row.protocol} row={row} />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* ── Statistics Summary Table (mirrors HTML report) ── */}
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
                {protocolStats.map((ps) => (
                  <StatsRow key={`${ps.protocol}:${ps.payloadBytes}`} ps={ps} />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* ── Protocol Comparison Chart ── */}
      {protocolChartData.length > 1 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">protocol comparison — p50 vs p95</h3>
          <ResponsiveContainer width="100%" height={250}>
            <BarChart data={protocolChartData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis dataKey="name" stroke="#4b5563" fontSize={10} />
              <YAxis stroke="#4b5563" fontSize={10} />
              <Tooltip contentStyle={TOOLTIP_STYLE} />
              <Bar dataKey="p50" fill="#94a3b8" name="p50" />
              <Bar dataKey="p95" fill="#eab308" name="p95" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* ── Box-and-Whisker Chart ── */}
      {protocolStats.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">latency distribution — box &amp; whisker</h3>
          <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
            <div className="space-y-3">
              {protocolStats.map((ps) => {
                const s = ps.stats;
                const maxVal = Math.max(...protocolStats.map(p => p.stats.max));
                const scale = (v: number) => maxVal > 0 ? (v / maxVal) * 100 : 0;
                const whiskerLeft = scale(s.min);
                const boxLeft = scale(s.p25);
                const median = scale(s.p50);
                const boxRight = scale(s.p75);
                const whiskerRight = scale(s.max);
                return (
                  <div key={ps.protocol} className="flex items-center gap-3">
                    <div className="w-20 text-xs text-gray-400 font-mono text-right shrink-0">{ps.protocol}</div>
                    <div className="flex-1 relative h-6">
                      {/* Whisker line (min to max) */}
                      <div className="absolute top-1/2 -translate-y-1/2 h-px bg-gray-600" style={{ left: `${whiskerLeft}%`, width: `${whiskerRight - whiskerLeft}%` }} />
                      {/* Min tick */}
                      <div className="absolute top-1 bottom-1 w-px bg-gray-500" style={{ left: `${whiskerLeft}%` }} />
                      {/* Max tick */}
                      <div className="absolute top-1 bottom-1 w-px bg-gray-500" style={{ left: `${whiskerRight}%` }} />
                      {/* Box (p25 to p75) */}
                      <div className="absolute top-0.5 bottom-0.5 rounded-sm border border-cyan-600/60 bg-cyan-900/30" style={{ left: `${boxLeft}%`, width: `${Math.max(boxRight - boxLeft, 0.5)}%` }} />
                      {/* Median line */}
                      <div className="absolute top-0 bottom-0 w-0.5 bg-cyan-400" style={{ left: `${median}%` }} />
                    </div>
                    <div className="w-24 text-xs text-gray-500 font-mono shrink-0">
                      {formatMs(s.min)}&ndash;{formatMs(s.max)}
                    </div>
                  </div>
                );
              })}
            </div>
            <div className="flex justify-between text-[10px] text-gray-600 mt-2 px-[calc(5rem+0.75rem)]">
              <span>0ms</span>
              <span>{formatMs(Math.max(...protocolStats.map(p => p.stats.max)))}</span>
            </div>
            <div className="flex items-center gap-4 mt-3 text-[10px] text-gray-600 px-[calc(5rem+0.75rem)]">
              <span className="flex items-center gap-1"><span className="w-3 h-px bg-gray-500 inline-block" /> whisker (min/max)</span>
              <span className="flex items-center gap-1"><span className="w-3 h-3 rounded-sm border border-cyan-600/60 bg-cyan-900/30 inline-block" /> IQR (p25–p75)</span>
              <span className="flex items-center gap-1"><span className="w-0.5 h-3 bg-cyan-400 inline-block" /> median (p50)</span>
            </div>
          </div>
        </div>
      )}

      {/* ── TTFB Distribution ── */}
      {ttfbDistribution.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">TTFB distribution (ms)</h3>
          <ResponsiveContainer width="100%" height={200}>
            <BarChart data={ttfbDistribution}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis dataKey="range" stroke="#4b5563" fontSize={9} angle={-30} textAnchor="end" height={50} />
              <YAxis stroke="#4b5563" fontSize={10} allowDecimals={false} />
              <Tooltip contentStyle={TOOLTIP_STYLE} />
              <Bar dataKey="count" fill="#8b5cf6" name="Attempts" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* ── Attempts by Protocol (collapsible) ── */}
      <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">probe details</h3>
      {Object.entries(groupByProtocol(attempts)).map(([protocol, group]) => {
        const isExpanded = expandedProtocols.has(protocol);
        const protoSuccess = group.filter((a) => a.success).length;
        const protoFail = group.length - protoSuccess;
        const values = group.filter((a) => a.success).map(primaryMetricValue).filter((v): v is number => v != null);
        const stats = computeStats(values);

        return (
          <div key={protocol} className="table-container mb-2">
            <button
              onClick={() => toggleProtocol(protocol)}
              className="w-full px-4 py-2.5 flex items-center justify-between text-left hover:bg-gray-800/10 transition-colors"
              aria-expanded={isExpanded}
            >
              <div className="flex items-center gap-3">
                <span className="text-gray-500 text-xs transition-transform" style={{ transform: isExpanded ? 'rotate(90deg)' : '' }} aria-hidden="true">{'\u25B6'}</span>
                <span className="text-gray-200 font-medium text-sm">{protocol.toUpperCase()}</span>
                <span className="text-gray-500 text-xs">{group.length} attempts</span>
                {stats && (
                  <span className="text-gray-600 text-xs">
                    p50: {formatMetricValue(protocol, stats.p50)} · p95: {formatMetricValue(protocol, stats.p95)}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-3 text-xs">
                <span className="text-green-400">{protoSuccess} OK</span>
                {protoFail > 0 && <span className="text-red-400">{protoFail} FAIL</span>}
              </div>
            </button>

            {isExpanded && (
              <div className="border-t border-gray-800 max-h-96 overflow-y-auto">
                {group.map((a) => (
                  <AttemptRow key={a.attempt_id} a={a} />
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

// ─── Sub-components ──────────────────────────────────────────────────────────

function TimingRow({ row }: { row: TimingBreakdown }) {
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

function StatsRow({ ps }: { ps: ProtocolStats }) {
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

function AttemptRow({ a }: { a: LiveAttempt }) {
  return (
    <div className="px-4 py-3 border-b border-gray-800/30 hover:bg-gray-800/10">
      <div className="flex items-center gap-4 mb-2">
        <span className="text-gray-500 font-mono text-xs w-8">#{a.sequence_num}</span>
        {a.success
          ? <span className="text-green-400 text-xs font-medium">OK</span>
          : <span className="text-red-400 text-xs font-medium">FAIL</span>
        }
        {a.retry_count > 0 && <span className="text-gray-600 text-xs">{a.retry_count} retries</span>}
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-2 text-xs">
        {a.dns && (
          <SubResult label="DNS" color="gray">
            <p className="text-gray-300">{formatMs(a.dns.duration_ms)}</p>
            {a.dns.resolved_ips?.length > 0 && (
              <p className="text-gray-500 font-mono truncate">{a.dns.resolved_ips.join(', ')}</p>
            )}
          </SubResult>
        )}
        {a.tcp && (
          <SubResult label="TCP" color="gray">
            <p className="text-gray-300">{formatMs(a.tcp.connect_duration_ms)}</p>
            <p className="text-gray-500 font-mono truncate">{a.tcp.remote_addr}</p>
          </SubResult>
        )}
        {a.tls && (
          <SubResult label="TLS" color="gray">
            <p className="text-gray-300">{formatMs(a.tls.handshake_duration_ms)}</p>
            <p className="text-gray-500 font-mono truncate">{a.tls.protocol_version} · {a.tls.cipher_suite}</p>
          </SubResult>
        )}
        {a.http && (
          <SubResult label="HTTP" color="cyan">
            <p className="text-gray-300">
              <span className={a.http.status_code >= 400 ? 'text-red-400' : 'text-green-400'}>{a.http.status_code}</span>
              {' · '}TTFB {formatMs(a.http.ttfb_ms)} · Total {formatMs(a.http.total_duration_ms)}
            </p>
            <p className="text-gray-500 font-mono truncate">
              {a.http.negotiated_version}
              {a.http.throughput_mbps != null && ` · ${a.http.throughput_mbps.toFixed(1)} MB/s`}
              {a.http.payload_bytes != null && a.http.payload_bytes > 0 && ` · ${formatBytes(a.http.payload_bytes)}`}
            </p>
          </SubResult>
        )}
        {a.udp && (
          <SubResult label="UDP" color="gray">
            <p className="text-gray-300">RTT avg {formatMs(a.udp.rtt_avg_ms)} · Loss {a.udp.loss_percent.toFixed(1)}%</p>
            <p className="text-gray-500">{a.udp.probe_count} probes</p>
          </SubResult>
        )}
        {a.page_load && (
          <SubResult label="Page Load" color="blue">
            <p className="text-gray-300">Total {formatMs(a.page_load.total_ms)} · {a.page_load.assets_fetched}/{a.page_load.asset_count} assets</p>
            {a.page_load.tls_setup_ms != null && <p className="text-gray-500">TLS setup: {formatMs(a.page_load.tls_setup_ms)}</p>}
          </SubResult>
        )}
        {a.browser && (
          <SubResult label="Browser" color="purple">
            <p className="text-gray-300">Load {formatMs(a.browser.load_ms)}</p>
            {a.browser.dom_content_loaded_ms != null && <p className="text-gray-500">DCL: {formatMs(a.browser.dom_content_loaded_ms)}</p>}
          </SubResult>
        )}
        {a.error && (
          <SubResult label="Error" color="red">
            <p className="text-red-300">{a.error.category}: {a.error.message}</p>
            {a.error.detail && <p className="text-red-400/60 truncate">{a.error.detail}</p>}
          </SubResult>
        )}
      </div>
    </div>
  );
}

function SubResult({ label, color, children }: { label: string; color: string; children: React.ReactNode }) {
  const borderColor = color === 'red' ? 'border-red-500/20' : color === 'cyan' ? 'border-gray-600' : color === 'blue' ? 'border-blue-500/20' : color === 'purple' ? 'border-purple-500/20' : 'border-gray-800';
  const bgColor = color === 'red' ? 'bg-red-500/5' : 'bg-[var(--bg-base)]';
  const labelColor = color === 'red' ? 'text-red-400' : color === 'cyan' ? 'text-gray-300' : color === 'blue' ? 'text-blue-400' : color === 'purple' ? 'text-purple-400' : 'text-gray-500';
  return (
    <div className={`${bgColor} border ${borderColor} rounded p-2`}>
      <p className={`${labelColor} tracking-wider mb-1 text-[11px] font-medium`}>{label}</p>
      {children}
    </div>
  );
}

function groupByProtocol(attempts: LiveAttempt[]): Record<string, LiveAttempt[]> {
  const groups: Record<string, LiveAttempt[]> = {};
  for (const a of attempts) {
    const key = a.protocol || 'unknown';
    if (!groups[key]) groups[key] = [];
    groups[key].push(a);
  }
  return groups;
}
