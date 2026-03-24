import { useState, useMemo, useEffect, useRef } from 'react';
import { useParams, Link } from 'react-router-dom';
import { api, type Job } from '../api/client';
import type { LiveAttempt, PacketCaptureSummary } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ShareDialog } from '../components/ShareDialog';
import { useLiveStore } from '../stores/liveStore';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { useProject } from '../hooks/useProject';
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
import { BoxWhiskerChart } from '../components/charts/BoxWhiskerChart';
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
  const { projectId, isProjectAdmin } = useProject();
  const { jobId } = useParams<{ jobId: string }>();
  const [job, setJob] = useState<Job | null>(null);
  const [dbAttempts, setDbAttempts] = useState<LiveAttempt[]>([]);
  const [runMeta, setRunMeta] = useState<{ client_version: string; client_os: string; endpoint_version: string | null } | null>(null);
  const [packetCapture, setPacketCapture] = useState<PacketCaptureSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [showShareDialog, setShowShareDialog] = useState(false);
  const liveAttempts = useLiveStore((s) =>
    jobId ? s.liveAttempts[jobId] ?? EMPTY_ATTEMPTS : EMPTY_ATTEMPTS
  );
  const jobLogs = useLiveStore((s) =>
    jobId ? s.jobLogs[jobId] ?? EMPTY_LOGS : EMPTY_LOGS
  );
  const cleanupJob = useLiveStore((s) => s.cleanupJob);
  const addToast = useToast();
  const testerLogRef = useRef<HTMLDivElement>(null);

  const shortId = jobId?.slice(0, 8) ?? '';
  usePageTitle(jobId ? `Test ${shortId}` : 'Test');

  const isTerminal = job ? TERMINAL_STATUSES.has(job.status) : false;

  // Load attempts and run metadata from DB when job completes with a run_id.
  // Retry briefly if run_id appears but DB returns empty (race: DB write in progress).
  useEffect(() => {
    if (!job?.run_id || !projectId) return;
    let cancelled = false;
    const fetchAttempts = (retries: number) => {
      api.getRunAttempts(projectId, job.run_id!).then((data) => {
        if (cancelled) return;
        const loaded = data as unknown as LiveAttempt[];
        if (loaded.length === 0 && retries > 0) {
          // DB write may still be in progress — retry after a short delay
          setTimeout(() => fetchAttempts(retries - 1), 1500);
        } else {
          setDbAttempts(loaded);
        }
      }).catch(() => {});
    };
    fetchAttempts(3);
    api.getRun(projectId, job.run_id).then((data) => {
      if (cancelled) return;
      setRunMeta({
        client_version: data.client_version,
        client_os: data.client_os,
        endpoint_version: data.endpoint_version,
      });
      if (data.packet_capture) {
        setPacketCapture(data.packet_capture);
      }
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [job?.run_id, projectId]);

  // Use live attempts while running; switch to DB attempts once loaded.
  // Keep live attempts visible until DB attempts are available to avoid blank flash.
  const attempts: LiveAttempt[] = dbAttempts.length > 0 ? dbAttempts : liveAttempts;

  // Clean up live attempts only after DB attempts are loaded
  useEffect(() => {
    if (isTerminal && jobId && dbAttempts.length > 0) {
      cleanupJob(jobId);
    }
  }, [isTerminal, jobId, dbAttempts.length, cleanupJob]);

  // Auto-scroll tester log to bottom
  useEffect(() => {
    if (testerLogRef.current) {
      testerLogRef.current.scrollTop = testerLogRef.current.scrollHeight;
    }
  }, [jobLogs]);

  // Poll job status while running; do one final fetch on completion to get run_id
  const [finalFetchDone, setFinalFetchDone] = useState(false);
  usePolling(
    () => {
      if (!jobId || !projectId) return;
      api
        .getJob(projectId, jobId)
        .then((j) => {
          setJob(j);
          setError(null);
          setLoading(false);
          if (TERMINAL_STATUSES.has(j.status)) setFinalFetchDone(true);
        })
        .catch((e) => {
          setError(String(e));
          setLoading(false);
        });
    },
    3000,
    !!jobId && (!isTerminal || !finalFetchDone || (isTerminal && !job?.run_id))
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
      .map(ps => ({
        label: ps.protocol + (ps.payloadBytes != null ? ` (${formatBytes(ps.payloadBytes)})` : ''),
        p5: ps.stats.p5,
        p25: ps.stats.p25,
        p50: ps.stats.p50,
        p75: ps.stats.p75,
        p95: ps.stats.p95,
      }));
  }, [protocolStats]);

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
      await api.cancelJob(projectId, jobId);
      addToast('success', `Job ${shortId} cancelled`);
    } catch (err) {
      addToast('error', err instanceof Error ? err.message : 'Failed to cancel job');
    }
  };

  if (loading && !job) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Tests', to: `/projects/${projectId}/tests` }, { label: `Test ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading job {shortId}...</div>
      </div>
    );
  }

  if (error && !job) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Tests', to: `/projects/${projectId}/tests` }, { label: `Test ${shortId}` }]} />
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
      <div className="p-4 md:p-6">
        <div className="text-gray-500">Job not found: {jobId}</div>
      </div>
    );
  }

  const isRunning = job.status === 'running' || job.status === 'assigned';
  const isFinished = TERMINAL_STATUSES.has(job.status);

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Tests', to: `/projects/${projectId}/tests` }, { label: `Test ${shortId}` }]} />

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
        <div className="flex items-center gap-2">
          {isProjectAdmin && jobId && (
            <button
              onClick={() => setShowShareDialog(true)}
              className="px-3 py-1.5 text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 rounded transition-colors border border-gray-700"
            >
              Share
            </button>
          )}
          {isRunning && (
            <button
              onClick={handleCancel}
              className="bg-red-600/20 text-red-400 border border-red-500/30 px-4 py-1.5 rounded text-sm hover:bg-red-600/30 transition-colors"
            >
              Cancel
            </button>
          )}
        </div>
      </div>

      {showShareDialog && jobId && (
        <ShareDialog
          projectId={projectId}
          resourceType="job"
          resourceId={jobId}
          onClose={() => setShowShareDialog(false)}
        />
      )}

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
                Run: <Link to={`/projects/${projectId}/runs/${job.run_id}`} className="font-mono text-cyan-400 hover:underline">{job.run_id.slice(0, 8)}</Link>
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
          <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">probe timing by attempt</h3>
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
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            timing breakdown
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
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            statistics summary
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

      {/* Box-and-Whisker: p5 ── [Q1 | median | Q3] ── p95 */}
      {boxPlotData.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">distribution by protocol</h3>
          <BoxWhiskerChart groups={boxPlotData} />
        </div>
      )}

      {/* TTFB Distribution */}
      {ttfbDistribution.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">timing distribution (ms)</h3>
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
          <h3 className="text-xs text-gray-500 tracking-wider mb-2 font-medium">analysis</h3>
          <ul className="space-y-1">
            {observations.map((obs, i) => (
              <li key={i} className="text-xs text-gray-400">
                {obs}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* Packet Capture */}
      {packetCapture && packetCapture.total_packets > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            packet capture
          </h3>
          <div className="p-4 space-y-4">
            {/* Summary bar */}
            <div className="flex flex-wrap items-center gap-x-5 gap-y-1 text-xs">
              <span className="text-gray-500">
                Status <span className={`font-mono font-semibold ml-1 ${packetCapture.capture_status === 'captured' ? 'text-green-400' : 'text-yellow-400'}`}>{packetCapture.capture_status}</span>
              </span>
              <span className="text-gray-500">
                Interface <span className="text-gray-300 font-mono ml-1">{packetCapture.interface}</span>
              </span>
              <span className="text-gray-500">
                Total <span className="text-gray-200 font-mono font-semibold ml-1">{packetCapture.total_packets.toLocaleString()}</span> packets
              </span>
              <span className="text-gray-500">
                Confidence <span className={`font-mono font-semibold ml-1 ${
                  packetCapture.capture_confidence === 'high' ? 'text-green-400' :
                  packetCapture.capture_confidence === 'medium' ? 'text-yellow-400' : 'text-red-400'
                }`}>{packetCapture.capture_confidence}</span>
              </span>
              <span className="text-gray-500">
                Mode <span className="text-gray-400 font-mono ml-1">{packetCapture.mode}</span>
              </span>
            </div>

            {/* Transport breakdown */}
            {packetCapture.transport_shares.length > 0 && (
              <div>
                <div className="text-xs text-gray-500 tracking-wider mb-2 font-medium">transport breakdown</div>
                <table className="w-full text-xs">
                  <thead>
                    <tr className="border-b border-gray-800 text-gray-500">
                      <th className="px-3 py-1.5 text-left">Protocol</th>
                      <th className="px-3 py-1.5 text-right">Packets</th>
                      <th className="px-3 py-1.5 text-right">% of Total</th>
                      <th className="px-3 py-1.5 text-left w-48">Distribution</th>
                    </tr>
                  </thead>
                  <tbody>
                    {packetCapture.transport_shares.map(share => (
                      <tr key={share.protocol} className="border-b border-gray-800/30">
                        <td className="px-3 py-1.5 text-gray-300 font-mono">{share.protocol}</td>
                        <td className="px-3 py-1.5 text-gray-200 text-right font-mono">{share.packets.toLocaleString()}</td>
                        <td className="px-3 py-1.5 text-gray-400 text-right font-mono">{share.pct_of_total.toFixed(1)}%</td>
                        <td className="px-3 py-1.5">
                          <div className="h-2 bg-gray-800 rounded overflow-hidden">
                            <div className="h-full bg-cyan-600 rounded" style={{ width: `${Math.min(share.pct_of_total, 100)}%` }} />
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            {/* TCP Health Indicators */}
            {(packetCapture.retransmissions > 0 || packetCapture.duplicate_acks > 0 || packetCapture.resets > 0) && (
              <div>
                <div className="text-xs text-gray-500 tracking-wider mb-2 font-medium">TCP health</div>
                <div className="flex flex-wrap gap-x-6 gap-y-1 text-xs">
                  <span className="text-gray-500">
                    Retransmissions <span className={`font-mono font-semibold ml-1 ${packetCapture.retransmissions > 0 ? 'text-yellow-400' : 'text-gray-600'}`}>{packetCapture.retransmissions.toLocaleString()}</span>
                  </span>
                  <span className="text-gray-500">
                    Duplicate ACKs <span className={`font-mono font-semibold ml-1 ${packetCapture.duplicate_acks > 0 ? 'text-yellow-400' : 'text-gray-600'}`}>{packetCapture.duplicate_acks.toLocaleString()}</span>
                  </span>
                  <span className="text-gray-500">
                    Resets <span className={`font-mono font-semibold ml-1 ${packetCapture.resets > 0 ? 'text-red-400' : 'text-gray-600'}`}>{packetCapture.resets.toLocaleString()}</span>
                  </span>
                </div>
              </div>
            )}

            {/* Target endpoint info */}
            {packetCapture.likely_target_endpoints.length > 0 && (
              <div>
                <div className="text-xs text-gray-500 tracking-wider mb-2 font-medium">target endpoints</div>
                <div className="flex flex-wrap gap-x-5 gap-y-1 text-xs">
                  <span className="text-gray-500">
                    Endpoints <span className="text-gray-300 font-mono ml-1">{packetCapture.likely_target_endpoints.join(', ')}</span>
                  </span>
                  <span className="text-gray-500">
                    Target packets <span className="text-gray-200 font-mono font-semibold ml-1">{packetCapture.likely_target_packets.toLocaleString()}</span>
                    <span className="text-gray-600 ml-1">({packetCapture.likely_target_pct_of_total.toFixed(1)}%)</span>
                  </span>
                  {packetCapture.dominant_trace_port != null && (
                    <span className="text-gray-500">
                      Dominant port <span className="text-gray-300 font-mono ml-1">{packetCapture.dominant_trace_port}</span>
                    </span>
                  )}
                </div>
              </div>
            )}

            {/* Top endpoints and ports side by side */}
            <div className="grid grid-cols-2 gap-4">
              {packetCapture.top_endpoints.length > 0 && (
                <div>
                  <div className="text-xs text-gray-500 tracking-wider mb-2 font-medium">top endpoints</div>
                  <table className="w-full text-xs">
                    <thead>
                      <tr className="border-b border-gray-800 text-gray-500">
                        <th className="px-2 py-1 text-left">Endpoint</th>
                        <th className="px-2 py-1 text-right">Packets</th>
                      </tr>
                    </thead>
                    <tbody>
                      {packetCapture.top_endpoints.map(ep => (
                        <tr key={ep.endpoint} className="border-b border-gray-800/30">
                          <td className="px-2 py-1 text-gray-300 font-mono">{ep.endpoint}</td>
                          <td className="px-2 py-1 text-gray-400 text-right font-mono">{ep.packets.toLocaleString()}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {packetCapture.top_ports.length > 0 && (
                <div>
                  <div className="text-xs text-gray-500 tracking-wider mb-2 font-medium">top ports</div>
                  <table className="w-full text-xs">
                    <thead>
                      <tr className="border-b border-gray-800 text-gray-500">
                        <th className="px-2 py-1 text-left">Port</th>
                        <th className="px-2 py-1 text-right">Packets</th>
                      </tr>
                    </thead>
                    <tbody>
                      {packetCapture.top_ports.map(p => (
                        <tr key={p.port} className="border-b border-gray-800/30">
                          <td className="px-2 py-1 text-gray-300 font-mono">{p.port}</td>
                          <td className="px-2 py-1 text-gray-400 text-right font-mono">{p.packets.toLocaleString()}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>

            {/* Observation flags */}
            <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
              {packetCapture.observed_quic && (
                <span className="text-cyan-400 font-mono">QUIC observed</span>
              )}
              {packetCapture.observed_tcp_only && (
                <span className="text-gray-400 font-mono">TCP only</span>
              )}
              {packetCapture.observed_mixed_transport && (
                <span className="text-yellow-400 font-mono">Mixed transport</span>
              )}
              {packetCapture.capture_may_be_ambiguous && (
                <span className="text-orange-400 font-mono">Ambiguous capture</span>
              )}
            </div>

            {/* Warnings */}
            {packetCapture.warnings.length > 0 && (
              <div className="border-l-2 border-yellow-600/50 pl-3 space-y-1">
                {packetCapture.warnings.map((w, i) => (
                  <div key={i} className="text-xs text-yellow-400/80">{w}</div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Tester Log */}
      {jobLogs.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            tester log
          </h3>
          <div ref={testerLogRef} className="h-[400px] overflow-y-auto p-3 font-mono text-xs leading-5">
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
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">browser results</h3>
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
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">page load results</h3>
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
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">TLS details</h3>
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
          <h3 className="px-4 py-2.5 text-xs text-gray-500 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            all probes ({attempts.length})
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
          <h3 className="px-4 py-2.5 text-xs text-red-400 tracking-wider bg-red-500/5 border-b border-red-500/15 font-medium">
            errors ({attempts.filter(a => a.error).length})
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
        <h3 className="text-xs text-gray-500 tracking-wider mb-3 font-medium">test details</h3>
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
                <Link to={`/projects/${projectId}/runs/${job.run_id}`} className="text-cyan-400 hover:underline">
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
