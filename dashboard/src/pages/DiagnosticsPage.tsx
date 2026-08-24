import { useState, useEffect, useRef, useMemo } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useNow } from '../hooks/useNow';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { runsApi } from '../features/runs/api';
import { testersApi, type TesterRow } from '../api/testers';
import type { TestConfig, TestConfigListItem, TestRun, TestSchedule } from '../api/types';
import {
  buildDiagRequest,
  decodeHostQueryParam,
  extractHost,
  hostsToQueryParam,
  toProbeUrl,
  DIAG_SAMPLE_CHOICES,
  type DiagPreset,
  type DiagSamples,
} from '../lib/diag-request';
import { StatusBadge } from '../components/common/StatusBadge';
import { RunResult } from '../components/common/RunResult';
import { runDisplayStatus } from '../lib/runStatus';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { timeAgo } from '../lib/format';
import { stripAnsi } from '../lib/ansi';
import {
  hostsForDiagConfig,
  hostsFromDiagConfigName,
  isDiagSetConfigName,
  isWatchlistConfig,
  isWatchlistConfigName,
  probeRunVerdict,
} from '../lib/watchlist';
import {
  MAX_SET_URLS,
  formatUrlSetInput,
  hostsForSelection,
  launchInputFor,
  parseUrlSetInput,
  setSelectionKey,
  splitUrlSetInput,
} from '../lib/probe-set';
import {
  NO_RUNNER,
  UNKNOWN_RUNNER,
  capacityOptions,
  matchesRunnerFilter,
  parseProbeGroupBy,
  probeBucketFor,
  providerOptions,
  runnerIdentityForRun,
  summarizeBucketRunners,
  type BucketRunnerSummary,
  type ProbeBucket,
  type ProbeGroupBy,
  type RunnerIdentity,
} from '../lib/probe-grouping';
import { cloudProviderBadge } from '../lib/provider';
import { Button } from '../components/common/Button';
import { isOnlineTester } from '../lib/tester-readiness';
import {
  runKeys,
  useRunsAttemptsQueries,
  useSchedulesQuery,
  useTestConfigDetailsQueries,
  useTestConfigsQuery,
  useTestRunsQuery,
  useUpdateScheduleMutation,
} from '../features/runs/queries';

// ── Types ───────────────────────────────────────────────────────────────

type FilterMode = 'all' | 'healthy' | 'partial' | 'failed' | 'pending' | 'stale';
type SortMode = 'last-checked' | 'name' | 'slowest' | 'most-runs';

interface UrlGroup {
  /** Row identity: the host under host grouping; host + runner axes otherwise
   *  (lib/probe-grouping.ts). Keys expanded/pending state and the card DOM id. */
  key: string;
  host: string;
  bucket: ProbeBucket;
  /** Region(s) / catalog specs of the runners behind this row's runs. */
  runner: BucketRunnerSummary;
  runs: TestRun[];
  configIds: Set<string>;
  lastRun: TestRun;
  lastStatus: 'healthy' | 'partial' | 'failed' | 'stale' | 'pending';
  totalDurationMs: number | null;
}

// ── Constants ───────────────────────────────────────────────────────────

const DIAG_PRESET_LABELS: Record<DiagPreset, { time: string; desc: string }> = {
  quick: { time: '~3s', desc: 'dns, tcp, tls, http2' },
  standard: { time: '~12s', desc: '+ http1, http3, tls-resume, native-tls' },
  full: { time: '~45s', desc: '+ curl, browser page loads' },
  route: { time: '~45s', desc: 'ping, traceroute, v4-vs-v6, path MTU' },
};

const PAGE_SIZE = 20;
const STALE_THRESHOLD_MS = 24 * 60 * 60 * 1000; // 24 hours
const DIAGNOSTIC_RUN_PARAMS = { endpoint_kind: 'network', limit: 200 } as const;
// The one cron this page creates (#782). Only schedules matching it render the
// "hourly" badge/button state — an API-created daily schedule on the same
// config must not read as "Monitoring hourly".
const HOURLY_CRON = '0 * * * *';

// The toolbar's compact select (sort + the runner-axis group/provider/size
// selects share it) — same chevron as the probe bar's selects, tighter padding.
const TOOLBAR_SELECT_CLASS =
  'bg-transparent border border-gray-800 rounded px-2.5 py-1 text-xs text-gray-400 focus:outline-none appearance-none pr-6 cursor-pointer';
const TOOLBAR_SELECT_STYLE = {
  backgroundImage: `url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%23475569' stroke-width='1.5'/%3E%3C/svg%3E")`,
  backgroundRepeat: 'no-repeat',
  backgroundPosition: 'right 8px center',
} as const;

const PHASE_CSS_COLORS: Record<string, string> = {
  dns: '#a78bfa',
  tcp: '#22d3ee',
  tls: '#f59e0b',
  ttfb: '#10b981',
  download: '#3b82f6',
};

// ── Helpers ─────────────────────────────────────────────────────────────

// Host resolution lives in lib/watchlist.ts (hostsForDiagConfig /
// hostsFromDiagConfigName) — shared with tests. A set config (#782/#820)
// resolves to EVERY member hostname so its runs land on each member's row.

/**
 * Entry-box state from the `?host=` param. decodeHostQueryParam (#820)
 * tolerates the double-encoded values the old sync produced (`%2C%2520`); a
 * param carrying SEVERAL hosts is a shared set link, so it opens in the
 * multi-URL box already laid out one per line (#782 P1).
 */
function initialEntryState(hostParam: string | null): { text: string; multi: boolean } {
  const decoded = decodeHostQueryParam(hostParam);
  const entries = splitUrlSetInput(decoded);
  return entries.length > 1
    ? { text: formatUrlSetInput(entries), multi: true }
    : { text: decoded, multi: false };
}

function getDayLabel(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const runDay = new Date(date.getFullYear(), date.getMonth(), date.getDate());

  if (runDay.getTime() === today.getTime()) return 'Today';
  if (runDay.getTime() === yesterday.getTime()) return 'Yesterday';
  return date.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });
}

