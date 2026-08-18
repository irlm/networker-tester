import { useState, useEffect, useRef, useMemo } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useNow } from '../hooks/useNow';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { runsApi } from '../features/runs/api';
import { testersApi, type TesterRow } from '../api/testers';
import type { EndpointRef, TestConfig, TestConfigCreate, TestConfigListItem, TestRun, Workload } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { RunResult } from '../components/common/RunResult';
import { runDisplayStatus } from '../lib/runStatus';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { timeAgo } from '../lib/format';
import { stripAnsi } from '../lib/ansi';
import { Button } from '../components/common/Button';
import {
  runKeys,
  useTestConfigDetailsQueries,
  useTestConfigsQuery,
  useTestRunsQuery,
} from '../features/runs/queries';

// ── Types ───────────────────────────────────────────────────────────────

type DiagPreset = 'quick' | 'standard' | 'full' | 'route';
type FilterMode = 'all' | 'healthy' | 'partial' | 'failed' | 'pending' | 'stale';
type SortMode = 'last-checked' | 'name' | 'slowest' | 'most-runs';

interface UrlGroup {
  host: string;
  runs: TestRun[];
  configIds: Set<string>;
  lastRun: TestRun;
  lastStatus: 'healthy' | 'partial' | 'failed' | 'stale' | 'pending';
  totalDurationMs: number | null;
}

// ── Constants ───────────────────────────────────────────────────────────

// Endpoint-only modes (udp echo, native pageload asset ladder) are excluded:
// URL diagnostics always target arbitrary URLs, where those modes fail by
// construction (user-caught 2026-08-12 — 4 guaranteed-failed attempts per Full
// run). Real-site page load is covered by the browser* modes. Keep in lockstep
// with mode-capabilities.ts / shared/modes.json `requires`.
const DIAG_PRESETS: Record<DiagPreset, string[]> = {
  quick: ['dns', 'tcp', 'tls', 'http2'],
  standard: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3'],
  full: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'curl', 'browser1', 'browser2', 'browser3'],
  // Reachability & route diagnostics (v0.28.78 modes — all `any`-target).
  // ping may need ICMP privileges on the runner (Linux ping_group_range);
  // a denial surfaces as an honest per-attempt Config error, not a hang.
  route: ['ping', 'path', 'dualstack', 'pmtud'],
};

const DIAG_PRESET_LABELS: Record<DiagPreset, { time: string; desc: string }> = {
  quick: { time: '~3s', desc: 'dns, tcp, tls, http2' },
  standard: { time: '~12s', desc: '+ http1, http3, tls-resume, native-tls' },
  full: { time: '~45s', desc: '+ curl, browser page loads' },
  route: { time: '~45s', desc: 'ping, traceroute, v4-vs-v6, path MTU' },
};

const PAGE_SIZE = 20;
const STALE_THRESHOLD_MS = 24 * 60 * 60 * 1000; // 24 hours
const DIAGNOSTIC_RUN_PARAMS = { endpoint_kind: 'network', limit: 200 } as const;

const PHASE_CSS_COLORS: Record<string, string> = {
  dns: '#a78bfa',
  tcp: '#22d3ee',
  tls: '#f59e0b',
  ttfb: '#10b981',
  download: '#3b82f6',
};

// ── Helpers ─────────────────────────────────────────────────────────────

function extractHost(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return '';
  try {
    if (trimmed.includes('://')) {
      return new URL(trimmed).hostname;
    }
    const candidate = new URL(`https://${trimmed}`);
    return candidate.hostname;
  } catch {
    return trimmed;
  }
}

/**
 * Full URL to actually probe — the URL Probe hits the URL AS ENTERED (root when
 * no path is given), not `<host>/health`. A bare host becomes `https://<host>/`;
 * a full URL is preserved. Passed as the config's endpoint host so the agent
 * uses it verbatim (a bare host would get `/health` appended — the E2E P1-4
 * false-failure on arbitrary sites like example.com).
 */
function toProbeUrl(input: string): string {
  const t = input.trim();
  try {
    const u = t.includes('://') ? new URL(t) : new URL(`https://${t}`);
    return u.toString();
  } catch {
    return t;
  }
}

function getHostFromConfig(config: TestConfigListItem | TestConfig): string | null {
  // The endpoint is stored as JSON on TestConfigListItem, but we need to check
  // if endpoint info is available. For list items, we may need to parse from the name.
  if ('endpoint' in config) {
    const ep = (config as TestConfig).endpoint;
    if (ep.kind === 'network') return ep.host;
  }
  return null;
}

