import { lazy, Suspense, useState, useMemo } from 'react';
import { Link, useParams } from 'react-router';
import { errorMessage } from '../api/client';
import { stripAnsi } from '../lib/ansi';
import type { LiveAttempt } from '../api/types';
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
import { useToast } from '../hooks/useToast';
import {
  useCancelRunMutation,
  useComparisonGroupQuery,
  usePinBaselineMutation,
  useRunArtifactQuery,
  useRunAttemptsQuery,
  useRunInfraQuery,
  useTestConfigQuery,
  useTestRunQuery,
  useTestRunsQuery,
} from '../features/runs/queries';
import { groupFallbackName, sortCellsByName } from '../features/runs/list-grouping';
import { Button } from '../components/common/Button';
import { buttonClassName } from '../components/common/button-styles';
import { ErrorState } from '../components/common/AsyncState';
import { PageShell } from '../components/common/PageShell';
import {
  ArtifactSection,
  AttemptRow,
  StatsRow,
  TimingRow,
} from '../features/runs/components/RunDetailSections';
import { RunErrorBanner } from '../features/runs/components/RunErrorBanner';
import { dominantFailureReason, groupByProtocol, groupByTargetUrl } from '../features/runs/grouping';
import {
  buildUrlComparison,
  formatComparisonValue,
  shortUrlLabel,
  type UrlComparison,
} from '../features/runs/urlComparison';
import {
  buildSamplePoints,
  hasRepeatedSamples,
  jitterRatio,
  usedBurstSampling,
  MIN_SAMPLES_FOR_MEDIAN,
  type SamplePoint,
} from '../features/runs/burstStats';
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
  type Stats,
} from '../lib/analysis';

const RunDetailCharts = lazy(() => import('../features/runs/components/RunDetailCharts'));

