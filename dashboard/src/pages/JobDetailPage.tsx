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
  formatMs,
  formatMetricValue,
  formatBytes,
  successRateClass,
} from '../lib/analysis';
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

export function JobDetailPage() {
  const { jobId } = useParams<{ jobId: string }>();
  const [job, setJob] = useState<Job | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const liveAttempts = useLiveStore((s) =>
    jobId ? s.liveAttempts[jobId] ?? EMPTY_ATTEMPTS : EMPTY_ATTEMPTS
  );
  const cleanupJob = useLiveStore((s) => s.cleanupJob);
  const addToast = useToast();

  const shortId = jobId?.slice(0, 8) ?? '';
  usePageTitle(jobId ? `Job ${shortId}` : 'Job');

  const isTerminal = job ? TERMINAL_STATUSES.has(job.status) : false;

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
  const protocolStats = useMemo(() => computeProtocolStats(liveAttempts), [liveAttempts]);
  const timingBreakdown = useMemo(() => computeTimingBreakdown(liveAttempts), [liveAttempts]);

  // Build chart data from live attempts
  const chartData = useMemo(
    () =>
      liveAttempts.map((a, i) => ({
        seq: i,
        protocol: a.protocol,
        success: a.success ? 1 : 0,
        ttfb_ms: a.http?.ttfb_ms ?? 0,
        total_ms: a.http?.total_duration_ms ?? 0,
      })),
    [liveAttempts]
  );

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
        <Breadcrumb items={[{ label: 'Jobs', to: '/jobs' }, { label: `Job ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading job {shortId}...</div>
      </div>
    );
  }

  if (error && !job) {
    return (
      <div className="p-6">
        <Breadcrumb items={[{ label: 'Jobs', to: '/jobs' }, { label: `Job ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load job</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
          <p className="text-gray-500 text-xs mt-2">Job ID: {jobId}</p>
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
      <Breadcrumb items={[{ label: 'Jobs', to: '/jobs' }, { label: `Job ${shortId}` }]} />

      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <div className="flex items-center gap-3 mb-1">
            <h2 className="text-xl font-bold text-gray-100">
              Job {job.job_id.slice(0, 8)}
            </h2>
            <StatusBadge status={job.status} />
          </div>
          <p className="text-sm text-gray-500">
            Target: {job.config?.target} | Modes: {job.config?.modes?.join(', ')}
          </p>
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
        <div className="bg-[#12131a] border border-cyan-500/30 rounded-lg p-4 mb-6 flex items-center gap-3">
          <span className="w-2 h-2 rounded-full bg-cyan-400 motion-safe:animate-pulse" />
          <span className="text-cyan-400 text-sm">
            Running... {liveAttempts.length} probes completed
          </span>
        </div>
      )}

      {/* Finished summary */}
      {isFinished && liveAttempts.length === 0 && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4 mb-6">
          <p className="text-gray-400 text-sm">
            Job {job.status}.
            {job.run_id && (
              <span className="ml-2 text-gray-500">
                Run ID: <span className="font-mono text-cyan-400">{job.run_id.slice(0, 8)}</span>
              </span>
            )}
          </p>
          {job.error_message && (
            <p className="text-red-400 text-sm mt-2">Error: {job.error_message}</p>
          )}
        </div>
      )}

      {/* Live TTFB chart */}
      {chartData.length > 0 && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4 mb-6">
          <h3 className="text-sm text-gray-400 mb-3">Probe Timing (ms)</h3>
          <ResponsiveContainer width="100%" height={250}>
            <BarChart data={chartData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis dataKey="seq" stroke="#4b5563" fontSize={10} />
              <YAxis stroke="#4b5563" fontSize={10} />
              <Tooltip
                contentStyle={{
                  background: '#12131a',
                  border: '1px solid #374151',
                  borderRadius: 6,
                  fontSize: 12,
                }}
              />
              <Bar dataKey="ttfb_ms" fill="#06b6d4" name="TTFB" />
              <Bar dataKey="total_ms" fill="#0e7490" name="Total" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* Timing Breakdown (shared analysis from lib/analysis.ts) */}
      {timingBreakdown.length > 0 && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg mb-6 overflow-hidden">
          <h3 className="px-4 py-3 text-sm text-gray-400 border-b border-gray-800 font-medium">
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
                    <td className="px-3 py-2 text-cyan-400 text-right font-mono">{formatMs(row.avgTtfb)}</td>
                    <td className="px-3 py-2 text-cyan-300 text-right font-mono font-bold">{formatMs(row.avgTotal)}</td>
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
        <div className="bg-[#12131a] border border-gray-800 rounded-lg mb-6 overflow-hidden">
          <h3 className="px-4 py-3 text-sm text-gray-400 border-b border-gray-800 font-medium">
            Statistics
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-500">
                  <th className="px-3 py-2 text-left">Protocol</th>
                  <th className="px-3 py-2 text-right">N</th>
                  <th className="px-3 py-2 text-right">Min</th>
                  <th className="px-3 py-2 text-right">p50</th>
                  <th className="px-3 py-2 text-right">p95</th>
                  <th className="px-3 py-2 text-right">Max</th>
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
                    <td className="px-3 py-2 text-cyan-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.p50)}</td>
                    <td className="px-3 py-2 text-yellow-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.p95)}</td>
                    <td className="px-3 py-2 text-gray-400 text-right font-mono">{formatMetricValue(ps.protocol, ps.stats.max)}</td>
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

      {/* Attempts table */}
      {liveAttempts.length > 0 && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg overflow-hidden mb-6">
          <h3 className="px-4 py-3 text-sm text-gray-400 border-b border-gray-800">
            Probe Results ({liveAttempts.length})
          </h3>
          <div className="max-h-96 overflow-y-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-xs text-gray-500 border-b border-gray-800">
                  <th className="px-4 py-2 text-left">#</th>
                  <th className="px-4 py-2 text-left">Protocol</th>
                  <th className="px-4 py-2 text-left">Status</th>
                  <th className="px-4 py-2 text-left">TTFB</th>
                  <th className="px-4 py-2 text-left">Total</th>
                </tr>
              </thead>
              <tbody>
                {liveAttempts.map((a) => (
                  <tr
                    key={a.attempt_id}
                    className="border-b border-gray-800/30 hover:bg-gray-800/20"
                  >
                    <td className="px-4 py-2 text-gray-500 font-mono text-xs">
                      {a.sequence_num}
                    </td>
                    <td className="px-4 py-2 text-gray-300">
                      {a.protocol}
                    </td>
                    <td className="px-4 py-2">
                      {a.success ? (
                        <span className="text-green-400">OK</span>
                      ) : (
                        <span className="text-red-400">FAIL</span>
                      )}
                    </td>
                    <td className="px-4 py-2 text-gray-400 font-mono text-xs">
                      {a.http?.ttfb_ms != null
                        ? `${a.http.ttfb_ms.toFixed(1)}ms`
                        : '-'}
                    </td>
                    <td className="px-4 py-2 text-gray-400 font-mono text-xs">
                      {a.http?.total_duration_ms != null
                        ? `${a.http.total_duration_ms.toFixed(1)}ms`
                        : '-'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Job metadata */}
      <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
        <h3 className="text-sm text-gray-400 mb-3">Job Details</h3>
        <div className="grid grid-cols-2 gap-2 text-xs">
          <div className="text-gray-500">Job ID</div>
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