function getHostFromConfigName(name: string): string | null {
  // Parse "Probe: hostname (Preset)" or "Diag: hostname (Preset)" or just use as-is
  const probeMatch = name.match(/^(?:Probe|Diag):\s+(.+?)\s+\(/);
  if (probeMatch) return probeMatch[1];
  // Also match config names like "Cloudflare connectivity" — fallback
  return null;
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
}: {
  projectId: string;
  group: UrlGroup;
  expanded: boolean;
  onToggle: () => void;
  onRunAgain: (host: string) => void;
  onRemove: (host: string, configIds: Set<string>) => void;
}) {
  const navigate = useNavigate();
  const { host, runs, lastRun, lastStatus, configIds } = group;
  const isActive = lastRun.status === 'queued' || lastRun.status === 'running';

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
      className={`border border-gray-800 rounded mb-1.5 transition-colors ${borderClass} ${
        expanded ? 'bg-[var(--bg-surface)]' : ''
      }`}
    >
      {/* Collapsed header */}
      <button
        onClick={onToggle}
        className="flex items-center w-full px-4 py-3 gap-3 text-left hover:bg-white/[0.015] transition-colors cursor-pointer"
        aria-expanded={expanded}
        aria-controls={`card-body-${host}`}
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

      {/* Expanded body */}
      {expanded && (
        <div id={`card-body-${host}`} className="px-4 pb-4">
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
  const [searchParams, setSearchParams] = useSearchParams();
  usePageTitle('URL Probe');

  const inputRef = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState(searchParams.get('host') || '');
  // Prefill the preset from ?preset= (scenario launcher); fall back to 'quick'.
  const [preset, setPreset] = useState<DiagPreset>(() => {
    const p = searchParams.get('preset');
    return p === 'standard' || p === 'full' || p === 'route' ? p : 'quick';
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
  const [page, setPage] = useState(1);
  const [expandedCards, setExpandedCards] = useState<Set<string>>(new Set());

  // Sync URL to query string
  useEffect(() => {
    const host = extractHost(url);
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (host) next.set('host', host);
      else next.delete('host');
      return next;
    }, { replace: true });
  }, [url, setSearchParams]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // ── Data loading ────────────────────────────────────────────────────

  const configsQuery = useTestConfigsQuery(projectId, { intervalMs: 15_000 });
  const configs = useMemo(
    () => ((configsQuery.data ?? []) as Array<TestConfigListItem | TestConfig>).filter((config) => {
      const kind = 'endpoint_kind' in config ? config.endpoint_kind : config.endpoint.kind;
      return kind === 'network';
    }),
    [configsQuery.data],
  );
  const configIds = useMemo(() => configs.map((config) => config.id), [configs]);
  const configDetailQueries = useTestConfigDetailsQueries(configIds);
  const configDetails = useMemo(() => {
    const details = new Map<string, TestConfig>();
    for (const query of configDetailQueries) {
      if (query.data) details.set(query.data.id, query.data);
    }
    return details;
  }, [configDetailQueries]);

  const runsQuery = useTestRunsQuery(projectId, DIAGNOSTIC_RUN_PARAMS, {
    intervalMs: 15_000,
    activeIntervalMs: 5_000,
  });
  const allRuns = useMemo(() => runsQuery.data ?? [], [runsQuery.data]);
  const loading = configsQuery.isPending || runsQuery.isPending;

  // Load testers once so the runner-picker can show the list of runners the
  // user can pin their probe to. Auto-pick stays the default — this lets them
  // override it when e.g. debugging a specific region or version.
  useEffect(() => {
    if (!projectId) return;
    let cancelled = false;
    testersApi.listTesters(projectId).then((rows) => {
      if (!cancelled) setTesters(rows);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [projectId]);

  // ── Build URL groups ──────────────────────────────────────────────

  // Clock from state rather than read during render (react-hooks/purity); it is
  // a memo dependency so staleness verdicts advance with it.
  const now = useNow();

  const urlGroups = useMemo(() => {
    const hostMap = new Map<string, { runs: TestRun[]; configIds: Set<string> }>();

    for (const run of allRuns) {
      let host: string | null = null;

      // Try to get host from config details
      const detail = configDetails.get(run.test_config_id);
      if (detail) {
        host = getHostFromConfig(detail);
      }

      // Fallback: parse from config_name
      if (!host && run.config_name) {
        host = getHostFromConfigName(run.config_name);
      }

      // Fallback: try config list item name
      if (!host) {
        const cfg = configs.find(c => c.id === run.test_config_id);
        if (cfg) {
          host = getHostFromConfigName(cfg.name);
        }
      }

      if (!host) continue;

      if (!hostMap.has(host)) {
        hostMap.set(host, { runs: [], configIds: new Set() });
      }
      const entry = hostMap.get(host)!;
      entry.runs.push(run);
      entry.configIds.add(run.test_config_id);
    }

    const groups: UrlGroup[] = [];
    for (const [host, { runs, configIds }] of hostMap) {
      // Sort runs by created_at desc
      runs.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime());
      const lastRun = runs[0];

      // Determine status. A run is "healthy" only when it has completed and
      // recorded at least one successful attempt — never before a dispatcher
      // has claimed and finished it. Staleness ("no check in 24h") is
      // evaluated before healthy so an old green check surfaces as stale,
      // matching the summary strip's label.
      const timeSinceLastRun = now - new Date(lastRun.created_at).getTime();
      let lastStatus: UrlGroup['lastStatus'];
      // Verdict rule shared with the Runs pages (runDisplayStatus, audit F9):
      // completed-with-some-failures reads "partial", not "failed" — the same
      // run must never be green on /runs and red here.
      if (
        lastRun.status === 'failed' ||
        lastRun.status === 'cancelled' ||
        (lastRun.failure_count > 0 && lastRun.success_count === 0)
      ) {
        lastStatus = 'failed';
      } else if (lastRun.status === 'completed' && lastRun.failure_count > 0) {
        lastStatus = 'partial';
      } else if (lastRun.status === 'queued' || lastRun.status === 'provisioning' || lastRun.status === 'running') {
        lastStatus = 'pending';
      } else if (timeSinceLastRun > STALE_THRESHOLD_MS) {
        lastStatus = 'stale';
      } else if (lastRun.status === 'completed' && lastRun.success_count > 0) {
        lastStatus = 'healthy';
      } else {
        // completed-with-no-attempts or unknown — neither healthy nor failed.
        lastStatus = 'pending';
      }

      groups.push({
        host,
        runs,
        configIds,
        lastRun,
        lastStatus,
        totalDurationMs: getDurationMs(lastRun),
      });
    }

    return groups;
  }, [allRuns, configs, configDetails, now]);

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
          return a.host.localeCompare(b.host);
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

  // ── Recent hosts ──────────────────────────────────────────────────

  const recentHosts = useMemo(() => {
    // Copy before sorting — urlGroups is a memoized array shared with the
    // summary/filter pipeline; sorting it in place mutates that cache.
    return [...urlGroups]
      .sort((a, b) => new Date(b.lastRun.created_at).getTime() - new Date(a.lastRun.created_at).getTime())
      .slice(0, 8)
      .map(g => g.host);
  }, [urlGroups]);

  // ── Handlers ──────────────────────────────────────────────────────

  const handleRun = async (targetHost?: string) => {
    const host = targetHost || extractHost(url);
    if (!host) {
      addToast('error', 'Enter a URL or hostname to test');
      return;
    }

    setSubmitting(true);
    try {
      const presetLabel = preset.charAt(0).toUpperCase() + preset.slice(1);
      const configName = `Diag: ${host} (${presetLabel})`;
      // Probe the URL as entered (root by default) — a bare host would get
      // `/health` appended by the agent (E2E P1-4). `host` stays bare for the
      // display name / watchlist grouping.
      const endpoint: EndpointRef = { kind: 'network', host: toProbeUrl(targetHost || url) };
      const workload: Workload = {
        modes: DIAG_PRESETS[preset],
        runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        payload_sizes: [],
        capture_mode: 'headers-only',
      };
      const config: TestConfigCreate = { name: configName, endpoint, workload };

      // `test_config` has UNIQUE (project_id, name), so re-running a diagnostic
      // against the same host+preset must reuse the existing config rather than
      // try (and fail) to create a duplicate. This also keeps the Watched URLs
      // list grouped by host instead of exploding every click.
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
      addToast('success', `Diagnostic ${run.id.slice(0, 8)} launched for ${host}`);

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
    try {
      await Promise.all(Array.from(configIds).map(id => runsApi.deleteConfig(id)));
      addToast('success', `Removed ${host} from watchlist`);
      queryClient.setQueryData<TestRun[]>(runKeys.list(projectId, DIAGNOSTIC_RUN_PARAMS), (previous = []) =>
        previous.filter((run) => !configIds.has(run.test_config_id)),
      );
      queryClient.setQueryData<TestConfigListItem[]>(runKeys.configs(projectId), (previous = []) =>
        previous.filter((config) => !configIds.has(config.id)),
      );
      for (const configId of configIds) {
        queryClient.removeQueries({ queryKey: runKeys.config(configId) });
      }
    } catch (e) {
      addToast('error', `Failed to remove: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const handleHostClick = (host: string) => {
    setUrl(host);
    inputRef.current?.focus();
    // Scroll to the card for this host if it exists
    const card = document.getElementById(`card-body-${host}`);
    if (card) {
      card.closest('[class*="border-gray-800"]')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      setExpandedCards(prev => new Set(prev).add(host));
    }
  };

  const toggleCard = (host: string) => {
    setExpandedCards(prev => {
      const next = new Set(prev);
      if (next.has(host)) next.delete(host);
      else next.add(host);
      return next;
    });
  };

  // Reset the page when the filter/sort changes. Done DURING RENDER via the
  // previous-value comparison React documents for this, rather than in an
  // effect: the effect version rendered page N of the new filter first, then
  // re-rendered at page 1 — a visible flash of the wrong slice.
  const filterKey = `${filter}\u0000${sort}`;
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
        <div className="text-xs tracking-wider text-faint mb-2.5">Probe a URL</div>
        <div className="flex items-center gap-2.5">
          <div className="flex-1 flex items-center gap-2">
            <label htmlFor="diag-url" className="text-xs text-gray-400 flex-shrink-0">URL:</label>
            <input
              ref={inputRef}
              id="diag-url"
              type="text"
              value={url}
              onChange={e => setUrl(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' && url.trim()) handleRun(); }}
              placeholder="Enter URL to test..."
              className="flex-1 bg-[var(--bg-raised)] border border-gray-800 rounded px-3 py-2 text-sm text-cyan-400 focus:outline-none focus:border-cyan-500/50 placeholder:text-gray-600 transition-colors"
              aria-label="URL or hostname to test"
            />
          </div>
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
            disabled={submitting || !url.trim()}
            className="w-9 h-9 !p-0 text-base flex-shrink-0"
            aria-label="Run diagnostic"
            title="Run diagnostic"
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
            <strong className="text-gray-400 font-medium">{summary.total}</strong> {summary.total === 1 ? 'URL' : 'URLs'}
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
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs tracking-wider text-faint">
          Watched URLs ({filteredGroups.length})
        </span>
        <div className="flex items-center gap-3">
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
            className="bg-transparent border border-gray-800 rounded px-2.5 py-1 text-xs text-gray-400 focus:outline-none appearance-none pr-6 cursor-pointer"
            style={{
              backgroundImage: `url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%23475569' stroke-width='1.5'/%3E%3C/svg%3E")`,
              backgroundRepeat: 'no-repeat',
              backgroundPosition: 'right 8px center',
            }}
            aria-label="Sort URLs by"
          >
            <option value="last-checked">Last checked</option>
            <option value="name">Name</option>
            <option value="slowest">Slowest</option>
            <option value="most-runs">Most runs</option>
          </select>
        </div>
      </div>

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
            {filter !== 'all'
              ? `No ${filter} URLs found. Try changing the filter.`
              : 'No probes yet. Enter a URL above to discover what it supports.'}
          </p>
        </div>
      ) : (
        <div>
          {paginatedGroups.map(group => (
            <UrlCard
              key={group.host}
              group={group}
              projectId={projectId}
              expanded={expandedCards.has(group.host)}
              onToggle={() => toggleCard(group.host)}
              onRunAgain={host => handleRun(host)}
              onRemove={handleRemove}
            />
          ))}
        </div>
      )}

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between mt-4 pt-4 border-t border-gray-800">
          <span className="text-xs text-faint">
            Showing {(safePage - 1) * PAGE_SIZE + 1}-{Math.min(safePage * PAGE_SIZE, filteredGroups.length)} of{' '}
            {filteredGroups.length} URLs
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

  const online = testers.filter(t => t.power_state === 'running' && t.agent_status === 'online');
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