export function RunDetailPage() {
  const { projectId, isProjectAdmin, isOperator } = useProject();
  const { runId } = useParams<{ runId: string }>();
  const addToast = useToast();
  const [expandedProtocols, setExpandedProtocols] = useState<Set<string>>(new Set());
  const [showShareDialog, setShowShareDialog] = useState(false);

  const shortId = runId?.slice(0, 8) ?? '';
  usePageTitle(runId ? `Run ${shortId}` : 'Run');
  const id = runId ?? '';
  const runQuery = useTestRunQuery(id);
  const run = runQuery.data ?? null;
  const isActive = !run || run.status === 'queued' || run.status === 'provisioning' || run.status === 'running';
  const attemptsQuery = useRunAttemptsQuery(id, isActive);
  const artifactQuery = useRunArtifactQuery(id, run?.artifact_id);
  const infraQuery = useRunInfraQuery(id);
  const cancelRun = useCancelRunMutation(id);

  // ── Pin-as-baseline (#810): completed benchmark runs (artifact-bearing —
  //    the detector's own gate) can be pinned as the config's regression
  //    baseline. The config detail carries the current pin.
  const isBenchmarkRun = !!run?.artifact_id && run.status === 'completed';
  const configQuery = useTestConfigQuery(run?.test_config_id ?? '', isBenchmarkRun);
  const isPinnedBaseline = isBenchmarkRun && configQuery.data?.baseline_run_id === id;
  const pinBaseline = usePinBaselineMutation(id, run?.test_config_id);

  // ── Comparison-group context (#803): breadcrumb + prev/next cell nav ──
  const groupId = run?.comparison_group_id ?? '';
  const groupQuery = useComparisonGroupQuery(groupId);
  // Polls (freshness audit) so prev/next tracks a still-running matrix:
  // 5s while any sibling cell is active, 15s once the group has settled.
  const siblingsQuery = useTestRunsQuery(
    projectId,
    { comparison_group_id: groupId },
    { enabled: !!groupId, intervalMs: 15_000, activeIntervalMs: 5_000 },
  );
  const groupNav = useMemo(() => {
    if (!groupId) return null;
    const siblings = sortCellsByName(siblingsQuery.data ?? []);
    const index = siblings.findIndex((r) => r.id === id);
    return {
      name: groupQuery.data?.name
        ?? groupFallbackName(siblings)
        ?? `Group ${groupId.slice(0, 8)}`,
      siblings,
      index,
      prev: index > 0 ? siblings[index - 1] : null,
      next: index >= 0 && index < siblings.length - 1 ? siblings[index + 1] : null,
    };
  }, [groupId, siblingsQuery.data, groupQuery.data, id]);

  const breadcrumbItems = groupNav
    ? [
        { label: 'Runs', to: `/projects/${projectId}/runs` },
        { label: groupNav.name, to: `/projects/${projectId}/benchmarks/compare/${groupId}` },
        { label: `Run ${shortId}` },
      ]
    : [
        { label: 'Runs', to: `/projects/${projectId}/runs` },
        { label: `Run ${shortId}` },
      ];

  const attempts = useMemo<LiveAttempt[]>(() => attemptsQuery.data ?? [], [attemptsQuery.data]);
  const artifact = artifactQuery.data ?? null;
  const infra = infraQuery.data ?? null;
  const loading = runQuery.isPending || attemptsQuery.isPending;
  const error = runQuery.error ? errorMessage(runQuery.error) : null;
  // Attempts failing must never dead-end the page when the run itself loaded.
  const attemptsError = attemptsQuery.error ? errorMessage(attemptsQuery.error) : null;

  const retry = () => {
    void runQuery.refetch();
    void attemptsQuery.refetch();
  };

  // ── Analysis (shared with HTML report logic) ──
  const protocolStats = useMemo(() => computeProtocolStats(attempts), [attempts]);
  const timingBreakdown = useMemo(() => computeTimingBreakdown(attempts), [attempts]);

  // Fair per-phase comparison across the URLs of a set run (#782): same run,
  // same runner, same tick — null for single-URL runs.
  const urlComparison = useMemo(() => buildUrlComparison(groupByTargetUrl(attempts)), [attempts]);

  // Median + spread per measurement point (#782 P2). Empty section unless the
  // run actually measured some point more than once — a run of single-shot
  // points has no median to report and must not pretend otherwise.
  const samplePoints = useMemo(() => buildSamplePoints(attempts), [attempts]);

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
      <PageShell before={<Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />}>
        <div className="text-gray-400 motion-safe:animate-pulse">Loading run {shortId}...</div>
      </PageShell>
    );
  }

  // Page-fatal only when the run itself failed to load. An attempts-only
  // failure renders the run with a degraded probe-details section below.
  if (error && !run) {
    return (
      <PageShell before={<Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: `Run ${shortId}` }]} />}>
        <ErrorState
          title={`Failed to load run ${shortId}`}
          message={error}
          onRetry={retry}
          secondaryAction={
            <Link
              to={`/projects/${projectId}/runs`}
              className={buttonClassName({ size: 'xs' })}
            >
              Back to runs
            </Link>
          }
        />
      </PageShell>
    );
  }

  // Attempt rows can be absent for a run that really happened (retention
  // pruning, ingest-only runs) — the run row still carries its counts. Fall
  // back so the header never claims "0 attempts" about a 20/20 run.
  const attemptsMissing = attempts.length === 0 && !!run && (run.success_count + run.failure_count) > 0;
  const successCount = attemptsMissing ? run.success_count : attempts.filter((a) => a.success).length;
  const failureCount = attemptsMissing ? run.failure_count : attempts.length - successCount;
  const probeCount = attemptsMissing ? run.success_count + run.failure_count : attempts.length;

  return (
    <PageShell before={<Breadcrumb items={breadcrumbItems} />}>

      {/* Header */}
      <div className="mb-6 flex items-start justify-between">
        <div>
          <div className="flex items-center gap-3 mb-1">
            <h2 className="text-xl font-bold text-gray-100">Run {shortId}</h2>
            {run && <StatusBadge status={runDisplayStatus(run)} />}
            {run?.artifact_id && (
              <span className="text-xs text-gray-300 bg-gray-500/10 px-1.5 py-0.5 rounded">benchmark</span>
            )}
            {isPinnedBaseline && (
              <span
                className="text-xs text-cyan-400 bg-cyan-500/10 px-1.5 py-0.5 rounded"
                title="This run is the config's pinned regression baseline"
              >
                baseline
              </span>
            )}
          </div>
          <p className="text-sm text-gray-400">
            {run?.config_name && <>Config: <span className="text-gray-300">{run.config_name}</span> · </>}
            {run?.modes && <>Modes: <span className="text-gray-300">{run.modes.join(', ')}</span> · </>}
            {probeCount} attempts{attemptsMissing ? ' (summary only)' : ''}
            {/* Comparison-group cell → cross-cell pivots (#794). */}
            {run?.comparison_group_id && (
              <>
                {' · '}
                <Link
                  to={`/projects/${projectId}/benchmarks/compare/${run.comparison_group_id}`}
                  className="text-cyan-400 hover:text-cyan-300"
                >
                  View group comparison &rarr;
                </Link>
              </>
            )}
          </p>
          {/* Prev/next through the group's sibling cells (#803), ordered by
              cell label like the compare page. */}
          {groupNav && groupNav.index >= 0 && groupNav.siblings.length > 1 && (
            <p className="text-sm text-gray-400 mt-1 flex items-center gap-2">
              <span>
                cell <span className="text-gray-300 tabular-nums">{groupNav.index + 1}</span> of{' '}
                <span className="text-gray-300 tabular-nums">{groupNav.siblings.length}</span>
              </span>
              {groupNav.prev ? (
                <Link
                  to={`/projects/${projectId}/runs/${groupNav.prev.id}`}
                  aria-label="Previous cell"
                  title={groupNav.prev.config_name ?? groupNav.prev.id.slice(0, 8)}
                  className="text-cyan-400 hover:text-cyan-300"
                >
                  &larr; prev
                </Link>
              ) : (
                <span className="text-faint" aria-hidden="true">&larr; prev</span>
              )}
              {groupNav.next ? (
                <Link
                  to={`/projects/${projectId}/runs/${groupNav.next.id}`}
                  aria-label="Next cell"
                  title={groupNav.next.config_name ?? groupNav.next.id.slice(0, 8)}
                  className="text-cyan-400 hover:text-cyan-300"
                >
                  next &rarr;
                </Link>
              ) : (
                <span className="text-faint" aria-hidden="true">next &rarr;</span>
              )}
            </p>
          )}
          {/* Run-envelope context (V046 pass-through) — data-gated: old runs
              have no envelope and render nothing here. */}
          <RunEnvelopeBlock envelope={run?.envelope} />
        </div>
        <div className="flex items-center gap-2">
          {run && (run.status === 'queued' || run.status === 'running') && (
            <Button
              onClick={() => {
                if (!runId) return;
                cancelRun.mutate(undefined, {
                  onSuccess: () => addToast('info', `Run ${shortId} cancel requested`),
                  onError: (e) => addToast('error', `Failed to cancel: ${errorMessage(e)}`),
                });
              }}
              variant="danger"
              size="xs"
              loading={cancelRun.isPending}
              loadingLabel="Cancelling…"
            >
              Cancel
            </Button>
          )}
          {/* Pin/unpin this run as the config's regression baseline (#810) —
              operator write, only for completed artifact-bearing runs (the
              detector can only compare against those). */}
          {isOperator && isBenchmarkRun && configQuery.data && (
            <Button
              onClick={() => {
                pinBaseline.mutate(!isPinnedBaseline, {
                  onSuccess: () => addToast('info', isPinnedBaseline
                    ? 'Baseline unpinned — detection falls back to each run’s previous run'
                    : `Run ${shortId} pinned as the config’s regression baseline`),
                  onError: (e) => addToast('error', `Failed to ${isPinnedBaseline ? 'unpin' : 'pin'} baseline: ${errorMessage(e)}`),
                });
              }}
              size="xs"
              loading={pinBaseline.isPending}
              title={isPinnedBaseline
                ? 'Stop using this run as the regression baseline'
                : 'Compare every future run of this config against this run'}
            >
              {isPinnedBaseline ? 'Unpin baseline' : 'Pin as baseline'}
            </Button>
          )}
          {/* Read-only document export — available to every role that can see
              the run (same visibility as the page itself). */}
          {runId && (
            <ExportMenu path={`/v2/test-runs/${runId}/report`} fileBase={`test-run-${shortId}`} />
          )}
          {isProjectAdmin && runId && (
            <Button
              onClick={() => setShowShareDialog(true)}
              size="xs"
            >
              Share
            </Button>
          )}
        </div>
      </div>

      {/* Why the run failed — rendered whenever the run carries an error,
          not only while queued/running (#791: failed runs hid error_message). */}
      {run?.error_message && (
        <RunErrorBanner status={run.status} message={run.error_message} />
      )}

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
          Probes <span className="text-gray-200 font-semibold ml-1">{probeCount}</span>
        </span>
        <span className="text-gray-400">
          Success <span className="text-green-400 font-semibold ml-1">{successCount}</span>
        </span>
        <span className="text-gray-400">
          Failed <span className={`font-semibold ml-1 ${failureCount > 0 ? 'text-red-400' : 'text-faint'}`}>{failureCount}</span>
        </span>
        <span className="text-gray-400">
          Rate <span className={`font-semibold ml-1 ${successRateClass(probeCount > 0 ? (successCount / probeCount) * 100 : 100)}`}>
            {probeCount > 0 ? `${((successCount / probeCount) * 100).toFixed(0)}%` : '-'}
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
            <p className="px-4 py-2 text-xs text-faint border-t border-gray-800/50 leading-relaxed">
              throughput is measured per direction — download over the body-receive window, upload over
              the send window (cross-checked against the server&apos;s Server-Timing clock). upload and
              download legitimately differ on asymmetric paths: cloud VMs cap egress, so a small target
              serves downloads slower than it accepts uploads. small payloads reflect TCP slow-start
              burst, not steady-state bandwidth.
            </p>
          )}
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
                      <div className="w-40 text-xs text-right shrink-0 truncate">
                        <span className="text-gray-300">{r.protocol}</span>
                        {r.payloadBytes != null && <span className="text-faint"> · {formatBytes(r.payloadBytes)}</span>}
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
                      <div className="w-24 text-xs text-gray-400 shrink-0">
                        {formatMs(s.min)}&ndash;{formatMs(s.max)}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
            {/* Log-scale decade tick labels */}
            <div className="relative h-4 mt-2 text-xs text-faint" style={{ marginLeft: 'calc(10rem + 0.75rem)', marginRight: 'calc(6rem + 0.75rem)' }}>
              {latencyAxis.decades.map((t) => (
                <span key={t} className="absolute -translate-x-1/2 whitespace-nowrap" style={{ left: `${latencyAxis.scale(t)}%` }}>{formatMs(t)}</span>
              ))}
            </div>
            <div className="flex items-center gap-4 mt-3 text-xs text-faint px-[calc(10rem+0.75rem)]">
              <span className="flex items-center gap-1"><span className="w-3 h-px bg-gray-500 inline-block" /> whisker (min/max)</span>
              <span className="flex items-center gap-1"><span className="w-3 h-3 rounded-sm border border-cyan-600/60 bg-cyan-900/30 inline-block" /> IQR (p25–p75)</span>
              <span className="flex items-center gap-1"><span className="w-0.5 h-3 bg-cyan-400 inline-block" /> median (p50)</span>
              <span className="ml-auto text-gray-600">log scale · throughput modes shown as transfer time</span>
            </div>
          </div>
        </div>
      )}

      {(protocolChartData.length > 1 || ttfbDistribution.length > 0) && (
        <Suspense fallback={<div className="mb-6 h-52 rounded border border-gray-800 motion-safe:animate-pulse" aria-label="Loading charts" />}>
          <RunDetailCharts protocolData={protocolChartData} ttfbData={ttfbDistribution} />
        </Suspense>
      )}

      {/* ── Attempts by Protocol (collapsible) ── */}
      <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">probe details</h3>
      {!loading && !attemptsError && attempts.length === 0 && (
        <div className="border border-gray-800 rounded p-6 text-center mb-2">
          <p className="text-gray-400 text-sm mb-1">
            {attemptsMissing ? 'Per-attempt detail is no longer stored for this run' : 'No attempts recorded'}
          </p>
          <p className="text-faint text-xs">
            {attemptsMissing
              ? 'The summary above reflects the run\u2019s recorded totals; individual probe rows were pruned or never ingested.'
              : 'The run finished without producing any probe attempts.'}
          </p>
        </div>
      )}
      {attemptsError && attempts.length === 0 && (
        <div className="border border-gray-800 rounded p-6 text-center mb-2">
          <p className="text-gray-400 text-sm mb-1">Attempt data unavailable</p>
          <p className="text-faint text-xs mb-3">{attemptsError}</p>
          <Button
            onClick={() => { void attemptsQuery.refetch(); }}
            variant="ghost"
            size="xs"
          >
            Retry
          </Button>
        </div>
      )}
      {/* ── Burst sampling (#782 P2): median + spread per measurement point ── */}
      {hasRepeatedSamples(samplePoints) && <BurstSamplingTable points={samplePoints} />}

      {/* ── URL set comparison (#782): side-by-side per-phase medians ── */}
      {urlComparison && <UrlComparisonTable comparison={urlComparison} />}

      {Object.entries(groupByTargetUrl(attempts))
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([targetUrl, urlAttempts], _idx, urlEntries) => {
        // Single-URL runs (and pre-#782 attempts with no target_url) keep the
        // flat protocol layout; a URL-set run gets one labelled section per URL.
        const multiUrl = urlEntries.filter(([u]) => u !== '').length > 1;
        return (
        <div key={targetUrl || 'all'}>
        {multiUrl && (
          <h3 className="mt-4 mb-1 px-1 text-xs font-medium tracking-wider text-cyan-400 font-mono">
            {targetUrl || 'unattributed'}
          </h3>
        )}
      {Object.entries(groupByProtocol(urlAttempts)).map(([protocol, group]) => {
        const sectionKey = multiUrl ? `${targetUrl}|${protocol}` : protocol;
        const isExpanded = expandedProtocols.has(sectionKey);
        const protoSuccess = group.filter((a) => a.success).length;
        const protoFail = group.length - protoSuccess;
        // Collapsed header stays diagnostic (#824): when one reason accounts
        // for most of the block's failures, say it — "5 FAIL — QUIC handshake
        // timeout" answers the question without expanding a row.
        const failReason = protoFail > 0 ? dominantFailureReason(group) : null;
        const values = group.filter((a) => a.success).map(primaryMetricValue).filter((v): v is number => v != null);
        const stats = computeStats(values);

        return (
          <div key={sectionKey} className="table-container mb-2">
            <button
              onClick={() => toggleProtocol(sectionKey)}
              className="w-full px-4 py-2.5 flex items-center justify-between text-left hover:bg-gray-800/10 transition-colors"
              aria-expanded={isExpanded}
            >
              <div className="flex items-center gap-3">
                <span className="text-gray-400 text-xs transition-transform" style={{ transform: isExpanded ? 'rotate(90deg)' : '' }} aria-hidden="true">{'\u25B6'}</span>
                <span className="text-gray-200 font-medium text-sm">{protocol.toUpperCase()}</span>
                <span className="text-gray-400 text-xs">{group.length} attempts</span>
                {stats && (
                  <span className="text-faint text-xs">
                    p50: {formatMetricValue(protocol, stats.p50)} · p95: {formatMetricValue(protocol, stats.p95)}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-3 text-xs min-w-0">
                <span className="text-green-400 shrink-0">{protoSuccess} OK</span>
                {protoFail > 0 && (
                  <span
                    className="text-red-400 min-w-0 truncate"
                    title={failReason ? `${protoFail} FAIL — ${failReason}` : undefined}
                  >
                    {protoFail} FAIL
                    {failReason && <span className="text-red-300/70"> — {failReason}</span>}
                  </span>
                )}
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
              <p className="text-red-400 text-xs mt-2">{stripAnsi(run.error_message)}</p>
            )}
          </div>
        </div>
      )}
    </PageShell>
  );
}

// ─── Artifact Section (merged from BenchmarkDetailPage) ─────────────────────

// ─── URL set comparison (#782) ───────────────────────────────────────────────
// Side-by-side per-phase medians for a multi-URL set run. The green cell is
// the row's unique winner; ties and single-value rows crown nobody. Medians
// are over successful attempts of the modes shared by every URL — modes that
// only succeed on some URLs are excluded so protocol support can't pose as
// latency (#820 review), and the footnote says so.
/**
 * Median + spread per measurement point (#782 P2). The median is the headline
 * — one cold DNS cache or one retransmit must not decide what a point
 * "measured" — with p95/min/max beside it and the sample count in front of it,
 * so the reader can see what the median rests on. A point with fewer than
 * MIN_SAMPLES_FOR_MEDIAN usable samples is labelled as a reading, not dressed
 * up as a median; failed samples are counted and shown, never dropped.
 */
function BurstSamplingTable({ points }: { points: SamplePoint[] }) {
  const multiUrl = new Set(points.map((p) => p.targetUrl)).size > 1;
  const burst = usedBurstSampling(points);
  // Only points that actually render a number: a metric-less point is
  // under-sampled by the letter of the flag, but the footer's "those
  // numbers are readings" caption would be describing an empty cell.
  const underSampled = points.filter((p) => p.underSampled && p.stats).length;

  return (
    <div className="table-container mb-4">
      <h3 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
        {burst ? 'burst sampling' : 'repeat sampling'} {'\u2014'} median &amp; spread per point
      </h3>
      <div className="overflow-x-auto">
        <table className="w-full text-xs tabular-nums">
          <thead>
            <tr className="text-faint uppercase tracking-wider">
              {multiUrl && (
                <th className="text-left py-2 px-4 font-medium border-b border-gray-800/50">URL</th>
              )}
              <th className="text-left py-2 px-4 font-medium border-b border-gray-800/50">Mode</th>
              <th className="text-left py-2 px-4 font-medium border-b border-gray-800/50">Metric</th>
              <th className="text-right py-2 px-4 font-medium border-b border-gray-800/50">Samples</th>
              <th className="text-right py-2 px-4 font-medium border-b border-gray-800/50">Median</th>
              <th className="text-right py-2 px-4 font-medium border-b border-gray-800/50">p95</th>
              <th className="text-right py-2 px-4 font-medium border-b border-gray-800/50">Min</th>
              <th className="text-right py-2 px-4 font-medium border-b border-gray-800/50">Max</th>
              <th className="text-right py-2 px-4 font-medium border-b border-gray-800/50">p95/p50</th>
            </tr>
          </thead>
          <tbody>
            {points.map((point) => {
              const ratio = jitterRatio(point);
              return (
                <tr key={point.key} className="border-b border-white/[0.02] last:border-b-0">
                  {multiUrl && (
                    <td className="py-1.5 px-4 text-cyan-400 font-mono">
                      {point.targetUrl ? shortUrlLabel(point.targetUrl) : 'unattributed'}
                    </td>
                  )}
                  <td className="py-1.5 px-4 text-gray-200">
                    {point.protocol.toUpperCase()}
                    {point.payloadBytes != null && (
                      <span className="text-faint"> {'\u00B7'} {formatBytes(point.payloadBytes)}</span>
                    )}
                  </td>
                  <td className="py-1.5 px-4 text-gray-400">{point.metricLabel}</td>
                  <td className="py-1.5 px-4 text-right">
                    <span className={point.underSampled ? 'text-yellow-400' : 'text-gray-200'}>
                      {point.usableCount}
                    </span>
                    <span className="text-faint">/{point.sampleCount}</span>
                    {point.failedCount > 0 && (
                      <span className="text-red-400"> {'\u00B7'} {point.failedCount} failed</span>
                    )}
                  </td>
                  {point.stats ? (
                    <>
                      <td className="py-1.5 px-4 text-right text-gray-100 font-medium">
                        {formatMetricValue(point.protocol, point.stats.p50)}
                        {point.underSampled && (
                          <span
                            className="text-yellow-400 font-normal"
                            title={`Only ${point.usableCount} usable sample${point.usableCount === 1 ? '' : 's'} \u2014 a median needs at least ${MIN_SAMPLES_FOR_MEDIAN}. This is the reading itself, not a median.`}
                          >
                            {' '}
                            {point.usableCount === 1 ? '(1 sample)' : `(${point.usableCount} samples)`}
                          </span>
                        )}
                      </td>
                      <td className="py-1.5 px-4 text-right text-gray-300">
                        {point.underSampled ? '-' : formatMetricValue(point.protocol, point.stats.p95)}
                      </td>
                      <td className="py-1.5 px-4 text-right text-gray-400">
                        {formatMetricValue(point.protocol, point.stats.min)}
                      </td>
                      <td className="py-1.5 px-4 text-right text-gray-400">
                        {formatMetricValue(point.protocol, point.stats.max)}
                      </td>
                      <td className="py-1.5 px-4 text-right text-gray-300">
                        {ratio == null ? '-' : `${ratio.toFixed(2)}\u00D7`}
                      </td>
                    </>
                  ) : point.failedCount === point.sampleCount ? (
                    <td className="py-1.5 px-4 text-right text-red-400" colSpan={5}>
                      no usable sample {'\u2014'} every sample failed
                    </td>
                  ) : point.failedCount > 0 ? (
                    <td className="py-1.5 px-4 text-right text-red-400" colSpan={5}>
                      no usable sample {'\u2014'} {point.failedCount} failed, the rest reported no{' '}
                      {point.metricLabel.toLowerCase()}
                    </td>
                  ) : (
                    <td className="py-1.5 px-4 text-right text-faint" colSpan={5}>
                      no {point.metricLabel.toLowerCase()} recorded {'\u2014'} all {point.sampleCount}{' '}
                      sample{point.sampleCount === 1 ? '' : 's'} succeeded
                    </td>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <p className="px-4 py-2 text-xs text-faint border-t border-gray-800/50 leading-relaxed">
        median over the successful samples of each point; p95/min/max are its spread and p95/p50 its
        jitter ratio (1.00{'\u00D7'} = perfectly consistent). failed samples are counted, never dropped
        {underSampled > 0 && (
          <>
            {'. '}
            <span className="text-yellow-400">
              {underSampled} point{underSampled === 1 ? '' : 's'} had fewer than {MIN_SAMPLES_FOR_MEDIAN}{' '}
              usable samples
            </span>
            {' \u2014 those numbers are readings, not medians'}
          </>
        )}
        .
      </p>
    </div>
  );
}

function UrlComparisonTable({ comparison }: { comparison: UrlComparison }) {
  return (
    <div className="table-container mb-4">
      <h3 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
        url comparison — same run, same tick · medians over successful attempts
      </h3>
      <div className="overflow-x-auto">
        <table className="w-full text-xs tabular-nums">
          <thead>
            <tr className="text-faint uppercase tracking-wider">
              <th className="text-left py-2 px-4 font-medium border-b border-gray-800/50">Phase</th>
              {comparison.columns.map(col => (
                <th key={col.url} className="text-right py-2 px-4 font-medium border-b border-gray-800/50">
                  <span className="text-cyan-400 font-mono normal-case">{col.label}</span>
                  <span className="block text-faint font-normal">{col.attempts} attempts</span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {comparison.rows.map(row => (
              <tr key={row.key} className="border-b border-white/[0.02] last:border-b-0">
                <td className="py-1.5 px-4 text-gray-400">{row.label}</td>
                {row.values.map((value, i) => (
                  <td
                    key={comparison.columns[i].url}
                    className={`py-1.5 px-4 text-right ${
                      i === row.bestIndex ? 'text-green-400 font-medium' : 'text-gray-200'
                    }`}
                  >
                    {formatComparisonValue(value, row.unit)}
                    {i === row.bestIndex && <span aria-hidden="true"> {'\u2713'}</span>}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {comparison.excludedModes.length > 0 && (
        <p className="px-4 py-2 text-xs text-faint border-t border-gray-800/50">
          {comparison.comparedModes.length > 0
            ? `timings compared over shared modes: ${comparison.comparedModes.join(', ')} — excluded: ${comparison.excludedModes.join(', ')} (not successful on every URL)`
            : 'URLs share no successful mode — timings shown for reference, no winners marked'}
        </p>
      )}
    </div>
  );
}