function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return '-';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function fmtTime(dateStr: string): string {
  return new Date(dateStr).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

function getDurationMs(run: TestRun): number | null {
  if (run.started_at && run.finished_at) {
    return new Date(run.finished_at).getTime() - new Date(run.started_at).getTime();
  }
  return null;
}

// ── Sparkline Component ─────────────────────────────────────────────────

function Sparkline({ values }: { values: number[] }) {
  if (values.length < 2) return null;
  const w = 80;
  const h = 16;
  const padding = 2;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = max - min || 1;
  const points = values
    .slice(-10)
    .map((v, i, arr) => {
      const x = padding + (i / (arr.length - 1)) * (w - padding * 2);
      const y = h - padding - ((v - min) / range) * (h - padding * 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');

  return (
    <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} className="flex-shrink-0" aria-hidden="true">
      <polyline
        points={points}
        fill="none"
        stroke="#22d3ee"
        strokeWidth="1.5"
        opacity="0.5"
      />
    </svg>
  );
}

// ── Phase Bar Component ─────────────────────────────────────────────────

interface PhaseTimings {
  dns: number | null;
  tcp: number | null;
  tls: number | null;
  ttfb: number | null;
  download: number | null;
}

function parsePhaseTimings(run: TestRun): PhaseTimings {
  // Phase timings come from the run's duration breakdown
  // Since we don't have per-phase timing in TestRun, we estimate from
  // success_count and total duration proportionally
  const duration = getDurationMs(run);
  if (!duration || duration <= 0) {
    return { dns: null, tcp: null, tls: null, ttfb: null, download: null };
  }
  // Without per-phase data from the API, show total duration only
  // The real timing data would come from attempts/artifacts
  return { dns: null, tcp: null, tls: null, ttfb: null, download: null };
}

function PhaseBar({ timings }: { timings: PhaseTimings }) {
  const phases = [
    { key: 'dns', label: 'dns', value: timings.dns, color: PHASE_CSS_COLORS.dns },
    { key: 'tcp', label: 'tcp', value: timings.tcp, color: PHASE_CSS_COLORS.tcp },
    { key: 'tls', label: 'tls', value: timings.tls, color: PHASE_CSS_COLORS.tls },
    { key: 'ttfb', label: 'ttfb', value: timings.ttfb, color: PHASE_CSS_COLORS.ttfb },
    { key: 'download', label: 'download', value: timings.download, color: PHASE_CSS_COLORS.download },
  ].filter(p => p.value != null && p.value > 0);

  if (phases.length === 0) return null;

  const total = phases.reduce((sum, p) => sum + (p.value ?? 0), 0);

  return (
    <div className="mb-4">
      <div className="flex h-1.5 rounded overflow-hidden mb-2">
        {phases.map(p => (
          <div
            key={p.key}
            style={{
              width: `${((p.value ?? 0) / total) * 100}%`,
              background: p.color,
              minWidth: 2,
            }}
          />
        ))}
      </div>
      <div className="flex gap-4 text-xs">
        {phases.map(p => (
          <span key={p.key} className="flex items-center gap-1.5 text-gray-400">
            <span
              className="w-1.5 h-1.5 rounded-sm flex-shrink-0"
              style={{ background: p.color }}
            />
            {p.label}
            <span className="text-gray-400 tabular-nums">{fmtMs(p.value)}</span>
          </span>
        ))}
      </div>
    </div>
  );
}

// ── URL Card Component ──────────────────────────────────────────────────

function UrlCard({
  group,
  expanded,
  onToggle,
  onRunAgain,
  onRemove,
  projectId,
  schedule,
  onMonitorHourly,
  monitorPending,
  onToggleMonitor,
  selected,
  onSelectedChange,
}: {
  projectId: string;
  group: UrlGroup;
  expanded: boolean;
  onToggle: () => void;
  onRunAgain: (host: string) => void;
  onRemove: (host: string, configIds: Set<string>) => void;
  /** Ticked into the current URL set (#782 P1 multi-select). */
  selected: boolean;
  onSelectedChange: (selected: boolean) => void;
  /** Hourly-monitoring schedule attached to one of this row's configs (#782). */
  schedule: TestSchedule | null;
  onMonitorHourly: () => void;
  /** True while a "Monitor hourly" request for this host is in flight —
   *  disables the button so a double click cannot create duplicate rows. */
  monitorPending: boolean;
  onToggleMonitor: (schedule: TestSchedule) => void;
}) {
  const navigate = useNavigate();
  const { host, runs, lastRun, lastStatus, configIds, bucket, runner } = group;
  const isActive = lastRun.status === 'queued' || lastRun.status === 'running';
  // Runner axis facts for grouped views (SideLine style, InfraEnvelope.tsx):
  // provider badge · size · region · vCPU / GB. Region is not part of a
  // capacity key, so a row spanning several regions says so instead of
  // picking one.
  const regionLabel =
    runner.regions.length === 1
      ? runner.regions[0]
      : runner.regions.length > 1
        ? `${runner.regions.length} regions`
        : null;

  // Determine card border class
  const borderClass =
    lastStatus === 'failed'
      ? 'border-l-2 border-l-red-500'
      : lastStatus === 'partial' || lastStatus === 'stale'
        ? 'border-l-2 border-l-yellow-500'
        : lastStatus === 'pending'
          ? 'border-l-2 border-l-cyan-500'
          : '';

  // URL text color
  const urlColor =
    lastStatus === 'failed'
      ? 'text-red-400'
      : lastStatus === 'partial' || lastStatus === 'stale'
        ? 'text-yellow-400'
        : lastStatus === 'pending'
          ? 'text-gray-300'
          : 'text-cyan-400';

  // Status dot color
  const dotColor =
    lastStatus === 'failed'
      ? 'bg-red-500'
      : lastStatus === 'partial' || lastStatus === 'stale'
        ? 'bg-yellow-500'
        : lastStatus === 'pending'
          ? 'bg-cyan-500 animate-pulse'
          : 'bg-green-500';

  // Sparkline data: last 10 runs' durations
  const sparklineValues = useMemo(() => {
    return runs
      .slice(0, 10)
      .map(r => getDurationMs(r))
      .filter((v): v is number => v != null)
      .reverse();
  }, [runs]);

  // Phase timings from last run (placeholder until phase data available)
  const lastTimings = useMemo(() => parsePhaseTimings(lastRun), [lastRun]);

  // Group runs by day for expanded view
  const groupedRuns = useMemo(() => {
    const groups: Array<{ label: string; runs: TestRun[] }> = [];
    let currentLabel = '';
    for (const run of runs) {
      const label = getDayLabel(run.created_at);
      if (label !== currentLabel) {
        groups.push({ label, runs: [run] });
        currentLabel = label;
      } else {
        groups[groups.length - 1].runs.push(run);
      }
    }
    return groups;
  }, [runs]);

  const totalDuration = getDurationMs(lastRun);

  return (
    <div
      className={`border rounded mb-1.5 transition-colors ${borderClass} ${
        selected ? 'border-cyan-500/40' : 'border-gray-800'
      } ${expanded ? 'bg-[var(--bg-surface)]' : ''}`}
    >
      {/* Collapsed header. The set checkbox is a SIBLING of the expand button,
          not a child: an interactive control inside a <button> is invalid HTML
          and swallows its own clicks. */}
      <div className="flex items-center">
        <label
          className="flex items-center pl-4 pr-0.5 py-3 cursor-pointer flex-shrink-0"
          title={`Add ${host} to a URL set`}
        >
          <input
            type="checkbox"
            checked={selected}
            onChange={e => onSelectedChange(e.target.checked)}
            // 24 CSS px is the WCAG 2.2 AA minimum target size, and axe measures
            // the INPUT's own box — the label's padding around it does not count.
            // The repo's coarse-pointer hit-area rule only covers <button>, since
            // ::after does not render on a replaced element like a checkbox.
            className="w-6 h-6 accent-cyan-500 cursor-pointer"
            aria-label={`Select ${host} for a URL set`}
          />
        </label>
        <button
          onClick={onToggle}
          className="flex items-center flex-1 min-w-0 pl-2.5 pr-4 py-3 gap-3 text-left hover:bg-white/[0.015] transition-colors cursor-pointer"
          aria-expanded={expanded}
          aria-controls={`card-body-${group.key}`}
        >
          <span
            className={`text-faint text-xs flex-shrink-0 transition-transform duration-200 ${
              expanded ? 'rotate-90' : ''
            }`}
            aria-hidden="true"
          >
            {'\u25B8'}
          </span>

          <div className="flex items-center gap-3 flex-1 min-w-0">
            <span className={`text-sm font-medium truncate ${urlColor}`}>
              {host}
            </span>

            {/* Runner axis (provider / capacity grouping only) */}
            {bucket.groupBy !== 'host' && (
              <span className="flex items-center gap-2 text-xs whitespace-nowrap">
                {bucket.unknownRunner ? (
                  <span
                    className="px-1.5 py-0.5 rounded bg-gray-500/20 text-gray-400"
                    title="No runner identity for these runs: no tester bound, or the tester row was deleted"
                  >
                    unknown runner
                  </span>
                ) : (
                  <>
                    {bucket.cloud !== null ? (
                      <span className={`px-1.5 py-0.5 rounded ${cloudProviderBadge(bucket.cloud)}`}>
                        {bucket.cloud}
                      </span>
                    ) : (
                      <span className="px-1.5 py-0.5 rounded bg-gray-500/20 text-gray-400">unknown provider</span>
                    )}
                    {bucket.groupBy === 'capacity' && (
                      <span className="text-gray-300">{bucket.vmSize ?? 'unknown size'}</span>
                    )}
                    {regionLabel && <span className="text-faint">{regionLabel}</span>}
                    {bucket.groupBy === 'capacity' && (
                      runner.vcpus !== null && runner.memoryGb !== null ? (
                        <span className="text-faint">{runner.vcpus} vCPU / {runner.memoryGb} GB</span>
                      ) : bucket.vmSize !== null ? (
                        <span className="text-gray-600">no spec in catalog</span>
                      ) : null
                    )}
                  </>
                )}
              </span>
            )}

            {/* Inline phase timings - show total duration if no phase breakdown */}
            {totalDuration != null && (
              <div className="hidden xl:flex items-center gap-3 text-xs tabular-nums whitespace-nowrap">
                <span className="text-gray-200 font-medium" title="Wall-clock duration of the last full run">
                  <span className="text-gray-400 mr-1 font-normal">run</span>
                  {fmtMs(totalDuration)}
                </span>
              </div>
            )}
          </div>

          <div className="flex items-center gap-3 flex-shrink-0">
            {schedule?.enabled && (
              <span
                className="text-xs px-1.5 py-0.5 rounded border border-cyan-500/30 text-cyan-400/90 tracking-wider"
                title="Re-probed automatically every hour"
              >
                hourly
              </span>
            )}
            {sparklineValues.length >= 2 && <Sparkline values={sparklineValues} />}
            <span className="text-xs px-1.5 py-0.5 rounded bg-white/5 text-gray-400 font-medium tabular-nums">
              {runs.length}
            </span>
            <span className="text-xs text-faint whitespace-nowrap">
              {timeAgo(lastRun.created_at)}
            </span>
            <span className={`w-[7px] h-[7px] rounded-full flex-shrink-0 ${dotColor}`} />
            {isActive && (
              <span className="w-1.5 h-1.5 rounded-full bg-cyan-400 motion-safe:animate-pulse flex-shrink-0" />
            )}
          </div>
        </button>
      </div>

      {/* Expanded body */}
      {expanded && (
        <div id={`card-body-${group.key}`} className="px-4 pb-4">
          {/* Phase breakdown bar */}
          <PhaseBar timings={lastTimings} />

          {/* History by day */}
          <div className="mt-3">
            {groupedRuns.map(group => (
              <div key={group.label}>
                <div className="text-xs text-faint uppercase tracking-wider mb-1 mt-3 first:mt-0">
                  {group.label}
                </div>
                <table className="w-full text-xs tabular-nums">
                  <thead>
                    <tr className="text-xs text-faint uppercase tracking-wider">
                      <th className="text-left py-1 px-2 font-medium border-b border-gray-800/50">Time</th>
                      <th className="text-left py-1 px-2 font-medium border-b border-gray-800/50" />
                      <th className="text-left py-1 px-2 font-medium border-b border-gray-800/50">Status</th>
                      <th className="text-right py-1 px-2 font-medium border-b border-gray-800/50">Results</th>
                      <th className="text-right py-1 px-2 font-medium border-b border-gray-800/50">Duration</th>
                      <th className="border-b border-gray-800/50 w-6"></th>
                    </tr>
                  </thead>
                  <tbody>
                    {group.runs.map(run => {
                      const dur = getDurationMs(run);
                      const verdict = runDisplayStatus(run);
                      return (
                        // Every run row opens the run detail — mid-run it
                        // live-streams attempts; finished it shows the full
                        // breakdown. These rows were dead text before
                        // (user-caught 2026-08-12: "cannot select some link
                        // to see the result").
                        <tr
                          key={run.id}
                          onClick={() => navigate(`/projects/${projectId}/runs/${run.id}`)}
                          className={`border-b border-white/[0.02] last:border-b-0 cursor-pointer hover:bg-gray-800/20 transition-colors ${
                            verdict === 'failed' ? 'text-red-400/80' : ''
                          }`}
                        >
                          <td className="py-1.5 px-2 whitespace-nowrap">
                            <Link
                              to={`/projects/${projectId}/runs/${run.id}`}
                              onClick={(e) => e.stopPropagation()}
                              className="text-gray-400 hover:text-cyan-400"
                            >
                              {fmtTime(run.created_at)}
                            </Link>
                          </td>
                          <td className="py-1.5 px-2">
                            {verdict === 'completed' ? (
                              <span className="text-green-400">{'\u2713'}</span>
                            ) : verdict === 'partial' ? (
                              <span className="text-yellow-400">{'\u2713'}</span>
                            ) : verdict === 'failed' ? (
                              <span className="text-red-400">{'\u2717'}</span>
                            ) : verdict === 'running' || verdict === 'queued' ? (
                              <span className="text-cyan-400 motion-safe:animate-pulse">{'\u25CF'}</span>
                            ) : (
                              <span className="text-faint">-</span>
                            )}
                          </td>
                          <td className="py-1.5 px-2">
                            <StatusBadge status={verdict} />
                          </td>
                          <td className="py-1.5 px-2 text-right">
                            {run.status === 'completed' && (
                              <RunResult ok={run.success_count} fail={run.failure_count} />
                            )}
                          </td>
                          <td className="py-1.5 px-2 text-right text-gray-200 font-medium">
                            {fmtMs(dur)}
                          </td>
                          <td className="py-1.5 px-2 text-right text-gray-600 w-6">→</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
                {/* Show error messages for failed runs */}
                {group.runs
                  .filter(r => r.error_message)
                  .map(r => (
                    <div key={`err-${r.id}`} className="text-xs text-red-400/70 pl-2 mt-1">
                      {stripAnsi(r.error_message!)}
                    </div>
                  ))}
              </div>
            ))}
          </div>

          {/* Actions */}
          <div className="flex items-center gap-4 mt-4 pt-3 border-t border-gray-800">
            <button
              onClick={() => onRunAgain(host)}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 text-xs border border-gray-800 rounded text-gray-400 hover:text-gray-200 hover:border-gray-600 transition-colors"
            >
              {'\u25B6'} Run again
            </button>
            {schedule ? (
              <button
                onClick={() => onToggleMonitor(schedule)}
                className={`inline-flex items-center gap-1.5 px-3 py-1.5 text-xs border rounded transition-colors ${
                  schedule.enabled
                    ? 'border-cyan-500/30 text-cyan-400 hover:border-cyan-500/60'
                    : 'border-gray-800 text-gray-400 hover:text-gray-200 hover:border-gray-600'
                }`}
                title={schedule.enabled
                  ? 'Hourly monitoring is on \u2014 click to pause'
                  : 'Hourly monitoring is paused \u2014 click to resume'}
              >
                {schedule.enabled ? 'Monitoring hourly \u2713' : 'Resume hourly monitoring'}
              </button>
            ) : (
              <button
                onClick={onMonitorHourly}
                disabled={monitorPending}
                className="inline-flex items-center gap-1.5 px-3 py-1.5 text-xs border border-gray-800 rounded text-gray-400 hover:text-gray-200 hover:border-gray-600 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                title="Re-probe this URL automatically every hour (the last-used preset/set)"
              >
                {monitorPending ? 'Scheduling…' : 'Monitor hourly'}
              </button>
            )}
            <button
              onClick={() => onRemove(host, configIds)}
              className="ml-auto text-xs text-faint hover:text-red-400 transition-colors"
            >
              Remove from watchlist
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Main Page Component ─────────────────────────────────────────────────

export function DiagnosticsPage() {
  const { projectId } = useProject();
  const queryClient = useQueryClient();
  const addToast = useToast();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  usePageTitle('URL Probe');

  const inputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  // Entry box + which control renders it, both decided once from ?host=.
  // Multi-URL entry (#782 P1) is a textarea, one URL per line, paste-friendly;
  // it REPLACES the single-line field rather than sitting next to it, so there
  // is never a question about which box the probe reads.
  const [initialEntry] = useState(() => initialEntryState(searchParams.get('host')));
  const [url, setUrl] = useState(initialEntry.text);
  const [multiMode, setMultiMode] = useState(initialEntry.multi);
  // Prefill the preset from ?preset= (scenario launcher); fall back to 'quick'.
  const [preset, setPreset] = useState<DiagPreset>(() => {
    const p = searchParams.get('preset');
    return p === 'standard' || p === 'full' || p === 'route' ? p : 'quick';
  });
  // Burst sampling (#782 P2): N samples per mode per URL within one run.
  const [samples, setSamples] = useState<DiagSamples>(() => {
    const s = Number(searchParams.get('samples'));
    return (DIAG_SAMPLE_CHOICES as readonly number[]).includes(s) ? (s as DiagSamples) : 1;
  });
  const [submitting, setSubmitting] = useState(false);

  // Data
  const [testers, setTesters] = useState<TesterRow[]>([]);
  // null = "auto-pick", otherwise a specific tester_id. Persisted in URL so
  // a user can share a probe-URL that pins the runner.
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // UI state
  const [filter, setFilter] = useState<FilterMode>('all');
  const [sort, setSort] = useState<SortMode>('last-checked');
  // Runner axes (lib/probe-grouping.ts): split a host's history by the
  // provider / VM size of the runner that probed it, and narrow to one
  // provider or size. All three ride the query string (?group=, ?provider=,
  // ?size=) next to ?host= so a comparison view is shareable.
  const [groupBy, setGroupBy] = useState<ProbeGroupBy>(() => parseProbeGroupBy(searchParams.get('group')));
  const [providerFilter, setProviderFilter] = useState<string>(() => searchParams.get('provider') ?? '');
  const [sizeFilter, setSizeFilter] = useState<string>(() => searchParams.get('size') ?? '');
  const [page, setPage] = useState(1);
  const [expandedCards, setExpandedCards] = useState<Set<string>>(new Set());
  // Watchlist multi-select (#782 P1) — row keys, not hosts: under the
  // provider / capacity grouping one host owns several rows. Resolution back
  // to hosts (with de-duplication) is hostsForSelection, and it only ever sees
  // the rows currently on screen, so a stale key can never smuggle a URL into
  // a set the user cannot see.
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());

  // Sync URL to query string. hostsToQueryParam (#820): hostnames only,
  // comma-joined without spaces — the raw multi-entry input ("a.com, b.com")
  // used to be stored verbatim and its space percent-encoded (then DOUBLE-
  // encoded to %2520 in links built from already-encoded values).
  useEffect(() => {
    const host = hostsToQueryParam(url);
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (host) next.set('host', host);
      else next.delete('host');
      return next;
    }, { replace: true });
  }, [url, setSearchParams]);

  // Grouping / runner filters → query string. Defaults are omitted so the
  // plain /probe URL stays clean.
  useEffect(() => {
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (groupBy !== 'host') next.set('group', groupBy);
      else next.delete('group');
      if (providerFilter) next.set('provider', providerFilter);
      else next.delete('provider');
      if (sizeFilter) next.set('size', sizeFilter);
      else next.delete('size');
      return next;
    }, { replace: true });
  }, [groupBy, providerFilter, sizeFilter, setSearchParams]);

  // Whichever entry control is mounted (single-line input or the multi-URL
  // textarea) — only one of the two exists at a time.
  useEffect(() => {
    (inputRef.current ?? textareaRef.current)?.focus();
  }, []);

  // What the entry box actually resolves to, and what it cannot probe. Feeds
  // the counter, the rejected-line callout, and the run button's enabled state
  // — the page must never look ready to probe an input with nothing in it.
  const parsedEntry = useMemo(() => parseUrlSetInput(url), [url]);

  // ── Data loading ────────────────────────────────────────────────────

  const configsQuery = useTestConfigsQuery(projectId, { intervalMs: 15_000 });
  const configs = useMemo(
    () => ((configsQuery.data ?? []) as Array<TestConfigListItem | TestConfig>).filter((config) => {
      const kind = 'endpoint_kind' in config ? config.endpoint_kind : config.endpoint.kind;
      // Only this page's own probe configs are watch entries — structural
      // test_kind wins ('url_probe' survives a rename), name prefix covers
      // legacy rows (isWatchlistConfig). Runs from other network-kind configs
      // (benchmark cells, canary, SDK endpoints) are excluded downstream too:
      // their config detail is never fetched and their names don't parse.
      return kind === 'network' && isWatchlistConfig(config);
    }),
    [configsQuery.data],
  );
  const configById = useMemo(
    () => new Map(configs.map((config) => [config.id, config])),
    [configs],
  );

  const runsQuery = useTestRunsQuery(projectId, DIAGNOSTIC_RUN_PARAMS, {
    intervalMs: 15_000,
    activeIntervalMs: 5_000,
  });
  const allRuns = useMemo(() => runsQuery.data ?? [], [runsQuery.data]);
  const loading = configsQuery.isPending || runsQuery.isPending;

  // List items carry the full endpoint object, so configs IN the 200-newest
  // list window need no detail fetch (the ~76-request fan-out this replaces).
  // Details are fetched ONLY for probe configs that runs reference but the
  // list window evicted — without them an evicted set run would attribute to
  // its first member only (name parse), or to nobody (#820 follow-up).
  const missingDetailConfigIds = useMemo(() => {
    // Until the list has loaded we can't tell what's missing — fetching
    // everything would recreate the fan-out this replaces.
    if (!configsQuery.data) return [];
    const listed = new Set(
      (configsQuery.data as Array<TestConfigListItem | TestConfig>).map((config) => config.id),
    );
    const ids = new Set<string>();
    for (const run of allRuns) {
      if (listed.has(run.test_config_id)) continue;
      // "Looks like a probe run": structural test_kind, or a legacy run whose
      // denormalized config_name carries the probe-page prefix.
      if (run.test_kind === 'url_probe' || (run.config_name && isWatchlistConfigName(run.config_name))) {
        ids.add(run.test_config_id);
      }
    }
    return [...ids].sort();
  }, [configsQuery.data, allRuns]);
  const configDetailQueries = useTestConfigDetailsQueries(missingDetailConfigIds);
  const configDetails = useMemo(() => {
    const details = new Map<string, TestConfig>();
    for (const query of configDetailQueries) {
      if (query.data) details.set(query.data.id, query.data);
    }
    return details;
  }, [configDetailQueries]);

  // Testers feed the runner-picker and the wake CTA, so online state must
  // track reality — poll at 30s (freshness audit; runner heartbeats are
  // coarser than run state, no need for the 15s run cadence). The api call is
  // issued synchronously in the callback so the perf-log 'poll' source tag
  // holds (see usePolling doc); projectId as resetKey restarts the loop with
  // an immediate tick on project switch. Auto-pick stays the default — the
  // picker only lets users override e.g. a specific region or version.
  const testersMountedRef = useRef(true);
  useEffect(() => {
    testersMountedRef.current = true;
    return () => { testersMountedRef.current = false; };
  }, []);
  usePolling(() => {
    testersApi.listTesters(projectId).then((rows) => {
      // Guard the late resolve after unmount (what the old effect's
      // cancelled flag prevented).
      if (testersMountedRef.current) setTesters(rows);
    }).catch(() => {});
  }, 30_000, !!projectId, projectId);

  // ── Build URL groups ──────────────────────────────────────────────

  // Clock from state rather than read during render (react-hooks/purity); it is
  // a memo dependency so staleness verdicts advance with it.
  const now = useNow();

  // A run belongs to a set config when its config carries several member
  // URLs — list item first (the list wire sends the full endpoint), then the
  // fetched detail (evicted configs), then the "Diag set:" name as a last
  // resort while a detail loads.
  const isSetRun = useMemo(() => {
    return (run: TestRun): boolean => {
      const cfg = configById.get(run.test_config_id);
      if (cfg?.endpoint?.kind === 'network' && (cfg.endpoint.hosts?.length ?? 0) > 1) {
        return true;
      }
      const detail = configDetails.get(run.test_config_id);
      if (detail && detail.endpoint.kind === 'network' && (detail.endpoint.hosts?.length ?? 0) > 1) {
        return true;
      }
      const name = run.config_name ?? cfg?.name ?? '';
      return isDiagSetConfigName(name);
    };
  }, [configById, configDetails]);

  // ── Runner identity per run (provider / capacity axes) ────────────────
  // Denormalized runner_* list fields first; the testers map fills gaps by
  // tester_id; neither → the explicit unknown-runner bucket.
  const testersById = useMemo(
    () => new Map(testers.map(t => [t.tester_id, t])),
    [testers],
  );
  // Every run in allRuns has an entry; NO_RUNNER is only the type-level
  // fallback for a lookup miss.
  const runnerByRunId = useMemo(() => {
    const map = new Map<string, RunnerIdentity>();
    for (const run of allRuns) map.set(run.id, runnerIdentityForRun(run, testersById));
    return map;
  }, [allRuns, testersById]);
  const allRunners = useMemo(
    () => allRuns.map(run => runnerByRunId.get(run.id) ?? NO_RUNNER),
    [allRuns, runnerByRunId],
  );

  // Filter options come from what is actually loaded — provider over every
  // run, size over the runs under the chosen provider (so the size list
  // never offers an Azure SKU while GCP is selected).
  const providerOpts = useMemo(() => providerOptions(allRunners), [allRunners]);
  const capacityOpts = useMemo(
    () => capacityOptions(allRunners.filter(r => matchesRunnerFilter(r, providerFilter || null, null))),
    [allRunners, providerFilter],
  );

  // Runs that survive the provider / size filters — the grouping input.
  const visibleRuns = useMemo(
    () => (providerFilter || sizeFilter
      ? allRuns.filter(run => matchesRunnerFilter(
        runnerByRunId.get(run.id) ?? NO_RUNNER, providerFilter || null, sizeFilter || null,
      ))
      : allRuns),
    [allRuns, runnerByRunId, providerFilter, sizeFilter],
  );

  const baseUrlGroups = useMemo(() => {
    const bucketMap = new Map<string, {
      bucket: ProbeBucket;
      runs: TestRun[];
      configIds: Set<string>;
      runners: RunnerIdentity[];
    }>();

    for (const run of visibleRuns) {
      // Hosts this run covers: a single-URL config yields one; a set config
      // (#820) yields EVERY member, so the run lands on each member's row.
      // Resolution order: list item (endpoint-aware, no fetch needed) →
      // fetched detail (configs evicted from the list window) → the run's
      // denormalized config_name (last resort while a detail loads — a set
      // name only carries its first member).
      let hosts: string[] = [];

      const cfg = configById.get(run.test_config_id);
      if (cfg) hosts = hostsForDiagConfig(cfg);

      if (hosts.length === 0) {
        const detail = configDetails.get(run.test_config_id);
        if (detail) hosts = hostsForDiagConfig(detail);
      }
      if (hosts.length === 0 && run.config_name) {
        hosts = hostsFromDiagConfigName(run.config_name);
      }

      // Under host grouping the bucket IS the host (unchanged behaviour);
      // under provider / capacity the same run lands on the host's row for
      // its runner's cloud (and size).
      const runner = runnerByRunId.get(run.id) ?? NO_RUNNER;
      for (const host of hosts) {
        const bucket = probeBucketFor(host, groupBy, runner);
        let entry = bucketMap.get(bucket.key);
        if (!entry) {
          entry = { bucket, runs: [], configIds: new Set(), runners: [] };
          bucketMap.set(bucket.key, entry);
        }
        entry.runs.push(run);
        entry.configIds.add(run.test_config_id);
        entry.runners.push(runner);
      }
    }

    const groups: UrlGroup[] = [];
    for (const { bucket, runs, configIds, runners } of bucketMap.values()) {
      // Sort runs by created_at desc
      runs.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime());
      const lastRun = runs[0];

      groups.push({
        key: bucket.key,
        host: bucket.host,
        bucket,
        runner: summarizeBucketRunners(runners),
        runs,
        configIds,
        lastRun,
        // Run-level verdict (probeRunVerdict, lib/watchlist.ts). For set runs
        // this is provisional — refined below with per-URL attempt tallies.
        lastStatus: probeRunVerdict(lastRun, now, STALE_THRESHOLD_MS),
        totalDurationMs: getDurationMs(lastRun),
      });
    }

    return groups;
  }, [visibleRuns, configById, configDetails, now, groupBy, runnerByRunId]);

  // ── Per-URL health for set runs (#820) ────────────────────────────────
  // A set run's run-level counts aggregate every member URL — one flaky
  // member must not paint the others red. For each row whose LATEST run is a
  // finished set run, fetch that run's attempts (cached/shared with the run
  // detail page) and re-verdict the row from the attempts attributed to its
  // host via target_url. The per-URL override applies to status==='failed'
  // runs too — that is WHY they are included here: a watchdog-killed set run
  // whose attempts show a member went {ok:4, fail:0} renders that member
  // healthy/stale, not red (probeRunVerdict counts override run status).
  const setRunIds = useMemo(() => {
    const ids = new Set<string>();
    for (const g of baseUrlGroups) {
      const status = g.lastRun.status;
      if (status !== 'completed' && status !== 'failed') continue;
      if (isSetRun(g.lastRun)) ids.add(g.lastRun.id);
    }
    return [...ids].sort();
  }, [baseUrlGroups, isSetRun]);

  const setRunAttemptQueries = useRunsAttemptsQueries(setRunIds);
  const setRunAttempts = useMemo(() => {
    const map = new Map<string, NonNullable<(typeof setRunAttemptQueries)[number]['data']>>();
    setRunIds.forEach((runId, i) => {
      const data = setRunAttemptQueries[i]?.data;
      if (data) map.set(runId, data);
    });
    return map;
  }, [setRunIds, setRunAttemptQueries]);

  const urlGroups = useMemo(() => {
    return baseUrlGroups.map(group => {
      const attempts = setRunAttempts.get(group.lastRun.id);
      if (!attempts) return group;
      const mine = attempts.filter(
        a => a.target_url && extractHost(a.target_url) === group.host,
      );
      if (mine.length === 0) {
        // Two different absences (#820 follow-up): a pre-#782 tester persisted
        // no target_url on ANY attempt — the run-level verdict is the best
        // signal we have. But when the run DID attribute attempts and this
        // member just has none, its evidence was lost — no evidence must not
        // render green: {ok:0, fail:0} yields 'pending' via probeRunVerdict.
        if (!attempts.some(a => a.target_url)) return group;
        return {
          ...group,
          lastStatus: probeRunVerdict(group.lastRun, now, STALE_THRESHOLD_MS, { ok: 0, fail: 0 }),
        };
      }
      const ok = mine.filter(a => a.success).length;
      return {
        ...group,
        lastStatus: probeRunVerdict(group.lastRun, now, STALE_THRESHOLD_MS, {
          ok,
          fail: mine.length - ok,
        }),
      };
    });
  }, [baseUrlGroups, setRunAttempts, now]);

  // ── Summary counts ────────────────────────────────────────────────

  const summary = useMemo(() => {
    const total = urlGroups.length;
    const healthy = urlGroups.filter(g => g.lastStatus === 'healthy').length;
    const partial = urlGroups.filter(g => g.lastStatus === 'partial').length;
    const failed = urlGroups.filter(g => g.lastStatus === 'failed').length;
    const stale = urlGroups.filter(g => g.lastStatus === 'stale').length;
    const pending = urlGroups.filter(g => g.lastStatus === 'pending').length;
    return { total, healthy, partial, failed, stale, pending };
  }, [urlGroups]);

  // ── Filter + Sort + Paginate ──────────────────────────────────────

  const filteredGroups = useMemo(() => {
    let result = urlGroups;

    // Filter
    if (filter !== 'all') result = result.filter(g => g.lastStatus === filter);

    // Sort
    result = [...result].sort((a, b) => {
      switch (sort) {
        case 'last-checked':
          return new Date(b.lastRun.created_at).getTime() - new Date(a.lastRun.created_at).getTime();
        case 'name':
          // Same host under provider/capacity grouping: keep its rows together.
          return a.host.localeCompare(b.host) || a.key.localeCompare(b.key);
        case 'slowest':
          return (b.totalDurationMs ?? 0) - (a.totalDurationMs ?? 0);
        case 'most-runs':
          return b.runs.length - a.runs.length;
        default:
          return 0;
      }
    });

    return result;
  }, [urlGroups, filter, sort]);

  const totalPages = Math.max(1, Math.ceil(filteredGroups.length / PAGE_SIZE));
  const safePage = Math.min(page, totalPages);
  const paginatedGroups = filteredGroups.slice((safePage - 1) * PAGE_SIZE, safePage * PAGE_SIZE);

  // ── URL-set selection (#782 P1) ───────────────────────────────────────
  // Resolved against the FILTERED rows only: the set is what the user can see
  // ticked. Keys for rows the current filter hides simply do not resolve, and
  // come back if the filter does — nothing is probed off-screen either way.
  const selectedHosts = useMemo(
    () => hostsForSelection(filteredGroups, selectedKeys),
    [filteredGroups, selectedKeys],
  );
  const pageAllSelected =
    paginatedGroups.length > 0 && paginatedGroups.every(g => selectedKeys.has(g.key));

  // ── Recent hosts ──────────────────────────────────────────────────

  const recentHosts = useMemo(() => {
    // Copy before sorting — urlGroups is a memoized array shared with the
    // summary/filter pipeline; sorting it in place mutates that cache.
    // Grouped views carry several rows per host — dedupe before slicing.
    const hosts = [...urlGroups]
      .sort((a, b) => new Date(b.lastRun.created_at).getTime() - new Date(a.lastRun.created_at).getTime())
      .map(g => g.host);
    return [...new Set(hosts)].slice(0, 8);
  }, [urlGroups]);

  // ── Handlers ──────────────────────────────────────────────────────

  const handleRun = async (targetHost?: string) => {
    // Multi-URL set (#782): the input accepts several URLs separated by
    // whitespace, commas, or newlines — they are probed TOGETHER in one run
    // (same tick, comparable conditions) via endpoint.hosts[]. `samples` > 1
    // bursts every mode N times per URL within that run (#782 P2).
    //
    // Parse FIRST so the launch is honest about what it dropped: an unusable
    // line would otherwise become a `--target` that fails on the runner, and
    // an over-cap set would become a run the watchdog kills halfway. Both are
    // reported before the run starts, never after.
    const parsed = parseUrlSetInput(targetHost || url);
    const launchInput = launchInputFor(parsed);
    if (!launchInput) {
      const firstReject = parsed.invalid[0];
      addToast('error', firstReject
        ? `Nothing to probe — "${firstReject.raw}" is ${firstReject.reason}`
        : 'Enter a URL or hostname to test');
      return;
    }
    if (parsed.invalid.length > 0) {
      const shown = parsed.invalid.slice(0, 2).map(r => `"${r.raw}" (${r.reason})`).join('; ');
      const more = parsed.invalid.length - Math.min(2, parsed.invalid.length);
      addToast('info', `Skipped ${parsed.invalid.length} unusable entr${parsed.invalid.length === 1 ? 'y' : 'ies'}: ${shown}${more > 0 ? ` and ${more} more` : ''}`);
    }
    if (parsed.overflow.length > 0) {
      addToast('info', `A set is capped at ${MAX_SET_URLS} URLs — ${parsed.overflow.length} entr${parsed.overflow.length === 1 ? 'y was' : 'ies were'} not included`);
    }
    const diag = buildDiagRequest(launchInput, preset, samples);
    if (!diag) {
      addToast('error', 'Enter a URL or hostname to test');
      return;
    }
    const { host, entries, isSet, configName, config } = diag;

    setSubmitting(true);
    try {
      // `test_config` has UNIQUE (project_id, name), so re-running a diagnostic
      // against the same host+preset must reuse the existing config rather than
      // try (and fail) to create a duplicate. This also keeps the Watched URLs
      // list grouped by host instead of exploding every click. The client-side
      // find is only a fast path over the 200-newest list window — when the
      // name fell out of it, the create itself is idempotent (find_or_create,
      // #812) and returns the existing config instead of 409.
      const existing = configs.find(c => c.name === configName);
      let configId: string;
      let createdOrExisting: TestConfig | TestConfigListItem;
      if (existing) {
        configId = existing.id;
        createdOrExisting = existing;
      } else {
        const created = await runsApi.createConfig(projectId, config);
        configId = created.id;
        createdOrExisting = created;
        queryClient.setQueryData(runKeys.config(created.id), created);
        queryClient.setQueryData<TestConfigListItem[]>(runKeys.configs(projectId), (previous = []) => [
          {
            id: created.id,
            project_id: created.project_id,
            name: created.name,
            test_kind: created.test_kind,
            // Full endpoint, matching the list wire DTO — the structural
            // set-membership paths (hostsForDiagConfig / isSetRun) must see
            // every member host immediately, not after the next list refetch.
            endpoint: created.endpoint,
            endpoint_kind: created.endpoint.kind,
            modes: created.workload.modes,
            has_methodology: created.methodology !== null,
            created_at: created.created_at,
            updated_at: created.updated_at,
          },
          ...previous.filter((item) => item.id !== created.id),
        ]);
      }
      const run = await runsApi.launchConfig(configId, selectedTesterId ?? undefined);
      addToast('success', isSet
        ? `Diagnostic set ${run.id.slice(0, 8)} launched for ${entries.length} URLs`
        : `Diagnostic ${run.id.slice(0, 8)} launched for ${host}`);

      queryClient.setQueryData<TestRun[]>(runKeys.list(projectId, DIAGNOSTIC_RUN_PARAMS), (previous = []) => [
        run,
        ...previous.filter((item) => item.id !== run.id),
      ]);
      if ('endpoint' in createdOrExisting) {
        queryClient.setQueryData(runKeys.config(configId), createdOrExisting);
      }
    } catch (e) {
      addToast('error', `Diagnostic failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  const handleRemove = async (host: string, configIds: Set<string>) => {
    // Set configs (#820) are SHARED between their member URLs — deleting one
    // to remove a single host would erase the other members' history too
    // (test_run rows go with the config, ON DELETE CASCADE). Only configs
    // POSITIVELY classified single-URL are deleted; shared sets are kept, and
    // — the fail-safe inversion — a config we cannot classify at all (evicted
    // from the list window, detail not loaded, no run-borne name) is ALSO
    // kept: guessing "single" on an evicted set config would destroy every
    // member's history.
    const removable: string[] = [];
    let sharedSets = 0;
    let keptUnknown = 0;
    for (const id of configIds) {
      const cfg = configById.get(id);
      const detail = configDetails.get(id);
      // Same name fallback chain the host/set classifiers use: list item →
      // detail → the denormalized config_name any of this row's runs carry.
      const name =
        detail?.name
        ?? cfg?.name
        ?? allRuns.find(r => r.test_config_id === id && r.config_name)?.config_name
        ?? null;
      const structuralSet =
        (cfg?.endpoint?.kind === 'network' && (cfg.endpoint.hosts?.length ?? 0) > 1) ||
        (detail?.endpoint.kind === 'network' && (detail.endpoint.hosts?.length ?? 0) > 1);
      if (structuralSet || (name !== null && isDiagSetConfigName(name))) {
        sharedSets += 1;
      } else if (name === null) {
        keptUnknown += 1;
      } else {
        removable.push(id);
      }
    }
    const kept = sharedSets + keptUnknown;
    const keptNote = keptUnknown > 0
      ? `${kept} config${kept !== 1 ? 's' : ''} kept: shared sets or unidentifiable`
      : `${sharedSets} shared set config${sharedSets !== 1 ? 's' : ''} kept`;
    try {
      await Promise.all(removable.map(id => runsApi.deleteConfig(id)));
      if (removable.length === 0 && kept > 0) {
        addToast('info', keptUnknown > 0
          ? `Nothing deleted for ${host} — ${keptNote} (removal only deletes configs it can prove are single-URL)`
          : `${host} only has history from multi-URL set runs — shared set configs were kept`);
        return;
      }
      addToast(
        'success',
        kept > 0
          ? `Removed ${host} from watchlist (${keptNote})`
          : `Removed ${host} from watchlist`,
      );
      const removed = new Set(removable);
      queryClient.setQueryData<TestRun[]>(runKeys.list(projectId, DIAGNOSTIC_RUN_PARAMS), (previous = []) =>
        previous.filter((run) => !removed.has(run.test_config_id)),
      );
      queryClient.setQueryData<TestConfigListItem[]>(runKeys.configs(projectId), (previous = []) =>
        previous.filter((config) => !removed.has(config.id)),
      );
      for (const configId of removed) {
        queryClient.removeQueries({ queryKey: runKeys.config(configId) });
      }
    } catch (e) {
      addToast('error', `Failed to remove: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  // ── Hourly monitoring (#782): standing re-probe of a watched URL/set ──
  // Rides the existing test_schedule machinery (SchedulerService fires cron
  // schedules every ~30s tick) — the probe page only creates/toggles rows.
  const schedulesQuery = useSchedulesQuery(projectId);
  const schedules = useMemo(() => schedulesQuery.data ?? [], [schedulesQuery.data]);
  const updateSchedule = useUpdateScheduleMutation(projectId);

  // In-flight guard for "Monitor hourly" (keyed by group.key): a double
  // click, or two clicks before the schedules query refreshes, must not
  // create duplicate schedule rows.
  const [monitorPending, setMonitorPending] = useState<Set<string>>(new Set());

  const scheduleForGroup = (group: UrlGroup): TestSchedule | null =>
    // Only THIS page's hourly cron qualifies — an API-created daily schedule
    // on the same config must not render "Monitoring hourly ✓". Prefer the
    // schedule on the config that produced the latest run; fall back to any
    // hourly schedule on one of the row's configs.
    schedules.find(s => s.cron_expr === HOURLY_CRON && s.test_config_id === group.lastRun.test_config_id)
      ?? schedules.find(s => s.cron_expr === HOURLY_CRON && group.configIds.has(s.test_config_id))
      ?? null;

  /** Member URLs of a schedule's config — >1 means pausing it silences a
   *  whole set, not just the card it was clicked from (#820 follow-up). */
  const scheduleMemberCount = (configId: string): number => {
    const cfg = configById.get(configId);
    const detail = configDetails.get(configId);
    const ep = cfg?.endpoint?.kind === 'network'
      ? cfg.endpoint
      : detail?.endpoint.kind === 'network'
        ? detail.endpoint
        : undefined;
    return Math.max(ep?.hosts?.length ?? 1, 1);
  };

  const handleMonitorHourly = async (group: UrlGroup) => {
    if (monitorPending.has(group.key)) return;
    setMonitorPending(prev => new Set(prev).add(group.key));
    try {
      // Re-check against the FRESHEST schedule data before POSTing — the
      // schedule may exist already (created from another member's card, or a
      // click that raced the 10s poll): re-enable a paused one, no-op an
      // active one, create only when truly absent.
      const fresh = (await schedulesQuery.refetch()).data ?? schedules;
      const existing = fresh.find(s =>
        s.cron_expr === HOURLY_CRON
        && (s.test_config_id === group.lastRun.test_config_id || group.configIds.has(s.test_config_id)));
      if (existing && existing.enabled) {
        addToast('info', `${group.host} is already monitored hourly`);
      } else if (existing) {
        await runsApi.updateSchedule(existing.id, { enabled: true });
        addToast('success', `Resumed hourly monitoring for ${group.host}`);
      } else {
        await runsApi.createSchedule(projectId, {
          test_config_id: group.lastRun.test_config_id,
          cron_expr: HOURLY_CRON,
          timezone: 'UTC',
          enabled: true,
        });
        addToast('success', `Monitoring ${group.host} hourly`);
      }
      queryClient.invalidateQueries({ queryKey: runKeys.schedules(projectId) });
    } catch (e) {
      addToast('error', `Failed to schedule: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setMonitorPending(prev => {
        const next = new Set(prev);
        next.delete(group.key);
        return next;
      });
    }
  };

  const handleToggleMonitor = (group: UrlGroup, schedule: TestSchedule) => {
    // A shared set's schedule covers EVERY member URL — pausing it from one
    // member's card silences them all, so the toast must say so (#820).
    const others = scheduleMemberCount(schedule.test_config_id) - 1;
    const scope = others > 0
      ? `${group.host} and ${others} more URL${others !== 1 ? 's' : ''} in its set`
      : group.host;
    updateSchedule.mutate(
      { scheduleId: schedule.id, enabled: !schedule.enabled },
      {
        onSuccess: () =>
          addToast(
            'success',
            schedule.enabled
              ? `Paused hourly monitoring for ${scope}`
              : `Resumed hourly monitoring for ${scope}`,
          ),
        onError: (e) =>
          addToast('error', `Failed to update schedule: ${e instanceof Error ? e.message : String(e)}`),
      },
    );
  };

  const focusEntry = () => {
    (multiMode ? textareaRef.current : inputRef.current)?.focus();
  };

  /** Switch entry mode, re-laying out the SAME text (one per line vs inline). */
  const handleEntryModeChange = (next: boolean) => {
    setMultiMode(next);
    setUrl(prev => (next ? formatUrlSetInput(splitUrlSetInput(prev)) : splitUrlSetInput(prev).join(' ')));
  };

  /** Load the ticked rows into the multi-URL box so they can be edited first. */
  const handleEditSelectionAsList = () => {
    if (selectedHosts.length === 0) return;
    setMultiMode(true);
    setUrl(formatUrlSetInput(selectedHosts));
    // The textarea mounts on this render; focus after it exists.
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  /**
   * Open the comparison report (#782 P3) for the ticked rows.
   *
   * The selection is normalised with the SAME `toProbeUrl` the launch payload
   * uses, so the URLs handed to the report are byte-identical to the
   * `target_url` the tester stamps on each attempt — the report keys on that,
   * and a host-shaped string would silently match nothing.
   */
  const handleCompareSelection = () => {
    if (selectedHosts.length < 2) {
      addToast('error', 'Tick at least two watched URLs to compare them');
      return;
    }
    const urls = selectedHosts.slice(0, MAX_SET_URLS).map(toProbeUrl);
    navigate(`/projects/${projectId}/probe/compare?urls=${encodeURIComponent(urls.join(','))}`);
  };

  /** Probe every ticked row TOGETHER — one config, one run, N `--target`s. */
  const handleProbeSet = () => {
    if (selectedHosts.length === 0) {
      addToast('error', 'Tick at least one watched URL first');
      return;
    }
    void handleRun(selectedHosts.join(' '));
  };

  const handleHostClick = (host: string) => {
    // Single mode replaces the field (unchanged). Multi mode APPENDS — the
    // chips are how you assemble a set from history, and clobbering four
    // pasted URLs because you clicked a fifth would be the wrong answer.
    setUrl(prev => {
      if (!multiMode) return host;
      const entries = splitUrlSetInput(prev);
      return formatUrlSetInput(entries.includes(host) ? entries : [...entries, host]);
    });
    focusEntry();
    // Scroll to the host's first card if it exists (grouped views have one
    // per runner bucket; the first in display order is the nearest match).
    const target = filteredGroups.find(g => g.host === host);
    const card = target ? document.getElementById(`card-body-${target.key}`) : null;
    if (target && card) {
      card.closest('[class*="border-gray-800"]')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      setExpandedCards(prev => new Set(prev).add(target.key));
    }
  };

  const toggleCard = (key: string) => {
    setExpandedCards(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // Reset the page when the filter/sort changes. Done DURING RENDER via the
  // previous-value comparison React documents for this, rather than in an
  // effect: the effect version rendered page N of the new filter first, then
  // re-rendered at page 1 — a visible flash of the wrong slice.
  const filterKey = `${filter}\u0000${sort}\u0000${groupBy}\u0000${providerFilter}\u0000${sizeFilter}`;
  const [prevFilterKey, setPrevFilterKey] = useState(filterKey);
  if (filterKey !== prevFilterKey) {
    setPrevFilterKey(filterKey);
    setPage(1);
  }

  // ── Render ────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-8 max-w-[1200px]">
      {/* Page header */}
      <div className="flex items-start justify-between mb-6">
        <div>
          <h1 className="text-[22px] font-semibold text-gray-200 tracking-tight">URL Probe</h1>
          <p className="text-xs text-gray-400 mt-1">Discover what features a URL supports — protocols, TLS, certificates, ALPN.</p>
        </div>
        <Link
          to={`/projects/${projectId}/tls-profiles`}
          className="text-xs text-gray-400 hover:text-cyan-400 transition-colors whitespace-nowrap mt-1"
        >
          TLS profiles →
        </Link>
      </div>

      <RunnerAvailabilityBanner
        projectId={projectId}
        testers={testers}
        queuedCount={allRuns.filter(r => r.status === 'queued').length}
        onWoken={() => testersApi.listTesters(projectId).then(setTesters).catch(() => {})}
      />

      {/* Probe input bar */}
      <div className="border border-gray-800 rounded p-4 mb-7">
        <div className="flex items-center justify-between gap-3 mb-2.5">
          <div className="text-xs tracking-wider text-faint">
            {multiMode ? 'Probe a URL set' : 'Probe a URL'}
          </div>
          <div className="flex border border-gray-800 rounded overflow-hidden text-xs">
            {([false, true] as const).map(mode => (
              <button
                key={String(mode)}
                onClick={() => handleEntryModeChange(mode)}
                className={`px-2.5 py-1 border-r border-gray-800 last:border-r-0 transition-colors ${
                  multiMode === mode ? 'bg-white/5 text-gray-200' : 'text-faint hover:text-gray-400'
                }`}
                aria-pressed={multiMode === mode}
                title={mode
                  ? 'Paste a list of URLs — one per line, all probed together in ONE run'
                  : 'Probe a single URL'}
              >
                {mode ? 'URL set' : 'Single URL'}
              </button>
            ))}
          </div>
        </div>

        {/* Multi-URL entry (#782 P1): one URL per line. The set is probed in a
            SINGLE tester invocation (repeated --target), so every member sees
            the same network conditions and the run list gains one row, not N. */}
        {multiMode && (
          <div className="mb-2.5">
            <label htmlFor="diag-url-set" className="text-xs text-gray-400 block mb-1.5">
              URLs (one per line):
            </label>
            <textarea
              ref={textareaRef}
              id="diag-url-set"
              rows={5}
              value={url}
              onChange={e => setUrl(e.target.value)}
              onKeyDown={e => {
                // Enter inserts a newline here (it is a list); Ctrl/Cmd+Enter
                // launches, the usual multi-line-field submit gesture.
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && parsedEntry.entries.length > 0) {
                  e.preventDefault();
                  void handleRun();
                }
              }}
              placeholder={'example.com\nhttps://www.cloudflare.com/\napi.example.com/health'}
              className="w-full bg-[var(--bg-raised)] border border-gray-800 rounded px-3 py-2 text-sm text-cyan-400 focus:outline-none focus:border-cyan-500/50 placeholder:text-gray-600 transition-colors [font-variant-ligatures:none] font-mono resize-y"
              aria-label="URLs to probe together, one per line"
              aria-describedby="diag-url-set-summary"
            />
            <div id="diag-url-set-summary" className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
              <span className={parsedEntry.entries.length > 0 ? 'text-gray-400' : 'text-faint'}>
                {parsedEntry.entries.length} URL{parsedEntry.entries.length === 1 ? '' : 's'}
                {parsedEntry.entries.length > 1 ? ' — probed together in one run' : ''}
              </span>
              {parsedEntry.duplicates.length > 0 && (
                <span className="text-faint">
                  {parsedEntry.duplicates.length} duplicate{parsedEntry.duplicates.length === 1 ? '' : 's'} collapsed
                </span>
              )}
              {parsedEntry.overflow.length > 0 && (
                <span className="text-yellow-400" title={parsedEntry.overflow.map(r => r.raw).join('\n')}>
                  {parsedEntry.overflow.length} over the {MAX_SET_URLS}-URL cap — not probed
                </span>
              )}
              {parsedEntry.invalid.length > 0 && (
                <span className="text-red-400" title={parsedEntry.invalid.map(r => `${r.raw} — ${r.reason}`).join('\n')}>
                  {parsedEntry.invalid.length} unusable: {parsedEntry.invalid.slice(0, 2).map(r => r.raw).join(', ')}
                  {parsedEntry.invalid.length > 2 ? ` +${parsedEntry.invalid.length - 2}` : ''}
                </span>
              )}
            </div>
          </div>
        )}

        <div className="flex items-center gap-2.5">
          {multiMode ? (
            <div className="flex-1" />
          ) : (
            <div className="flex-1 flex items-center gap-2">
              <label htmlFor="diag-url" className="text-xs text-gray-400 flex-shrink-0">URL:</label>
              {/* Ligatures off (#765): coding fonts render `//` as a slashed
                  ligature that reads as ` /` in a URL field. The stored value
                  was always correct — display only. */}
              <input
                ref={inputRef}
                id="diag-url"
                type="text"
                value={url}
                onChange={e => setUrl(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter' && url.trim()) handleRun(); }}
                placeholder="Enter URL(s) to test — several at once, separated by spaces or commas..."
                className="flex-1 bg-[var(--bg-raised)] border border-gray-800 rounded px-3 py-2 text-sm text-cyan-400 focus:outline-none focus:border-cyan-500/50 placeholder:text-gray-600 transition-colors [font-variant-ligatures:none]"
                aria-label="URL or hostname to test"
              />
            </div>
          )}
          <label htmlFor="diag-preset" className="text-xs text-gray-400">Preset</label>
          <select
            id="diag-preset"
            value={preset}
            onChange={e => setPreset(e.target.value as DiagPreset)}
            className="bg-[var(--bg-raised)] border border-gray-800 rounded px-3 py-2 text-xs text-gray-400 focus:outline-none appearance-none pr-7 cursor-pointer"
            style={{
              backgroundImage: `url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%23475569' stroke-width='1.5'/%3E%3C/svg%3E")`,
              backgroundRepeat: 'no-repeat',
              backgroundPosition: 'right 10px center',
            }}
          >
            {(['quick', 'standard', 'full', 'route'] as DiagPreset[]).map(p => (
              <option key={p} value={p}>
                {p.charAt(0).toUpperCase() + p.slice(1)} ({DIAG_PRESET_LABELS[p].time})
              </option>
            ))}
          </select>
          <label htmlFor="diag-samples" className="text-xs text-gray-400">Samples</label>
          <select
            id="diag-samples"
            value={samples}
            onChange={e => setSamples(Number(e.target.value) as DiagSamples)}
            className="bg-[var(--bg-raised)] border border-gray-800 rounded px-3 py-2 text-xs text-gray-400 focus:outline-none appearance-none pr-7 cursor-pointer"
            style={{
              backgroundImage: `url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%23475569' stroke-width='1.5'/%3E%3C/svg%3E")`,
              backgroundRepeat: 'no-repeat',
              backgroundPosition: 'right 10px center',
            }}
            title="Burst sampling: probe every mode N times per URL in one run — median/p95 become meaningful within a single point"
          >
            {DIAG_SAMPLE_CHOICES.map(n => (
              <option key={n} value={n}>
                {n === 1 ? '1 (single)' : `${n} (burst)`}
              </option>
            ))}
          </select>
          <label htmlFor="diag-runner" className="text-xs text-gray-400">Runner</label>
          <select
            id="diag-runner"
            value={selectedTesterId ?? ''}
            onChange={e => setSelectedTesterId(e.target.value || null)}
            className="bg-[var(--bg-raised)] border border-gray-800 rounded px-3 py-2 text-xs text-gray-400 focus:outline-none appearance-none pr-7 cursor-pointer max-w-[14rem]"
            style={{
              backgroundImage: `url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%23475569' stroke-width='1.5'/%3E%3C/svg%3E")`,
              backgroundRepeat: 'no-repeat',
              backgroundPosition: 'right 10px center',
            }}
            title="Pick a specific runner or leave auto-pick to let the dispatcher choose"
          >
            <option value="">auto-pick</option>
            {testers
              .filter(t => t.power_state === 'running')
              .map(t => {
                const v = t.installer_version ?? '?';
                return (
                  <option key={t.tester_id} value={t.tester_id}>
                    {t.name} ({t.cloud}/{t.region}) · v{v}
                  </option>
                );
              })}
          </select>
          <Button
            variant="primary"
            onClick={() => handleRun()}
            // Enabled by what is actually PROBEABLE, not by "the box has text":
            // a box holding only unusable lines must not look ready to run.
            disabled={submitting || parsedEntry.entries.length === 0}
            className="w-9 h-9 !p-0 text-base flex-shrink-0"
            aria-label={parsedEntry.entries.length > 1
              ? `Probe ${parsedEntry.entries.length} URLs together`
              : 'Run diagnostic'}
            title={parsedEntry.entries.length > 1
              ? `Probe ${parsedEntry.entries.length} URLs together in one run`
              : 'Run diagnostic'}
          >
            {submitting ? (
              <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full motion-safe:animate-spin" />
            ) : (
              '\u25B6'
            )}
          </Button>
        </div>
      </div>

      {/* Recent host chips */}
      {recentHosts.length > 0 && (
        <div className="mb-6 flex flex-wrap gap-1.5 items-center">
          <span className="text-xs text-faint mr-1">Recent:</span>
          {recentHosts.map(host => (
            <button
              key={host}
              onClick={() => handleHostClick(host)}
              className={`text-xs px-2 py-1 rounded border transition-colors ${
                extractHost(url) === host
                  ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-400'
                  : 'border-gray-800 text-gray-400 hover:border-gray-600 hover:text-gray-400'
              }`}
            >
              {host.length > 30 ? host.slice(0, 27) + '...' : host}
            </button>
          ))}
        </div>
      )}

      {/* Summary strip */}
      {!loading && urlGroups.length > 0 && (
        <div className="flex items-center gap-4 mb-4 text-xs">
          {/* Color carries signal — zero counts stay grey (audit: colored zeros). */}
          <span className="text-gray-400">
            <strong className="text-gray-400 font-medium">{summary.total}</strong>{' '}
            {groupBy === 'host'
              ? (summary.total === 1 ? 'URL' : 'URLs')
              : `URL × ${groupBy === 'provider' ? 'provider' : 'capacity'} ${summary.total === 1 ? 'row' : 'rows'}`}
          </span>
          <span className="text-gray-700">&middot;</span>
          <span className="text-gray-400">
            <strong className={`font-medium ${summary.healthy > 0 ? 'text-green-400' : 'text-faint'}`}>{summary.healthy}</strong> healthy
          </span>
          {summary.partial > 0 && (
            <>
              <span className="text-gray-700">&middot;</span>
              <span className="text-gray-400">
                <strong className="text-yellow-400 font-medium">{summary.partial}</strong> partial
              </span>
            </>
          )}
          <span className="text-gray-700">&middot;</span>
          <span className="text-gray-400">
            <strong className={`font-medium ${summary.failed > 0 ? 'text-red-400' : 'text-faint'}`}>{summary.failed}</strong> failed
          </span>
          {summary.pending > 0 && (
            <>
              <span className="text-gray-700">&middot;</span>
              <span className="text-gray-400">
                <strong className="text-cyan-400 font-medium">{summary.pending}</strong> pending
              </span>
            </>
          )}
          <span className="text-gray-700">&middot;</span>
          <span className="text-gray-400">
            <strong className={`font-medium ${summary.stale > 0 ? 'text-yellow-400' : 'text-faint'}`}>{summary.stale}</strong> stale (no check in 24h)
          </span>
        </div>
      )}

      {/* Toolbar */}
      <div className="flex items-center justify-between mb-3 gap-3 flex-wrap">
        <span className="text-xs tracking-wider text-faint">
          {groupBy === 'host'
            ? `Watched URLs (${filteredGroups.length})`
            : `Watched URLs × ${groupBy === 'provider' ? 'provider' : 'capacity'} (${filteredGroups.length})`}
        </span>
        <div className="flex items-center gap-3 flex-wrap">
          {/* Runner axes: group + provider + capacity filters */}
          <select
            value={groupBy}
            onChange={e => setGroupBy(parseProbeGroupBy(e.target.value))}
            className={TOOLBAR_SELECT_CLASS}
            style={TOOLBAR_SELECT_STYLE}
            aria-label="Group rows by"
            title="Split each host's history by the runner that probed it — Azure vs GCP numbers differ for infrastructure reasons, not network-path reasons"
          >
            <option value="host">By host</option>
            <option value="provider">By provider</option>
            <option value="capacity">By capacity</option>
          </select>
          <select
            value={providerFilter}
            onChange={e => { setProviderFilter(e.target.value); setSizeFilter(''); }}
            className={TOOLBAR_SELECT_CLASS}
            style={TOOLBAR_SELECT_STYLE}
            aria-label="Filter by runner provider"
            title="Only runs probed from this cloud provider"
          >
            <option value="">All providers</option>
            {providerOpts.map(o => (
              <option key={o.value} value={o.value}>{o.label} ({o.count})</option>
            ))}
            {providerFilter && !providerOpts.some(o => o.value === providerFilter) && (
              <option value={providerFilter}>{providerFilter} (no runs)</option>
            )}
          </select>
          <select
            value={sizeFilter}
            onChange={e => setSizeFilter(e.target.value)}
            className={`${TOOLBAR_SELECT_CLASS} max-w-[18rem]`}
            style={TOOLBAR_SELECT_STYLE}
            aria-label="Filter by runner VM size"
            title="Only runs probed from this runner VM size (vCPU / memory from the VM-size catalog)"
          >
            <option value="">All sizes</option>
            {capacityOpts.map(o => (
              <option key={o.value} value={o.value}>{o.label} ({o.count})</option>
            ))}
            {sizeFilter && !capacityOpts.some(o => o.value === sizeFilter) && (
              <option value={sizeFilter}>{sizeFilter === UNKNOWN_RUNNER ? 'unknown size' : sizeFilter} (no runs)</option>
            )}
          </select>
          {/* Filter toggle */}
          <div className="flex border border-gray-800 rounded overflow-hidden">
            {(['all', 'healthy', 'partial', 'pending', 'failed', 'stale'] as FilterMode[]).map(f => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={`px-3 py-1 text-xs border-r border-gray-800 last:border-r-0 transition-colors ${
                  filter === f
                    ? 'bg-white/5 text-gray-200'
                    : 'text-faint hover:text-gray-400'
                }`}
                aria-pressed={filter === f}
              >
                {f.charAt(0).toUpperCase() + f.slice(1)}
              </button>
            ))}
          </div>
          {/* Sort dropdown */}
          <select
            value={sort}
            onChange={e => setSort(e.target.value as SortMode)}
            className={TOOLBAR_SELECT_CLASS}
            style={TOOLBAR_SELECT_STYLE}
            aria-label="Sort URLs by"
          >
            <option value="last-checked">Last checked</option>
            <option value="name">Name</option>
            <option value="slowest">Slowest</option>
            <option value="most-runs">Most runs</option>
          </select>
        </div>
      </div>

      {/* ── URL-set action bar (#782 P1) ──────────────────────────────────
          Only rendered with a live selection: the watchlist is a reading
          surface first, and an always-on bar would claim vertical space for
          an action nobody asked for. */}
      {selectedHosts.length > 0 && (
        <div
          className="flex flex-wrap items-center gap-2 mb-3 border border-cyan-500/30 bg-cyan-500/[0.04] rounded px-3 py-2"
          role="region"
          aria-label="URL set actions"
        >
          <span className="text-xs text-cyan-300 font-medium tabular-nums">
            {selectedHosts.length} URL{selectedHosts.length === 1 ? '' : 's'} selected
          </span>
          <span className="text-xs text-faint">
            {selectedHosts.length > 1
              ? '— probed together in one run, same conditions'
              : '— one URL, a plain single probe'}
          </span>
          {selectedHosts.length > MAX_SET_URLS && (
            <span className="text-xs text-yellow-400">
              only the first {MAX_SET_URLS} will be probed
            </span>
          )}
          <span className="flex-1" />
          <Button
            variant="primary"
            size="xs"
            onClick={handleProbeSet}
            disabled={submitting}
            title={`Probe ${Math.min(selectedHosts.length, MAX_SET_URLS)} URL(s) in one run`}
          >
            Probe set now
          </Button>
          <Button
            variant="ghost"
            size="xs"
            onClick={handleEditSelectionAsList}
            title="Load the selection into the multi-URL box to edit before probing"
          >
            Edit as list
          </Button>
          {/* Compare (#782 P3). Disabled below two URLs because a comparison of
              one is not a comparison — the report needs a shared-bucket
              intersection to compute anything at all. The selection travels as
              toProbeUrl() output, which is the SAME normalisation the launch
              payload uses and therefore the exact string the tester stamps on
              each attempt as target_url — so the compare page's picker and this
              button agree on what a URL is called. */}
          <Button
            variant="ghost"
            size="xs"
            onClick={handleCompareSelection}
            disabled={selectedHosts.length < 2}
            title={
              selectedHosts.length < 2
                ? 'Select at least two URLs to compare them'
                : `Compare ${Math.min(selectedHosts.length, MAX_SET_URLS)} URLs over the hours they were all probed`
            }
          >
            Compare…
          </Button>
          <Button variant="ghost" size="xs" onClick={() => setSelectedKeys(new Set())}>
            Clear
          </Button>
        </div>
      )}

      {/* URL cards */}
      {loading ? (
        <div className="space-y-2">
          {[1, 2, 3, 4, 5].map(i => (
            <div key={i} className="border border-gray-800/50 rounded p-4 flex gap-4">
              <div className="h-3 w-40 bg-gray-800 rounded motion-safe:animate-pulse" />
              <div className="flex-1" />
              <div className="h-3 w-16 bg-gray-800/40 rounded motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      ) : paginatedGroups.length === 0 ? (
        <div className="border border-gray-800 rounded p-12 text-center">
          <p className="text-gray-400 text-sm">
            {providerFilter || sizeFilter
              ? 'No runs from that runner provider / size. Try another filter or pin a runner above and probe again.'
              : filter !== 'all'
                ? `No ${filter} URLs found. Try changing the filter.`
                : 'No probes yet. Enter a URL above to discover what it supports.'}
          </p>
        </div>
      ) : (
        <div>
          {/* Select-all covers THIS PAGE of rows, and says so — a control that
              silently reached the other 180 watched URLs would be a trap. */}
          <label className="flex items-center gap-2 px-4 py-1.5 mb-0.5 text-xs text-faint cursor-pointer w-fit">
            <input
              type="checkbox"
              checked={pageAllSelected}
              onChange={e => {
                const checked = e.target.checked;
                setSelectedKeys(prev => {
                  let next = prev;
                  for (const g of paginatedGroups) next = setSelectionKey(next, g.key, checked);
                  return next;
                });
              }}
              // 24 CSS px is the WCAG 2.2 AA minimum target size, and axe measures
            // the INPUT's own box — the label's padding around it does not count.
            // The repo's coarse-pointer hit-area rule only covers <button>, since
            // ::after does not render on a replaced element like a checkbox.
            className="w-6 h-6 accent-cyan-500 cursor-pointer"
            />
            Select all {paginatedGroups.length} on this page
          </label>
          {paginatedGroups.map(group => (
            <UrlCard
              key={group.key}
              group={group}
              projectId={projectId}
              expanded={expandedCards.has(group.key)}
              onToggle={() => toggleCard(group.key)}
              onRunAgain={host => handleRun(host)}
              onRemove={handleRemove}
              schedule={scheduleForGroup(group)}
              onMonitorHourly={() => handleMonitorHourly(group)}
              monitorPending={monitorPending.has(group.key)}
              onToggleMonitor={schedule => handleToggleMonitor(group, schedule)}
              selected={selectedKeys.has(group.key)}
              onSelectedChange={checked =>
                setSelectedKeys(prev => setSelectionKey(prev, group.key, checked))}
            />
          ))}
        </div>
      )}

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between mt-4 pt-4 border-t border-gray-800">
          <span className="text-xs text-faint">
            Showing {(safePage - 1) * PAGE_SIZE + 1}-{Math.min(safePage * PAGE_SIZE, filteredGroups.length)} of{' '}
            {filteredGroups.length} {groupBy === 'host' ? 'URLs' : 'rows'}
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setPage(p => Math.max(1, p - 1))}
              disabled={safePage <= 1}
              className="px-3 py-1 text-xs border border-gray-800 rounded text-gray-400 hover:text-gray-300 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              Previous
            </button>
            <span className="text-xs text-gray-400 tabular-nums px-2">
              {safePage} / {totalPages}
            </span>
            <button
              onClick={() => setPage(p => Math.min(totalPages, p + 1))}
              disabled={safePage >= totalPages}
              className="px-3 py-1 text-xs border border-gray-800 rounded text-gray-400 hover:text-gray-300 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Runner availability banner ───────────────────────────────────────────────
// A probe launched with zero online runners used to queue SILENTLY until the
// watchdog failed it (user-caught 2026-08-11, microsoft.com probe). The
// backend now auto-wakes a stopped idle runner within ~60s; this banner makes
// the state visible immediately and offers to start one right now.
function RunnerAvailabilityBanner({
  projectId,
  testers,
  queuedCount,
  onWoken,
}: {
  projectId: string;
  testers: TesterRow[];
  queuedCount: number;
  onWoken: () => void;
}) {
  const addToast = useToast();
  const [starting, setStarting] = useState(false);

  const online = testers.filter(isOnlineTester);
  if (online.length > 0) return null; // healthy — stay quiet

  const waking = testers.filter(t => t.power_state === 'starting');
  const wakeable = testers.filter(
    t => (t.power_state === 'stopped' || t.power_state === 'deallocated') && t.allocation === 'idle',
  );
  const queuedNote = queuedCount > 0
    ? ` ${queuedCount} probe${queuedCount !== 1 ? 's' : ''} queued — `
    : ' ';

  if (waking.length > 0) {
    return (
      <div className="border border-cyan-500/30 bg-cyan-500/5 rounded p-3 mb-4 text-xs text-cyan-300">
        No runner online —{queuedNote}waking <span className="text-cyan-200">{waking[0].name}</span> (~2 min);
        queued probes start automatically when it connects.
      </div>
    );
  }

  if (wakeable.length > 0) {
    return (
      <div className="border border-yellow-500/30 bg-yellow-500/5 rounded p-3 mb-4 text-xs text-yellow-300 flex items-center justify-between gap-3">
        <span>
          No runner online —{queuedNote}probes will queue and a stopped runner wakes automatically within ~1 min.
        </span>
        <button
          onClick={() => {
            setStarting(true);
            testersApi.startTester(projectId, wakeable[0].tester_id)
              .then(() => { addToast('info', `Starting ${wakeable[0].name}...`); onWoken(); })
              .catch((e) => addToast('error', `Start failed: ${e instanceof Error ? e.message : String(e)}`))
              .finally(() => setStarting(false));
          }}
          disabled={starting}
          className="px-3 py-1 border border-yellow-500/40 text-yellow-300 hover:bg-yellow-500/10 rounded transition-colors disabled:opacity-50 whitespace-nowrap"
        >
          {starting ? 'Starting…' : `Start ${wakeable[0].name} now`}
        </button>
      </div>
    );
  }

  return (
    <div className="border border-red-500/30 bg-red-500/5 rounded p-3 mb-4 text-xs text-red-300 flex items-center justify-between gap-3">
      <span>No runners in this project — probes cannot run.</span>
      <Link
        to={`/projects/${projectId}/vms`}
        className="px-3 py-1 border border-red-500/40 text-red-300 hover:bg-red-500/10 rounded transition-colors whitespace-nowrap"
      >
        Deploy a runner →
      </Link>
    </div>
  );
}
