import { useState, useMemo, useEffect } from 'react';
import { useParams, Link } from 'react-router-dom';
import { api, type Job } from '../api/client';
import type { LiveAttempt } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { useLiveStore } from '../stores/liveStore';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import {
  computeProtocolStats,
  computeTimingBreakdown,
  primaryMetricValue,
  formatMs,
  formatMetricValue,
  formatBytes,
  successRateClass,
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

const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled']);
const EMPTY_ATTEMPTS: LiveAttempt[] = [];
const EMPTY_LOGS: { line: string; level: string }[] = [];

export function JobDetailPage() {
  const { jobId } = useParams<{ jobId: string }>();
  const [job, setJob] = useState<Job | null>(null);
  const [dbAttempts, setDbAttempts] = useState<LiveAttempt[]>([]);
  const [runMeta, setRunMeta] = useState<{ client_version: string; client_os: string; endpoint_version: string | null } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const liveAttempts = useLiveStore((s) =>
    jobId ? s.liveAttempts[jobId] ?? EMPTY_ATTEMPTS : EMPTY_ATTEMPTS
  );
  const jobLogs = useLiveStore((s) =>
    jobId ? s.jobLogs[jobId] ?? EMPTY_LOGS : EMPTY_LOGS
  );
  const cleanupJob = useLiveStore((s) => s.cleanupJob);
  const addToast = useToast();

  const shortId = jobId?.slice(0, 8) ?? '';
  usePageTitle(jobId ? `Test ${shortId}` : 'Test');

  const isTerminal = job ? TERMINAL_STATUSES.has(job.status) : false;

  // Load attempts and run metadata from DB when job has a run_id (completed)
  useEffect(() => {
    if (job?.run_id && liveAttempts.length === 0) {
      api.getRunAttempts(job.run_id).then((data) => {
        setDbAttempts(data as unknown as LiveAttempt[]);
      }).catch(() => {});
      api.getRun(job.run_id).then((data) => {
        setRunMeta({
          client_version: data.client_version,
          client_os: data.client_os,
          endpoint_version: data.endpoint_version,
        });
      }).catch(() => {});
    }
  }, [job?.run_id, liveAttempts.length]);

  // Use live attempts while running, DB attempts when completed
  const attempts: LiveAttempt[] = liveAttempts.length > 0 ? liveAttempts : dbAttempts;

  // Clean up live attempts when job reaches terminal state
  useEffect(() => {
    if (isTerminal && jobId) {
      cleanupJob(jobId);
    }
  }, [isTerminal, jobId, cleanupJob]);

  usePolling(
    () => {
      if (!jobId) return;
      api
        .getJob(jobId)
        .then((j) => {
          setJob(j);
          setError(null);
          setLoading(false);
        })
        .catch((e) => {
          setError(String(e));
          setLoading(false);
        });
    },
    3000,
    !!jobId && !isTerminal
  );

  // Analysis using shared module (same logic as HTML report)
  const protocolStats = useMemo(() => computeProtocolStats(attempts), [attempts]);
  const timingBreakdown = useMemo(() => computeTimingBreakdown(attempts), [attempts]);

  // Build chart data from live attempts
  const chartData = useMemo(
    () =>
      attempts.map((a, i) => ({
        seq: i,
        protocol: a.protocol,
        success: a.success ? 1 : 0,
        ttfb_ms: a.http?.ttfb_ms ?? a.browser?.ttfb_ms ?? 0,
        total_ms: a.http?.total_duration_ms ?? a.browser?.load_ms ?? a.page_load?.total_ms ?? 0,
      })),
    [attempts]
  );

  // Box plot data per protocol (min, p25, p50, p75, max)
  const boxPlotData = useMemo(() => {
    return protocolStats
      .filter(ps => ps.stats.count >= 2)
      .map(ps => {
        // Approximate p25/p75 from the data
        const values = attempts
          .filter(a => a.protocol === ps.protocol && a.success)
          .map(a => primaryMetricValue(a))
          .filter((v): v is number => v != null && v > 0)
          .sort((a, b) => a - b);
        const p25 = values.length >= 4 ? values[Math.floor(values.length * 0.25)] : ps.stats.min;
        const p75 = values.length >= 4 ? values[Math.floor(values.length * 0.75)] : ps.stats.max;
        return {
          name: ps.protocol + (ps.payloadBytes != null ? ` (${formatBytes(ps.payloadBytes)})` : ''),
          min: Number(ps.stats.min.toFixed(1)),
          p25: Number(p25.toFixed(1)),
          p50: Number(ps.stats.p50.toFixed(1)),
          p75: Number(p75.toFixed(1)),
          max: Number(ps.stats.max.toFixed(1)),
          // For stacked bar rendering: each segment = difference from previous
          base: Number(ps.stats.min.toFixed(1)),
          iqr_low: Number((p25 - ps.stats.min).toFixed(1)),
          iqr_mid: Number((ps.stats.p50 - p25).toFixed(1)),
          iqr_high: Number((p75 - ps.stats.p50).toFixed(1)),
          whisker: Number((ps.stats.max - p75).toFixed(1)),
        };
      });
  }, [protocolStats, attempts]);

  // Protocol comparison chart data (p50 vs p95)
  const protocolChartData = useMemo(
    () =>
      protocolStats
        .filter((ps) => ps.stats.count >= 1)
        .map((ps) => ({
          name: ps.protocol + (ps.payloadBytes != null ? ` (${formatBytes(ps.payloadBytes)})` : ''),
          p50: Number(ps.stats.p50.toFixed(1)),
          p95: Number(ps.stats.p95.toFixed(1)),
        })),
    [protocolStats]
  );

  // TTFB distribution histogram
  const ttfbDistribution = useMemo(() => {
    const values = attempts
      .filter((a) => a.success)
      .map((a) => primaryMetricValue(a))
      .filter((v): v is number => v != null && v > 0);
    if (values.length < 2) return [];
    const min = Math.min(...values);
    const max = Math.max(...values);
    const range = max - min;
    if (range === 0) return [];
    const bucketCount = Math.min(12, Math.max(4, Math.ceil(values.length / 3)));
    const bucketSize = range / bucketCount;
    const buckets: { range: string; count: number }[] = [];
    for (let i = 0; i < bucketCount; i++) {
      const lo = min + i * bucketSize;
      const hi = lo + bucketSize;
      buckets.push({
        range: `${lo.toFixed(0)}-${hi.toFixed(0)}`,
        count: values.filter((v) => v >= lo && (i === bucketCount - 1 ? v <= hi : v < hi)).length,
      });
    }
    return buckets;
  }, [attempts]);

  const successCount = attempts.filter((a) => a.success).length;
  const failureCount = attempts.length - successCount;

  // Generate analysis observations
  const observations = useMemo(() => {
    if (protocolStats.length < 2) return [];
    const obs: string[] = [];
    const sorted = [...protocolStats]
      .filter(ps => ps.stats.count >= 1 && ps.stats.p50 > 0)
      .sort((a, b) => a.stats.p50 - b.stats.p50);
    if (sorted.length >= 2) {
      const fastest = sorted[0];
      const slowest = sorted[sorted.length - 1];
      if (slowest.stats.p50 > 0 && fastest.stats.p50 > 0) {
        const ratio = (slowest.stats.p50 / fastest.stats.p50).toFixed(1);
        obs.push(`${fastest.protocol} is the fastest at p50 (${fastest.stats.p50.toFixed(1)}ms), ${ratio}x faster than ${slowest.protocol} (${slowest.stats.p50.toFixed(1)}ms)`);
      }
    }
    // Compare H1 vs H2 vs H3 if all present
    const h1 = protocolStats.find(ps => ps.protocol.includes('1') && ps.stats.p50 > 0);
    const h2 = protocolStats.find(ps => ps.protocol.includes('2') && ps.stats.p50 > 0);
    const h3 = protocolStats.find(ps => ps.protocol.includes('3') && ps.stats.p50 > 0);
    if (h1 && h2 && h3) {
      const best = [h1, h2, h3].sort((a, b) => a.stats.p50 - b.stats.p50)[0];
      obs.push(`Best protocol: ${best.protocol} with p50=${best.stats.p50.toFixed(1)}ms`);
    }
    // Consistency check
    for (const ps of protocolStats) {
      if (ps.stats.count >= 3 && ps.stats.stddev > 0) {
        const cv = (ps.stats.stddev / ps.stats.mean) * 100;
        if (cv > 50) {
          obs.push(`${ps.protocol} shows high variability (CV=${cv.toFixed(0)}%) — results may be inconsistent`);
        }
      }
    }
    // Failure check
    for (const ps of protocolStats) {
      if (ps.successRate < 100 && ps.successRate > 0) {
        obs.push(`${ps.protocol}: ${ps.successRate.toFixed(0)}% success rate (${ps.stats.count - Math.round(ps.stats.count * ps.successRate / 100)} failures)`);
      } else if (ps.successRate === 0) {
        obs.push(`${ps.protocol}: all probes failed`);
      }
    }
    return obs;
  }, [protocolStats]);

  const handleCancel = async () => {
    if (!jobId) return;
    try {
      await api.cancelJob(jobId);
      addToast('success', `Job ${shortId} cancelled`);
    } catch (err) {
      addToast('error', err instanceof Error ? err.message : 'Failed to cancel job');
    }
  };

  if (loading && !job) {
    return (
      <div className="p-6">
        <Breadcrumb items={[{ label: 'Tests', to: '/tests' }, { label: `Test ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading job {shortId}...</div>
      </div>
    );
  }

  if (error && !job) {
    return (
      <div className="p-6">
        <Breadcrumb items={[{ label: 'Tests', to: '/tests' }, { label: `Test ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load job</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
          <p className="text-gray-500 text-xs mt-2">Test ID: {jobId}</p>
        </div>
      </div>
    );
  }

  if (!job) {
    return (
      <div className="p-6">
        <div className="text-gray-500">Job not found: {jobId}</div>
      </div>
    );
  }

  const isRunning = job.status === 'running' || job.status === 'assigned';
  const isFinished = TERMINAL_STATUSES.has(job.status);

  return (
    <div className="p-6">
      <Breadcrumb items={[{ label: 'Tests', to: '/tests' }, { label: `Test ${shortId}` }]} />

      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <div className="flex items-center gap-3 mb-1">
            <h2 className="text-xl font-bold text-gray-100">
              Test {job.job_id.slice(0, 8)}
            </h2>
            <StatusBadge status={job.status} />
          </div>
          <p className="text-sm text-gray-500">
            Target: {job.config?.target} | Modes: {job.config?.modes?.join(', ')}
          </p>
          {runMeta && (
            <div className="flex gap-3 mt-1">
              <span className="text-xs text-gray-600">
                Tester: <span className="text-gray-400 font-mono">v{runMeta.client_version}</span>
                <span className="text-gray-700 ml-1">({runMeta.client_os})</span>
              </span>
              {runMeta.endpoint_version && (
                <span className="text-xs text-gray-600">
                  Endpoint: <span className="text-gray-400 font-mono">v{runMeta.endpoint_version}</span>
                </span>
              )}
              {runMeta.endpoint_version === null && (
                <span className="text-xs text-gray-700">Endpoint: offline</span>
              )}
            </div>
          )}
        </div>
        {isRunning && (
          <button
            onClick={handleCancel}
            className="bg-red-600/20 text-red-400 border border-red-500/30 px-4 py-1.5 rounded text-sm hover:bg-red-600/30 transition-colors"
          >
            Cancel
          </button>
        )}
      </div>

      {/* Progress indicator */}
      {isRunning && (
        <div className="border border-blue-500/20 rounded p-4 mb-6 flex items-center gap-3">
          <span className="w-2 h-2 rounded-full bg-blue-400 motion-safe:animate-pulse" />
          <span className="text-blue-400 text-sm">
            Running... {attempts.length} probes completed
          </span>
        </div>
      )}

      {/* Finished summary */}
      {isFinished && attempts.length === 0 && (
        <div className="border border-gray-800/50 rounded p-4 mb-6">
          <p className="text-gray-400 text-sm">
            Test {job.status}.
            {job.run_id && (
              <span className="ml-2 text-gray-500">
                Run: <Link to={`/runs/${job.run_id}`} className="font-mono text-cyan-400 hover:underline">{job.run_id.slice(0, 8)}</Link>
              </span>
            )}
          </p>
          {job.error_message && (
            <p className="text-red-400 text-sm mt-2">Error: {job.error_message}</p>
          )}
        </div>
      )}

      {/* Inline metrics — compact bar instead of card grid */}
      {attempts.length > 0 && (
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
      )}

      {/* Probe Timing per attempt */}
      {chartData.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-3 font-medium">Probe Timing by Attempt</h3>
          <ResponsiveContainer width="100%" height={250}>
            <BarChart data={chartData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis
                dataKey="seq"
                stroke="#4b5563"
                fontSize={10}
                tickFormatter={(v, i) => chartData[i]?.protocol ?? v}
              />
              <YAxis stroke="#4b5563" fontSize={10} unit="ms" />
              <Tooltip
                contentStyle={TOOLTIP_STYLE}
                formatter={(v) => [`${Number(v).toFixed(1)}ms`]}
                labelFormatter={(_, payload) => {
                  const p = payload?.[0]?.payload;
                  return p ? `#${p.seq} ${p.protocol}` : '';
                }}
              />
              <Bar dataKey="ttfb_ms" fill="#94a3b8" name="TTFB" />
              <Bar dataKey="total_ms" fill="#64748b" name="Total" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* Timing Breakdown (shared analysis from lib/analysis.ts) */}
      {timingBreakdown.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            Timing Breakdown
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-right">N</th>
                  <th className="px-3 py-2 text-right">DNS</th>
                  <th className="px-3 py-2 text-right">TCP</th>
                  <th className="px-3 py-2 text-right">TLS</th>
                  <th className="px-3 py-2 text-right">TTFB</th>
                  <th className="px-3 py-2 text-right">Total</th>
                  <th className="px-3 py-2 text-right">OK</th>
                </tr>
              </thead>
              <tbody>
                {timingBreakdown.map((row) => (
                  <tr key={row.protocol} className="border-b border-gray-800/30">
                    <td className="px-3 py-2 text-gray-200">{row.protocol}</td>
                    <td className="px-3 py-2 text-gray-400 text-right">{row.count}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMs(row.avgDns)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMs(row.avgTcp)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMs(row.avgTls)}</td>
                    <td className="px-3 py-2 text-gray-200 text-right font-mono">{formatMs(row.avgTtfb)}</td>
                    <td className="px-3 py-2 text-gray-100 text-right font-mono font-bold">{formatMs(row.avgTotal)}</td>
                    <td className={`px-3 py-2 text-right font-mono ${successRateClass(row.totalCount > 0 ? (row.successCount / row.totalCount) * 100 : 100)}`}>
                      {row.successCount}/{row.totalCount}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Statistics Summary (shared analysis — same as HTML report) */}
      {protocolStats.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            Statistics Summary
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-right">N</th>
                  <th className="px-3 py-2 text-right">Min</th>
                  <th className="px-3 py-2 text-right">Mean</th>
                  <th className="px-3 py-2 text-right">p50</th>
                  <th className="px-3 py-2 text-right">p95</th>
                  <th className="px-3 py-2 text-right">p99</th>
                  <th className="px-3 py-2 text-right">Max</th>
                  <th className="px-3 py-2 text-right">StdDev</th>
                  <th className="px-3 py-2 text-right">Success</th>
                </tr>
              </thead>
              <tbody>
                {protocolStats.map((ps) => (
                  <tr key={`${ps.protocol}:${ps.payloadBytes}`} className="border-b border-gray-800/30">
                    <td className="px-3 py-2 text-gray-200">
                      {ps.protocol}
                      {ps.payloadBytes != null && <span className="text-gray-500 ml-1">({formatBytes(ps.payloadBytes)})</span>}
                    </td>
                    <td className="px-3 py-2 text-gray-400 text-right">{ps.stats.count}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.min)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.mean)}</td>
                    <td className="px-3 py-2 text-gray-100 text-right font-mono font-semibold">{formatMetricValue(ps.protocol, ps.stats.p50)}</td>
                    <td className="px-3 py-2 text-yellow-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.p95)}</td>
                    <td className="px-3 py-2 text-orange-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.p99)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.max)}</td>
                    <td className="px-3 py-2 text-gray-500 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.stddev)}</td>
                    <td className={`px-3 py-2 text-right font-mono ${successRateClass(ps.successRate)}`}>
                      {ps.successRate.toFixed(0)}%
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Protocol Comparison Chart */}
      {protocolChartData.length > 1 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-3 font-medium">Protocol Comparison — p50 vs p95</h3>
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

      {/* Box Plot: min / p25 / p50 / p75 / max */}
      {boxPlotData.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-3 font-medium">Distribution by Protocol</h3>
          <ResponsiveContainer width="100%" height={250}>
            <BarChart data={boxPlotData} layout="vertical">
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" horizontal={false} />
              <XAxis type="number" stroke="#4b5563" fontSize={10} unit="ms" />
              <YAxis type="category" dataKey="name" stroke="#4b5563" fontSize={10} width={100} />
              <Tooltip
                contentStyle={TOOLTIP_STYLE}
                formatter={(_v, name, props) => {
                  const d = props?.payload as Record<string, number> | undefined;
                  if (!d) return [_v, name];
                  if (name === 'iqr_mid') return [`p50: ${d.p50}ms`, 'Median'];
                  if (name === 'iqr_low') return [`p25: ${d.p25}ms`, 'Q1'];
                  if (name === 'iqr_high') return [`p75: ${d.p75}ms`, 'Q3'];
                  if (name === 'whisker') return [`max: ${d.max}ms`, 'Max'];
                  return null;
                }}
              />
              <Bar dataKey="base" stackId="box" fill="transparent" />
              <Bar dataKey="iqr_low" stackId="box" fill="#374151" name="p25" />
              <Bar dataKey="iqr_mid" stackId="box" fill="#94a3b8" name="Median" />
              <Bar dataKey="iqr_high" stackId="box" fill="#64748b" name="p75" />
              <Bar dataKey="whisker" stackId="box" fill="#374151" opacity={0.5} name="max" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* TTFB Distribution */}
      {ttfbDistribution.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-3 font-medium">Timing Distribution (ms)</h3>
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

      {/* Analysis & Observations */}
      {observations.length > 0 && (
        <div className="mb-6 border-l-2 border-gray-700 pl-4">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-2 font-medium">Analysis</h3>
          <ul className="space-y-1">
            {observations.map((obs, i) => (
              <li key={i} className="text-xs text-gray-400">
                {obs}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* Tester Log */}
      {jobLogs.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            Tester Log
          </h3>
          <div className="max-h-48 overflow-y-auto p-3 font-mono text-xs leading-5">
            {jobLogs.map((entry, i) => (
              <div
                key={i}
                className={
                  entry.level === 'error'
                    ? 'text-red-400'
                    : entry.level === 'warn'
                      ? 'text-yellow-400'
                      : 'text-gray-400'
                }
              >
                {entry.line}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Browser Results */}
      {attempts.some(a => a.browser) && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">Browser Results</h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-3 py-2 text-left">#</th>
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-right">TTFB</th>
                  <th className="px-3 py-2 text-right">DCL</th>
                  <th className="px-3 py-2 text-right">Load</th>
                  <th className="px-3 py-2 text-right">Resources</th>
                  <th className="px-3 py-2 text-right">Transferred</th>
                  <th className="px-3 py-2 text-right">Negotiated</th>
                </tr>
              </thead>
              <tbody>
                {attempts.filter(a => a.browser).map(a => (
                  <tr key={a.attempt_id} className="border-b border-gray-800/30">
                    <td className="px-3 py-2 text-gray-500 font-mono">{a.sequence_num}</td>
                    <td className="px-3 py-2 text-gray-300">{a.protocol}</td>
                    <td className="px-3 py-2 text-gray-200 text-right font-mono">{formatMs(a.browser?.ttfb_ms)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMs(a.browser?.dom_content_loaded_ms)}</td>
                    <td className="px-3 py-2 text-gray-100 text-right font-mono font-bold">{formatMs(a.browser?.load_ms)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right">{a.browser?.resource_count ?? '-'}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{a.browser?.transferred_bytes ? formatBytes(a.browser.transferred_bytes) : '-'}</td>
                    <td className="px-3 py-2 text-gray-400 text-right">{a.browser?.protocol ?? '-'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Page Load Results */}
      {attempts.some(a => a.page_load) && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">Page Load Results</h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-3 py-2 text-left">#</th>
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-right">TTFB</th>
                  <th className="px-3 py-2 text-right">Total</th>
                  <th className="px-3 py-2 text-right">Assets</th>
                  <th className="px-3 py-2 text-right">Fetched</th>
                  <th className="px-3 py-2 text-right">Bytes</th>
                  <th className="px-3 py-2 text-right">Connections</th>
                </tr>
              </thead>
              <tbody>
                {attempts.filter(a => a.page_load).map(a => (
                  <tr key={a.attempt_id} className="border-b border-gray-800/30">
                    <td className="px-3 py-2 text-gray-500 font-mono">{a.sequence_num}</td>
                    <td className="px-3 py-2 text-gray-300">{a.protocol}</td>
                    <td className="px-3 py-2 text-gray-200 text-right font-mono">{formatMs(a.page_load?.ttfb_ms)}</td>
                    <td className="px-3 py-2 text-gray-100 text-right font-mono font-bold">{formatMs(a.page_load?.total_ms)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right">{a.page_load?.asset_count ?? '-'}</td>
                    <td className="px-3 py-2 text-gray-400 text-right">{a.page_load?.assets_fetched ?? '-'}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{a.page_load?.total_bytes ? formatBytes(a.page_load.total_bytes) : '-'}</td>
                    <td className="px-3 py-2 text-gray-400 text-right">{a.page_load?.connections_opened ?? '-'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* TLS Details */}
      {attempts.some(a => a.tls) && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">TLS Details</h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-3 py-2 text-left">#</th>
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-left">Version</th>
                  <th className="px-3 py-2 text-left">Cipher</th>
                  <th className="px-3 py-2 text-right">Handshake</th>
                </tr>
              </thead>
              <tbody>
                {attempts.filter(a => a.tls).map(a => (
                  <tr key={a.attempt_id} className="border-b border-gray-800/30">
                    <td className="px-3 py-2 text-gray-500 font-mono">{a.sequence_num}</td>
                    <td className="px-3 py-2 text-gray-300">{a.protocol}</td>
                    <td className="px-3 py-2 text-gray-400">{a.tls?.protocol_version}</td>
                    <td className="px-3 py-2 text-gray-400 font-mono text-[11px]">{a.tls?.cipher_suite}</td>
                    <td className="px-3 py-2 text-gray-200 text-right font-mono">{formatMs(a.tls?.handshake_duration_ms)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* All Probes (compact) */}
      {attempts.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 uppercase tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            All Probes ({attempts.length})
          </h3>
          <div className="max-h-72 overflow-y-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-gray-500 border-b border-gray-800">
                  <th className="px-3 py-2 text-left">#</th>
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-left">Status</th>
                  <th className="px-3 py-2 text-right">DNS</th>
                  <th className="px-3 py-2 text-right">TCP</th>
                  <th className="px-3 py-2 text-right">TLS</th>
                  <th className="px-3 py-2 text-right">TTFB</th>
                  <th className="px-3 py-2 text-right">Total</th>
                  <th className="px-3 py-2 text-left">Detail</th>
                </tr>
              </thead>
              <tbody>
                {attempts.map((a) => (
                  <tr key={a.attempt_id} className="border-b border-gray-800/30 hover:bg-gray-800/20">
                    <td className="px-3 py-1.5 text-gray-500 font-mono">{a.sequence_num}</td>
                    <td className="px-3 py-1.5 text-gray-300">{a.protocol}</td>
                    <td className="px-3 py-1.5">
                      {a.success ? <span className="text-green-400">OK</span> : <span className="text-red-400">FAIL</span>}
                    </td>
                    <td className="px-3 py-1.5 text-gray-500 text-right font-mono">{formatMs(a.dns?.duration_ms)}</td>
                    <td className="px-3 py-1.5 text-gray-500 text-right font-mono">{formatMs(a.tcp?.connect_duration_ms)}</td>
                    <td className="px-3 py-1.5 text-gray-500 text-right font-mono">{formatMs(a.tls?.handshake_duration_ms)}</td>
                    <td className="px-3 py-1.5 text-gray-200 text-right font-mono">
                      {formatMs(a.http?.ttfb_ms ?? a.browser?.ttfb_ms ?? a.page_load?.ttfb_ms)}
                    </td>
                    <td className="px-3 py-1.5 text-gray-100 text-right font-mono font-bold">
                      {formatMs(a.http?.total_duration_ms ?? a.browser?.load_ms ?? a.page_load?.total_ms)}
                    </td>
                    <td className="px-3 py-1.5 text-gray-600 max-w-48 truncate" title={a.error?.message}>
                      {a.error ? (
                        <span className="text-red-400">{a.error.message}</span>
                      ) : a.http?.negotiated_version ? (
                        a.http.negotiated_version
                      ) : a.browser?.protocol ? (
                        `${a.browser.protocol} ${a.browser.resource_count ?? 0}res`
                      ) : a.tls?.protocol_version ? (
                        `${a.tls.protocol_version} ${a.tls.cipher_suite?.split('_').slice(-2).join('_') ?? ''}`
                      ) : a.dns?.resolved_ips ? (
                        a.dns.resolved_ips.join(', ')
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Errors */}
      {attempts.some(a => a.error) && (
        <div className="border border-red-500/15 rounded mb-6 overflow-hidden">
          <h3 className="px-4 py-2.5 text-xs text-red-400 uppercase tracking-wider bg-red-500/5 border-b border-red-500/15 font-medium">
            Errors ({attempts.filter(a => a.error).length})
          </h3>
          <div className="p-3 space-y-2">
            {attempts.filter(a => a.error).map(a => (
              <div key={a.attempt_id} className="text-xs">
                <span className="text-gray-500 font-mono">#{a.sequence_num} [{a.protocol}]</span>
                <span className="text-red-400 ml-2">{a.error?.message}</span>
                {a.error?.detail && <span className="text-gray-600 ml-2">{a.error.detail}</span>}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Job metadata */}
      <div className="section-divider">
        <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-3 font-medium">Test Details</h3>
        <div className="grid grid-cols-2 gap-2 text-xs">
          <div className="text-gray-500">Test ID</div>
          <div className="text-gray-300 font-mono">{job.job_id}</div>
          <div className="text-gray-500">Status</div>
          <div><StatusBadge status={job.status} /></div>
          <div className="text-gray-500">Target</div>
          <div className="text-gray-300">{job.config?.target}</div>
          <div className="text-gray-500">Modes</div>
          <div className="text-gray-300">{job.config?.modes?.join(', ')}</div>
          <div className="text-gray-500">Runs</div>
          <div className="text-gray-300">{job.config?.runs}</div>
          <div className="text-gray-500">Created</div>
          <div className="text-gray-300">{new Date(job.created_at).toLocaleString()}</div>
          {job.started_at && (
            <>
              <div className="text-gray-500">Started</div>
              <div className="text-gray-300">{new Date(job.started_at).toLocaleString()}</div>
            </>
          )}
          {job.finished_at && (
            <>
              <div className="text-gray-500">Finished</div>
              <div className="text-gray-300">{new Date(job.finished_at).toLocaleString()}</div>
            </>
          )}
          {job.run_id && (
            <>
              <div className="text-gray-500">Run ID</div>
              <div className="font-mono">
                <Link to={`/runs/${job.run_id}`} className="text-cyan-400 hover:underline">
                  {job.run_id}
                </Link>
              </div>
            </>
          )}
          {job.agent_id && (
            <>
              <div className="text-gray-500">Agent ID</div>
              <div className="text-gray-300 font-mono">{job.agent_id.slice(0, 12)}</div>
            </>
          )}
          {job.error_message && (
            <>
              <div className="text-gray-500">Error</div>
              <div className="text-red-400">{job.error_message}</div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
