import { useState, useMemo } from 'react';
import { Link, useParams } from 'react-router';
import { api, errorMessage } from '../api/client';
import { stripAnsi } from '../lib/ansi';
import type { TestRun, LiveAttempt, BenchmarkArtifact, RunInfra } from '../api/types';
import { InfraEnvelope } from '../components/InfraEnvelope';
import { useProject } from '../hooks/useProject';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ExportMenu } from '../components/common/ExportMenu';
import { StatusBadge } from '../components/common/StatusBadge';
import { RunResult } from '../components/common/RunResult';
import { RunEnvelopeBlock } from '../components/RunEnvelopeBlock';
import { runDisplayStatus } from '../lib/runStatus';
import { ShareDialog } from '../components/ShareDialog';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useToast } from '../hooks/useToast';
import {
  computeProtocolStats,
  computeTimingBreakdown,
  computeStats,
  primaryMetricValue,
  latencyMetricValue,
  groupByProtocolAndPayload,
  formatMs,
  formatMetricValue,
  formatBytes,
  successRateClass,
  type ProtocolStats,
  type TimingBreakdown,
  type Stats,
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
  const addToast = useToast();
  const [run, setRun] = useState<TestRun | null>(null);
  const [attempts, setAttempts] = useState<LiveAttempt[]>([]);
  const [artifact, setArtifact] = useState<BenchmarkArtifact | null>(null);
  const [infra, setInfra] = useState<RunInfra | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Attempts failing must never dead-end the page when the run itself loaded
  // (audit F3: /attempts 404'd while the run returned 200) — track separately
  // and degrade that section instead.
  const [attemptsError, setAttemptsError] = useState<string | null>(null);
  const [expandedProtocols, setExpandedProtocols] = useState<Set<string>>(new Set());
  const [showShareDialog, setShowShareDialog] = useState(false);
  const [retryTick, setRetryTick] = useState(0);

  const shortId = runId?.slice(0, 8) ?? '';
  usePageTitle(runId ? `Run ${shortId}` : 'Run');

  usePolling(
    () => {
      if (!runId || !projectId) return;
      api
        .getTestRunAttempts(runId)
        .then((data) => {
          setAttempts(data as unknown as LiveAttempt[]);
          setAttemptsError(null);
          setLoading(false);
        })
        .catch((e) => { setAttemptsError(errorMessage(e)); setLoading(false); });
      api
        .getTestRun(runId)
        .then((data) => {
          setRun(data);
          setError(null);
          // Fetch artifact if present and not already loaded
          if (data.artifact_id && !artifact) {
            api.getTestRunArtifact(runId).then(setArtifact).catch(() => {});
          }
          // Infra facts are static per run — fetch once, degrade silently
          // (old control planes without the route → panel simply absent).
          if (!infra) {
            api.getTestRunInfra(runId).then(setInfra).catch(() => {});
          }
        })
        .catch((e) => { setError(errorMessage(e)); setLoading(false); });
    },
    15000,
    !!runId,
    retryTick
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

  // Latency distribution rows (one per protocol × payload) for the box plot.
  // Uses transfer-time ms for throughput modes so every row shares one ms axis
  // and per-payload rows ladder by transfer time. Sorted so each mode's payloads
  // read ascending (1 KB → 100 MB).
  const latencyDist = useMemo(() => {
    const groups = groupByProtocolAndPayload(attempts.filter((a) => a.success));
    const rows: { protocol: string; payloadBytes: number | null; stats: Stats }[] = [];
    for (const [key, atts] of groups) {
      const values = atts.map(latencyMetricValue).filter((v): v is number => v != null);
      const stats = computeStats(values);
      if (!stats) continue;
      const protocol = key.split(':')[0];
      const payloadStr = key.includes(':') ? key.split(':')[1] : null;
      rows.push({ protocol, payloadBytes: payloadStr ? parseInt(payloadStr, 10) : null, stats });
    }
    rows.sort((a, b) =>
      a.protocol === b.protocol
        ? (a.payloadBytes ?? 0) - (b.payloadBytes ?? 0)
        : a.protocol.localeCompare(b.protocol)
    );
    return rows;
  }, [attempts]);

  // Log-scaled axis for the box plot: latency spans several orders of magnitude
  // (sub-ms probes → multi-second 100 MB transfers), so a linear axis crushes
  // everything small into a sliver at the left. log10 gives every row visible
  // width and turns the payload ladder into evenly-spaced steps. `decades` are
  // the 1/10/100/1000ms gridlines that fall inside the data range.
  const latencyAxis = useMemo(() => {
    const mins = latencyDist.map((r) => r.stats.min).filter((v) => v > 0);
    const minV = mins.length ? Math.min(...mins) : 0.1;
    const maxV = Math.max(minV * 10, ...latencyDist.map((r) => r.stats.max));
    const logMin = Math.log10(minV);
    const logMax = Math.log10(maxV);
    const span = logMax - logMin || 1;
    const scale = (v: number) =>
      v > 0 ? Math.min(100, Math.max(0, ((Math.log10(v) - logMin) / span) * 100)) : 0;
    const decades: number[] = [];
    for (let e = Math.ceil(logMin - 1e-9); e <= Math.floor(logMax + 1e-9); e++) {
      decades.push(Math.pow(10, e));
    }
    return { scale, decades };
  }, [latencyDist]);

  const toggleProtocol = (protocol: string) => {
    setExpandedProtocols((prev) => {
      const next = new Set(prev);
      if (next.has(protocol)) { next.delete(protocol); } else { next.add(protocol); }
      return next;
    });
  };

  if (loading && attempts.length === 0 && !run) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />
        <div className="text-gray-400 motion-safe:animate-pulse">Loading run {shortId}...</div>
      </div>
    );
  }

  // Page-fatal only when the run itself failed to load. An attempts-only
  // failure renders the run with a degraded probe-details section below.
  if (error && !run) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load run {shortId}</h3>
          <p className="text-red-300 text-sm mb-3">{error}</p>
          <div className="flex items-center gap-3">
            <button
              onClick={() => setRetryTick(t => t + 1)}
              className="px-3 py-1.5 text-xs bg-cyan-600 hover:bg-cyan-500 text-white rounded transition-colors"
            >
              Retry
            </button>
            <Link
              to={`/projects/${projectId}/runs`}
              className="px-3 py-1.5 text-xs border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600 rounded transition-colors"
            >
              Back to runs
            </Link>
          </div>
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
          <div className="flex items-center gap-3 mb-1">
            <h2 className="text-xl font-bold text-gray-100">Run {shortId}</h2>
            {run && <StatusBadge status={runDisplayStatus(run)} />}
            {run?.artifact_id && (
              <span className="text-[10px] text-gray-300 bg-gray-500/10 px-1.5 py-0.5 rounded">benchmark</span>
            )}
          </div>
          <p className="text-sm text-gray-400">
            {run?.config_name && <>Config: <span className="text-gray-300">{run.config_name}</span> · </>}
            {run?.modes && <>Modes: <span className="text-gray-300">{run.modes.join(', ')}</span> · </>}
            {attempts.length} attempts
          </p>
          {/* Run-envelope context (V046 pass-through) — data-gated: old runs
              have no envelope and render nothing here. */}
          <RunEnvelopeBlock envelope={run?.envelope} />
        </div>
        <div className="flex items-center gap-2">
          {run && (run.status === 'queued' || run.status === 'running') && (
            <button
              onClick={() => {
                if (!runId) return;
                api.cancelTestRun(runId)
                  .then(() => addToast('info', `Run ${shortId} cancel requested`))
                  .catch((e) => addToast('error', `Failed to cancel: ${e instanceof Error ? e.message : String(e)}`));
              }}
              className="px-3 py-1.5 text-xs bg-red-500/10 hover:bg-red-500/20 text-red-400 rounded transition-colors border border-red-500/30"
            >
              Cancel
            </button>
          )}
          {/* Read-only document export — available to every role that can see
              the run (same visibility as the page itself). */}
          {runId && (
            <ExportMenu path={`/v2/test-runs/${runId}/report`} fileBase={`test-run-${shortId}`} />
          )}
          {isProjectAdmin && runId && (
            <button
              onClick={() => setShowShareDialog(true)}
              className="px-3 py-1.5 text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 rounded transition-colors border border-gray-700"
            >
              Share
            </button>
          )}
        </div>
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
        <span className="text-gray-400">
          Probes <span className="text-gray-200 font-mono font-semibold ml-1">{attempts.length}</span>
        </span>
        <span className="text-gray-400">
          Success <span className="text-green-400 font-mono font-semibold ml-1">{successCount}</span>
        </span>
        <span className="text-gray-400">
          Failed <span className={`font-mono font-semibold ml-1 ${failureCount > 0 ? 'text-red-400' : 'text-gray-500'}`}>{failureCount}</span>
        </span>
        <span className="text-gray-400">
          Rate <span className={`font-mono font-semibold ml-1 ${successRateClass(attempts.length > 0 ? (successCount / attempts.length) * 100 : 100)}`}>
            {attempts.length > 0 ? `${((successCount / attempts.length) * 100).toFixed(0)}%` : '-'}
          </span>
        </span>
      </div>

      {/* ── Infrastructure Envelope (expected vs measured + verdict) ── */}
      <InfraEnvelope infra={infra} attempts={attempts} envelope={run?.envelope} />

      {/* ── Timing Breakdown Table (mirrors HTML report) ── */}
      {timingBreakdown.length > 0 && (
        <div className="table-container mb-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            timing breakdown by protocol
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-400">
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
          <h3 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            statistics summary
          </h3>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-400">
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
          {protocolStats.some((ps) => ps.payloadBytes != null) && (
            <p className="px-4 py-2 text-[10px] text-gray-500 border-t border-gray-800/50 leading-relaxed">
              throughput is measured per direction — download over the body-receive window, upload over
              the send window (cross-checked against the server&apos;s Server-Timing clock). upload and
              download legitimately differ on asymmetric paths: cloud VMs cap egress, so a small target
              serves downloads slower than it accepts uploads. small payloads reflect TCP slow-start
              burst, not steady-state bandwidth.
            </p>
          )}
        </div>
      )}

      {/* ── Protocol Comparison Chart ── */}
      {protocolChartData.length > 1 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">protocol comparison — p50 vs p95</h3>
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
      {latencyDist.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">latency distribution — box &amp; whisker</h3>
          <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
            <div className="relative">
              {/* Decade gridlines behind the bars, aligned to the bar column */}
              <div className="absolute inset-y-0 pointer-events-none" style={{ left: 'calc(10rem + 0.75rem)', right: 'calc(6rem + 0.75rem)' }}>
                {latencyAxis.decades.map((t) => (
                  <div key={t} className="absolute top-0 bottom-0 w-px bg-gray-800" style={{ left: `${latencyAxis.scale(t)}%` }} />
                ))}
              </div>
              <div className="space-y-3 relative">
                {latencyDist.map((r) => {
                  const s = r.stats;
                  const scale = latencyAxis.scale;
                  const whiskerLeft = scale(s.min);
                  const boxLeft = scale(s.p25);
                  const median = scale(s.p50);
                  const boxRight = scale(s.p75);
                  const whiskerRight = scale(s.max);
                  return (
                    <div key={`${r.protocol}:${r.payloadBytes ?? ''}`} className="flex items-center gap-3">
                      <div className="w-40 text-xs font-mono text-right shrink-0 truncate">
                        <span className="text-gray-300">{r.protocol}</span>
                        {r.payloadBytes != null && <span className="text-gray-500"> · {formatBytes(r.payloadBytes)}</span>}
                      </div>
                      <div className="flex-1 relative h-6">
                        {/* Whisker line (min to max) */}
                        <div className="absolute top-1/2 -translate-y-1/2 h-px bg-gray-600" style={{ left: `${whiskerLeft}%`, width: `${Math.max(whiskerRight - whiskerLeft, 0)}%` }} />
                        {/* Min tick */}
                        <div className="absolute top-1 bottom-1 w-px bg-gray-500" style={{ left: `${whiskerLeft}%` }} />
                        {/* Max tick */}
                        <div className="absolute top-1 bottom-1 w-px bg-gray-500" style={{ left: `${whiskerRight}%` }} />
                        {/* Box (p25 to p75) */}
                        <div className="absolute top-0.5 bottom-0.5 rounded-sm border border-cyan-600/60 bg-cyan-900/30" style={{ left: `${boxLeft}%`, width: `${Math.max(boxRight - boxLeft, 0.5)}%` }} />
                        {/* Median line */}
                        <div className="absolute top-0 bottom-0 w-0.5 bg-cyan-400" style={{ left: `${median}%` }} />
                      </div>
                      <div className="w-24 text-xs text-gray-400 font-mono shrink-0">
                        {formatMs(s.min)}&ndash;{formatMs(s.max)}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
            {/* Log-scale decade tick labels */}
            <div className="relative h-4 mt-2 text-[10px] text-gray-500" style={{ marginLeft: 'calc(10rem + 0.75rem)', marginRight: 'calc(6rem + 0.75rem)' }}>
              {latencyAxis.decades.map((t) => (
                <span key={t} className="absolute -translate-x-1/2 whitespace-nowrap" style={{ left: `${latencyAxis.scale(t)}%` }}>{formatMs(t)}</span>
              ))}
            </div>
            <div className="flex items-center gap-4 mt-3 text-[10px] text-gray-500 px-[calc(10rem+0.75rem)]">
              <span className="flex items-center gap-1"><span className="w-3 h-px bg-gray-500 inline-block" /> whisker (min/max)</span>
              <span className="flex items-center gap-1"><span className="w-3 h-3 rounded-sm border border-cyan-600/60 bg-cyan-900/30 inline-block" /> IQR (p25–p75)</span>
              <span className="flex items-center gap-1"><span className="w-0.5 h-3 bg-cyan-400 inline-block" /> median (p50)</span>
              <span className="ml-auto text-gray-600">log scale · throughput modes shown as transfer time</span>
            </div>
          </div>
        </div>
      )}

      {/* ── TTFB Distribution ── */}
      {ttfbDistribution.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">TTFB distribution (ms)</h3>
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
      <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">probe details</h3>
      {attemptsError && attempts.length === 0 && (
        <div className="border border-gray-800 rounded p-6 text-center mb-2">
          <p className="text-gray-400 text-sm mb-1">Attempt data unavailable</p>
          <p className="text-gray-500 text-xs mb-3">{attemptsError}</p>
          <button
            onClick={() => setRetryTick(t => t + 1)}
            className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors"
          >
            Retry
          </button>
        </div>
      )}
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
                <span className="text-gray-400 text-xs transition-transform" style={{ transform: isExpanded ? 'rotate(90deg)' : '' }} aria-hidden="true">{'\u25B6'}</span>
                <span className="text-gray-200 font-medium text-sm">{protocol.toUpperCase()}</span>
                <span className="text-gray-400 text-xs">{group.length} attempts</span>
                {stats && (
                  <span className="text-gray-500 text-xs">
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

      {/* ── Benchmark Artifact (methodology runs only) ── */}
      {artifact && <ArtifactSection artifact={artifact} />}

      {/* ── Live Progress (queued/running runs) ── */}
      {run && (run.status === 'queued' || run.status === 'running') && (
        <div className="table-container mb-6 mt-6">
          <h3 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            live progress
          </h3>
          <div className="px-4 py-4 text-sm">
            <div className="flex items-center gap-3">
              <span className="w-2 h-2 rounded-full bg-cyan-400 motion-safe:animate-pulse" />
              <span className="text-gray-300">
                {run.success_count + run.failure_count} attempts completed
              </span>
              <RunResult ok={run.success_count} fail={run.failure_count} className="text-xs" />
            </div>
            {run.error_message && (
              <p className="text-red-400 text-xs mt-2 font-mono">{stripAnsi(run.error_message)}</p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ─── Artifact Section (merged from BenchmarkDetailPage) ─────────────────────

function ArtifactSection({ artifact }: { artifact: BenchmarkArtifact }) {
  return (
    <div className="mt-8 border-t border-purple-500/20 pt-6">
      <h3 className="text-sm font-bold text-purple-400 mb-4 flex items-center gap-2">
        <span className="text-purple-400/60">&#9670;</span> Benchmark Artifact
      </h3>

      {/* Data Quality Summary */}
      {artifact.data_quality && (
        <div className="border border-gray-800 rounded p-4 mb-4 text-xs">
          <div className="flex flex-wrap gap-x-6 gap-y-2">
            <span className="text-gray-400">
              Noise: <span className={artifact.data_quality.noise_level === 'low' ? 'text-green-400' : 'text-yellow-400'}>
                {artifact.data_quality.noise_level}
              </span>
            </span>
            <span className="text-gray-400">
              Sufficiency: <span className={artifact.data_quality.sufficiency === 'sufficient' ? 'text-green-400' : 'text-yellow-400'}>
                {artifact.data_quality.sufficiency}
              </span>
            </span>
            <span className="text-gray-400">
              Publication: <span className={artifact.data_quality.publication_ready ? 'text-green-400' : 'text-red-400'}>
                {artifact.data_quality.publication_ready ? 'Ready' : 'Not Ready'}
              </span>
            </span>
            {artifact.data_quality.quality_tier && (
              <span className="text-gray-400">
                Tier: <span className="text-gray-300">{artifact.data_quality.quality_tier}</span>
              </span>
            )}
          </div>
          {artifact.data_quality.warnings.length > 0 && (
            <div className="mt-2 space-y-1">
              {artifact.data_quality.warnings.map((w, i) => (
                <p key={i} className="text-yellow-400/80">&#9888; {w}</p>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Case Summaries */}
      {artifact.summaries && artifact.summaries.length > 0 && (
        <div className="table-container mb-4">
          <h4 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            case summaries
          </h4>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-400">
                  <th className="px-4 py-2 text-left">Protocol</th>
                  <th className="px-4 py-2 text-left">Metric</th>
                  <th className="px-4 py-2 text-right">N</th>
                  <th className="px-4 py-2 text-right">p50</th>
                  <th className="px-4 py-2 text-right">p95</th>
                  <th className="px-4 py-2 text-right">p99</th>
                  <th className="px-4 py-2 text-right">RPS</th>
                  <th className="px-4 py-2 text-right">StdDev</th>
                </tr>
              </thead>
              <tbody>
                {(Array.isArray(artifact.summaries) ? artifact.summaries : [artifact.summaries]).map((s, i) => (
                  <tr key={i} className="border-b border-gray-800/30 hover:bg-gray-800/10">
                    <td className="px-4 py-2 text-gray-200">{s.protocol}</td>
                    <td className="px-4 py-2 text-gray-400">{s.metric_name} ({s.metric_unit})</td>
                    <td className="px-4 py-2 text-gray-400 text-right">{s.included_sample_count}</td>
                    <td className="px-4 py-2 text-gray-100 text-right font-mono">{s.p50.toFixed(2)}</td>
                    <td className="px-4 py-2 text-yellow-400 text-right font-mono">{s.p95.toFixed(2)}</td>
                    <td className="px-4 py-2 text-orange-400 text-right font-mono">{s.p99.toFixed(2)}</td>
                    <td className="px-4 py-2 text-gray-300 text-right font-mono">{s.rps.toFixed(0)}</td>
                    <td className="px-4 py-2 text-gray-400 text-right font-mono">{s.stddev.toFixed(2)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Methodology */}
      {artifact.methodology && (
        <div className="border border-gray-800 rounded p-4 text-xs text-gray-400 space-y-1">
          <p className="text-gray-400 font-medium mb-2">Methodology</p>
          <p>Mode: {artifact.methodology.mode} | Phase model: {artifact.methodology.phase_model}</p>
          <p>Scenario: {artifact.methodology.scenario} | Sample phase: {artifact.methodology.sample_phase}</p>
          <p>Launches: {artifact.methodology.launch_count} | Phases: {artifact.methodology.phases_present?.join(', ')}</p>
        </div>
      )}
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
        {ps.payloadBytes != null && <span className="text-gray-400 ml-1">({formatBytes(ps.payloadBytes)})</span>}
      </td>
      <td className="px-4 py-2 text-gray-400">{ps.label}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{ps.stats.count}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.min)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.mean)}</td>
      <td className="px-4 py-2 text-gray-100 text-right font-mono font-semibold">{fmt(ps.stats.p50)}</td>
      <td className="px-4 py-2 text-yellow-400 text-right font-mono">{fmt(ps.stats.p95)}</td>
      <td className="px-4 py-2 text-orange-400 text-right font-mono">{fmt(ps.stats.p99)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.max)}</td>
      <td className="px-4 py-2 text-gray-400 text-right font-mono">{fmt(ps.stats.stddev)}</td>
      <td className={`px-4 py-2 text-right font-mono ${successRateClass(ps.successRate)}`}>
        {ps.successRate.toFixed(0)}%
      </td>
    </tr>
  );
}

// Exported for RunDetailPage.attempts.test.tsx — renders one probe attempt's
// per-phase cards. Every row below the core timings is conditional: older runs
// (pre phase-detail widening) simply render fewer lines.
export function AttemptRow({ a }: { a: LiveAttempt }) {
  const st = a.server_timing;
  const hasSplit = st != null && (st.server_ms != null || st.network_ms != null || st.app_ms != null);
  const hasServerTimings = st != null && (hasSplit || st.processing_ms != null || st.total_server_ms != null);
  return (
    <div className="px-4 py-3 border-b border-gray-800/30 hover:bg-gray-800/10">
      <div className="flex items-center gap-4 mb-2">
        <span className="text-gray-400 font-mono text-xs w-8">#{a.sequence_num}</span>
        {a.success
          ? <span className="text-green-400 text-xs font-medium">OK</span>
          : <span className="text-red-400 text-xs font-medium">FAIL</span>
        }
        {a.retry_count > 0 && <span className="text-gray-500 text-xs">{a.retry_count} retries</span>}
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-2 text-xs">
        {a.dns && (
          <SubResult label="DNS" color="gray">
            <p className="text-gray-300">{formatMs(a.dns.duration_ms)}</p>
            {a.dns.resolved_ips?.length > 0 && (
              <p className="text-gray-400 font-mono truncate">{a.dns.resolved_ips.join(', ')}</p>
            )}
          </SubResult>
        )}
        {a.tcp && (
          <SubResult label="TCP" color="gray">
            <p className="text-gray-300">{formatMs(a.tcp.connect_duration_ms)}</p>
            <p className="text-gray-400 font-mono truncate">{a.tcp.remote_addr}</p>
            {(a.tcp.total_retrans != null || a.tcp.congestion_algorithm != null || a.tcp.rtt_estimate_ms != null) && (
              <p className="font-mono truncate">
                {a.tcp.total_retrans != null && (
                  <span className={a.tcp.total_retrans > 0 ? 'text-yellow-400' : 'text-gray-400'}>
                    {a.tcp.total_retrans} retrans
                  </span>
                )}
                {a.tcp.congestion_algorithm != null && (
                  <span className="text-gray-400">{a.tcp.total_retrans != null ? ' · ' : ''}{a.tcp.congestion_algorithm}</span>
                )}
                {a.tcp.rtt_estimate_ms != null && (
                  <span className="text-gray-400"> · rtt {formatMs(a.tcp.rtt_estimate_ms)}</span>
                )}
              </p>
            )}
          </SubResult>
        )}
        {a.tls && (
          <SubResult label="TLS" color="gray">
            <p className="text-gray-300">{formatMs(a.tls.handshake_duration_ms)}</p>
            <p className="text-gray-400 font-mono truncate">{a.tls.protocol_version} · {a.tls.cipher_suite}</p>
            {(a.tls.handshake_kind != null || a.tls.resumed != null || a.tls.alpn_negotiated != null) && (
              <p className="font-mono truncate">
                {(a.tls.handshake_kind != null || a.tls.resumed != null) && (
                  <span className={(a.tls.resumed ?? a.tls.handshake_kind === 'resumed') ? 'text-cyan-400' : 'text-gray-400'}>
                    {a.tls.handshake_kind ?? (a.tls.resumed ? 'resumed' : 'full')}
                  </span>
                )}
                {a.tls.alpn_negotiated != null && (
                  <span className="text-gray-400">
                    {(a.tls.handshake_kind != null || a.tls.resumed != null) ? ' · ' : ''}alpn {a.tls.alpn_negotiated}
                  </span>
                )}
              </p>
            )}
          </SubResult>
        )}
        {a.http && (
          <SubResult label="HTTP" color="cyan">
            <p className="text-gray-300">
              <span className={a.http.status_code >= 400 ? 'text-red-400' : 'text-green-400'}>{a.http.status_code}</span>
              {' · '}TTFB {formatMs(a.http.ttfb_ms)} · Total {formatMs(a.http.total_duration_ms)}
            </p>
            <p className="text-gray-400 font-mono truncate">
              {a.http.negotiated_version}
              {a.http.throughput_mbps != null && ` · ${a.http.throughput_mbps.toFixed(1)} MB/s`}
              {a.http.goodput_mbps != null && ` · goodput ${a.http.goodput_mbps.toFixed(1)} MB/s`}
              {a.http.payload_bytes != null && a.http.payload_bytes > 0 && ` · ${formatBytes(a.http.payload_bytes)}`}
            </p>
          </SubResult>
        )}
        {a.udp && (
          <SubResult label="UDP" color="gray">
            <p className="text-gray-300">
              RTT avg {formatMs(a.udp.rtt_avg_ms)} · Loss {a.udp.loss_percent.toFixed(1)}%
              {a.udp.jitter_ms != null && ` · Jitter ${formatMs(a.udp.jitter_ms)}`}
            </p>
            <p className="text-gray-400">
              {a.udp.probe_count} probes
              {a.udp.rtt_p95_ms != null && ` · p95 ${formatMs(a.udp.rtt_p95_ms)}`}
            </p>
          </SubResult>
        )}
        {hasServerTimings && st && (
          <SubResult label="Server" color="cyan">
            {hasSplit ? (
              <>
                <p className="text-gray-300">
                  {st.server_ms != null && <>Server {formatMs(st.server_ms)}</>}
                  {st.server_ms != null && st.network_ms != null && ' · '}
                  {st.network_ms != null && <>Network {formatMs(st.network_ms)}</>}
                  {st.split_anomaly && (
                    <span className="text-yellow-400"> · split anomaly</span>
                  )}
                </p>
                {st.app_ms != null && <p className="text-gray-400 font-mono">app {formatMs(st.app_ms)}</p>}
              </>
            ) : (
              <p className="text-gray-300">
                {st.processing_ms != null && <>Proc {formatMs(st.processing_ms)}</>}
                {st.processing_ms != null && st.total_server_ms != null && ' · '}
                {st.total_server_ms != null && <>Total {formatMs(st.total_server_ms)}</>}
              </p>
            )}
          </SubResult>
        )}
        {a.rpm && (
          <SubResult label="RPM" color="cyan">
            <p className="text-gray-300">
              RTT {formatMs(a.rpm.unloaded_rtt_avg_ms)} &rarr; {formatMs(a.rpm.loaded_rtt_avg_ms)} under load
              {a.rpm.rpm != null && ` · ${a.rpm.rpm.toFixed(0)} RPM`}
            </p>
            <p className="font-mono truncate">
              {a.rpm.bufferbloat_factor != null && (
                <span className={a.rpm.bufferbloat_factor >= 2 ? 'text-yellow-400' : 'text-gray-400'}>
                  bufferbloat &times;{a.rpm.bufferbloat_factor.toFixed(2)}
                </span>
              )}
              {a.rpm.load_throughput_mbps != null && (
                <span className="text-gray-400">
                  {a.rpm.bufferbloat_factor != null ? ' · ' : ''}load {a.rpm.load_throughput_mbps.toFixed(1)} MB/s
                </span>
              )}
            </p>
          </SubResult>
        )}
        {a.ping && (
          <SubResult label="Ping" color="gray">
            <p className="text-gray-300">
              RTT avg {formatMs(a.ping.rtt_avg_ms)} · Jitter {formatMs(a.ping.jitter_ms)} · Loss {a.ping.loss_percent.toFixed(1)}%
            </p>
            <p className="text-gray-400">
              {a.ping.probe_count} probes
              {a.ping.reply_ttl != null && ` · ttl ${a.ping.reply_ttl}`}
            </p>
          </SubResult>
        )}
        {a.path && (
          <SubResult label="Path" color="gray">
            <p className="text-gray-300">
              {a.path.hop_count != null ? `${a.path.hop_count} hops` : 'hops unknown'}
              {' · '}
              <span className={a.path.destination_reached ? 'text-green-400' : 'text-yellow-400'}>
                {a.path.destination_reached ? 'reached' : 'not reached'}
              </span>
              {a.path.destination_rtt_ms != null && ` · ${formatMs(a.path.destination_rtt_ms)}`}
            </p>
            <p className="text-gray-400 font-mono truncate">{a.path.method}</p>
            {a.path.hops.length > 0 && (
              <p className="text-gray-400 font-mono truncate">
                {a.path.hops.map((h) => h.addr ?? '*').join(' → ')}
              </p>
            )}
          </SubResult>
        )}
        {a.dualstack && (
          <SubResult label="Dual Stack" color="cyan">
            <p className="text-gray-300">
              v4 {a.dualstack.ipv4.success && a.dualstack.ipv4.total_ms != null
                ? formatMs(a.dualstack.ipv4.total_ms)
                : <span className={a.dualstack.ipv4.attempted ? 'text-red-400' : 'text-gray-500'}>{a.dualstack.ipv4.attempted ? 'fail' : 'n/a'}</span>}
              {' · '}
              v6 {a.dualstack.ipv6.success && a.dualstack.ipv6.total_ms != null
                ? formatMs(a.dualstack.ipv6.total_ms)
                : <span className={a.dualstack.ipv6.attempted ? 'text-red-400' : 'text-gray-500'}>{a.dualstack.ipv6.attempted ? 'fail' : 'n/a'}</span>}
              {a.dualstack.faster_family != null && (
                <span className="text-cyan-400">
                  {' · '}{a.dualstack.faster_family} faster
                  {a.dualstack.delta_ms != null && ` by ${formatMs(a.dualstack.delta_ms)}`}
                </span>
              )}
            </p>
            <p className="text-gray-400 font-mono truncate">{a.dualstack.happy_eyeballs_verdict}</p>
          </SubResult>
        )}
        {a.websocket && (
          <SubResult label="WebSocket" color="cyan">
            <p className="text-gray-300">
              Upgrade {formatMs(a.websocket.upgrade_ms)} · RTT avg {formatMs(a.websocket.msg_rtt_avg_ms)} · Loss {a.websocket.loss_percent.toFixed(1)}%
            </p>
            <p className="text-gray-400">
              {a.websocket.echo_count}/{a.websocket.message_count} echoes
              {` · p95 ${formatMs(a.websocket.msg_rtt_p95_ms)}`}
            </p>
          </SubResult>
        )}
        {a.pmtud && (
          <SubResult label="PMTUD" color="gray">
            <p className="text-gray-300">
              {a.pmtud.path_mtu != null
                ? <>Path MTU <span className="font-mono">{a.pmtud.path_mtu}</span>{a.pmtud.lower_bound_only && <span className="text-yellow-400"> (lower bound)</span>}</>
                : <span className="text-yellow-400">no MTU verdict</span>}
              {a.pmtud.local_mtu != null && ` · local ${a.pmtud.local_mtu}`}
            </p>
            <p className="text-gray-400 font-mono truncate">{a.pmtud.method}</p>
          </SubResult>
        )}
        {a.page_load && (
          <SubResult label="Page Load" color="blue">
            <p className="text-gray-300">Total {formatMs(a.page_load.total_ms)} · {a.page_load.assets_fetched}/{a.page_load.asset_count} assets</p>
            {a.page_load.tls_setup_ms != null && <p className="text-gray-400">TLS setup: {formatMs(a.page_load.tls_setup_ms)}</p>}
          </SubResult>
        )}
        {a.browser && (
          <SubResult label="Browser" color="purple">
            <p className="text-gray-300">Load {formatMs(a.browser.load_ms)}</p>
            {a.browser.dom_content_loaded_ms != null && <p className="text-gray-400">DCL: {formatMs(a.browser.dom_content_loaded_ms)}</p>}
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
  const labelColor = color === 'red' ? 'text-red-400' : color === 'cyan' ? 'text-gray-300' : color === 'blue' ? 'text-blue-400' : color === 'purple' ? 'text-purple-400' : 'text-gray-400';
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
