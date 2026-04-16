import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { EndpointRef, TestConfig, TestConfigCreate, TestConfigListItem, TestRun, Workload } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';

// ── Types ───────────────────────────────────────────────────────────────

type DiagPreset = 'quick' | 'standard' | 'full';
type FilterMode = 'all' | 'healthy' | 'failed';
type SortMode = 'last-checked' | 'name' | 'slowest' | 'most-runs';

interface UrlGroup {
  host: string;
  runs: TestRun[];
  configIds: Set<string>;
  lastRun: TestRun;
  lastStatus: 'healthy' | 'failed' | 'stale';
  totalDurationMs: number | null;
}

// ── Constants ───────────────────────────────────────────────────────────

const DIAG_PRESETS: Record<DiagPreset, string[]> = {
  quick: ['dns', 'tcp', 'tls', 'http2'],
  standard: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp'],
  full: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp', 'curl', 'pageload', 'pageload2', 'pageload3', 'browser1', 'browser2', 'browser3'],
};

const DIAG_PRESET_LABELS: Record<DiagPreset, { time: string; desc: string }> = {
  quick: { time: '~3s', desc: 'dns, tcp, tls, http2' },
  standard: { time: '~15s', desc: '+ http1, http3, tls-resume, native-tls, udp' },
  full: { time: '~60s', desc: '+ pageload, browser' },
};

const PAGE_SIZE = 20;
const STALE_THRESHOLD_MS = 24 * 60 * 60 * 1000; // 24 hours

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

function timeAgo(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
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
      <div className="flex gap-4 text-[11px]">
        {phases.map(p => (
          <span key={p.key} className="flex items-center gap-1.5 text-gray-500">
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
}: {
  group: UrlGroup;
  expanded: boolean;
  onToggle: () => void;
  onRunAgain: (host: string) => void;
  onRemove: (host: string, configIds: Set<string>) => void;
}) {
  const { host, runs, lastRun, lastStatus, configIds } = group;
  const isActive = lastRun.status === 'queued' || lastRun.status === 'running';

  // Determine card border class
  const borderClass =
    lastStatus === 'failed'
      ? 'border-l-2 border-l-red-500'
      : lastStatus === 'stale'
        ? 'border-l-2 border-l-amber-500'
        : '';

  // URL text color
  const urlColor =
    lastStatus === 'failed'
      ? 'text-red-400'
      : lastStatus === 'stale'
        ? 'text-amber-400'
        : 'text-cyan-400';

  // Status dot color
  const dotColor =
    lastStatus === 'failed'
      ? 'bg-red-500'
      : lastStatus === 'stale'
        ? 'bg-amber-500'
        : 'bg-emerald-500';

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
        expanded ? 'bg-[#0d1218]' : ''
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
          className={`text-gray-600 text-[11px] flex-shrink-0 transition-transform duration-200 ${
            expanded ? 'rotate-90' : ''
          }`}
          aria-hidden="true"
        >
          {'\u25B8'}
        </span>

        <div className="flex items-center gap-3 flex-1 min-w-0">
          <span className={`text-[13px] font-medium font-mono truncate ${urlColor}`}>
            {host}
          </span>

          {/* Inline phase timings - show total duration if no phase breakdown */}
          {totalDuration != null && (
            <div className="hidden xl:flex items-center gap-3 text-xs tabular-nums whitespace-nowrap">
              <span className="text-gray-200 font-medium">
                <span className="text-gray-500 mr-1 font-normal">total</span>
                {fmtMs(totalDuration)}
              </span>
            </div>
          )}
        </div>

        <div className="flex items-center gap-3 flex-shrink-0">
          {sparklineValues.length >= 2 && <Sparkline values={sparklineValues} />}
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-white/5 text-gray-500 font-medium tabular-nums">
            {runs.length}
          </span>
          <span className="text-[11px] text-gray-600 whitespace-nowrap">
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
                <div className="text-[11px] text-gray-600 uppercase tracking-wider mb-1 mt-3 first:mt-0">
                  {group.label}
                </div>
                <table className="w-full text-xs tabular-nums">
                  <thead>
                    <tr className="text-[10px] text-gray-600 uppercase tracking-wider">
                      <th className="text-left py-1 px-2 font-medium border-b border-gray-800/50">Time</th>
                      <th className="text-left py-1 px-2 font-medium border-b border-gray-800/50" />
                      <th className="text-left py-1 px-2 font-medium border-b border-gray-800/50">Status</th>
                      <th className="text-right py-1 px-2 font-medium border-b border-gray-800/50">Results</th>
                      <th className="text-right py-1 px-2 font-medium border-b border-gray-800/50">Duration</th>
                    </tr>
                  </thead>
                  <tbody>
                    {group.runs.map(run => {
                      const dur = getDurationMs(run);
                      const isFail = run.status === 'failed' || run.failure_count > 0;
                      return (
                        <tr
                          key={run.id}
                          className={`border-b border-white/[0.02] last:border-b-0 ${
                            isFail ? 'text-red-400/80' : ''
                          }`}
                        >
                          <td className="py-1.5 px-2 text-gray-400 whitespace-nowrap">
                            {fmtTime(run.created_at)}
                          </td>
                          <td className="py-1.5 px-2">
                            {run.status === 'completed' && run.failure_count === 0 ? (
                              <span className="text-emerald-400">{'\u2713'}</span>
                            ) : run.status === 'failed' || run.failure_count > 0 ? (
                              <span className="text-red-400">{'\u2717'}</span>
                            ) : run.status === 'running' || run.status === 'queued' ? (
                              <span className="text-blue-400 motion-safe:animate-pulse">{'\u25CF'}</span>
                            ) : (
                              <span className="text-gray-600">-</span>
                            )}
                          </td>
                          <td className="py-1.5 px-2">
                            <StatusBadge status={run.status} />
                          </td>
                          <td className="py-1.5 px-2 text-right font-mono">
                            {run.status === 'completed' && (
                              <span className="flex items-center justify-end gap-2">
                                <span className="text-emerald-400">{run.success_count} ok</span>
                                {run.failure_count > 0 && (
                                  <span className="text-red-400">{run.failure_count} fail</span>
                                )}
                              </span>
                            )}
                          </td>
                          <td className="py-1.5 px-2 text-right text-gray-200 font-medium">
                            {fmtMs(dur)}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
                {/* Show error messages for failed runs */}
                {group.runs
                  .filter(r => r.error_message)
                  .map(r => (
                    <div key={`err-${r.id}`} className="text-[11px] text-red-400/70 pl-2 mt-1">
                      {r.error_message}
                    </div>
                  ))}
              </div>
            ))}
          </div>

          {/* Actions */}
          <div className="flex items-center gap-4 mt-4 pt-3 border-t border-gray-800">
            <button
              onClick={() => onRunAgain(host)}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 text-[11px] border border-gray-800 rounded text-gray-400 hover:text-gray-200 hover:border-gray-600 transition-colors"
            >
              {'\u25B6'} Run again
            </button>
            <button
              onClick={() => onRemove(host, configIds)}
              className="ml-auto text-[11px] text-gray-600 hover:text-red-400 transition-colors"
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
  const addToast = useToast();
  const [searchParams, setSearchParams] = useSearchParams();
  usePageTitle('Diagnostics');

  const inputRef = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState(searchParams.get('host') || '');
  const [preset, setPreset] = useState<DiagPreset>('quick');
  const [submitting, setSubmitting] = useState(false);

  // Data
  const [configs, setConfigs] = useState<TestConfigListItem[]>([]);
  const [configDetails, setConfigDetails] = useState<Map<string, TestConfig>>(new Map());
  const [allRuns, setAllRuns] = useState<TestRun[]>([]);
  const [loading, setLoading] = useState(true);
  const [pendingRunIds, setPendingRunIds] = useState<Set<string>>(new Set());

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

  const loadData = useCallback(() => {
    if (!projectId) return;

    // Fetch configs + runs in parallel
    Promise.all([
      api.listTestConfigs(projectId),
      api.listTestRuns(projectId, { endpoint_kind: 'network', limit: 200 }),
    ]).then(([cfgs, runs]) => {
      // Filter to only 'network' endpoint configs — handle both flat and nested shapes
      const networkConfigs = cfgs.filter((c) => c.endpoint_kind === 'network');
      setConfigs(networkConfigs);
      setAllRuns(runs);
      setLoading(false);

      // Fetch full config details (to get host) for configs we don't have yet
      const missing = networkConfigs.filter(c => !configDetails.has(c.id));
      if (missing.length > 0) {
        Promise.all(missing.map(c => api.getTestConfig(c.id).catch(() => null)))
          .then(details => {
            setConfigDetails(prev => {
              const next = new Map(prev);
              for (const d of details) {
                if (d) next.set(d.id, d);
              }
              return next;
            });
          });
      }

      // Clear completed pending runs
      setPendingRunIds(prev => {
        const stillPending = new Set<string>();
        for (const id of prev) {
          const run = runs.find(r => r.id === id);
          if (run && (run.status === 'queued' || run.status === 'running')) {
            stillPending.add(id);
          }
        }
        return stillPending.size !== prev.size ? stillPending : prev;
      });
    }).catch(() => {
      setLoading(false);
    });
  }, [projectId, configDetails]);

  const hasPending = pendingRunIds.size > 0;
  usePolling(loadData, hasPending ? 5000 : 15000);

  // ── Build URL groups ──────────────────────────────────────────────

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

      // Determine status
      const timeSinceLastRun = Date.now() - new Date(lastRun.created_at).getTime();
      let lastStatus: 'healthy' | 'failed' | 'stale';
      if (lastRun.status === 'failed' || lastRun.failure_count > 0) {
        lastStatus = 'failed';
      } else if (timeSinceLastRun > STALE_THRESHOLD_MS) {
        lastStatus = 'stale';
      } else {
        lastStatus = 'healthy';
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
  }, [allRuns, configs, configDetails]);

  // ── Summary counts ────────────────────────────────────────────────

  const summary = useMemo(() => {
    const total = urlGroups.length;
    const healthy = urlGroups.filter(g => g.lastStatus === 'healthy').length;
    const failed = urlGroups.filter(g => g.lastStatus === 'failed').length;
    const stale = urlGroups.filter(g => g.lastStatus === 'stale').length;
    return { total, healthy, failed, stale };
  }, [urlGroups]);

  // ── Filter + Sort + Paginate ──────────────────────────────────────

  const filteredGroups = useMemo(() => {
    let result = urlGroups;

    // Filter
    if (filter === 'healthy') result = result.filter(g => g.lastStatus === 'healthy');
    if (filter === 'failed') result = result.filter(g => g.lastStatus === 'failed');

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
    return urlGroups
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
      const endpoint: EndpointRef = { kind: 'network', host };
      const workload: Workload = {
        modes: DIAG_PRESETS[preset],
        runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        payload_sizes: [],
        capture_mode: 'headers-only',
      };
      const config: TestConfigCreate = { name: configName, endpoint, workload };

      const created = await api.createTestConfig(projectId, config);
      const run = await api.launchTestConfig(created.id);
      addToast('success', `Diagnostic ${run.id.slice(0, 8)} launched for ${host}`);

      setPendingRunIds(prev => new Set(prev).add(run.id));
      setAllRuns(prev => [run, ...prev]);

      // Update config details with the newly created config
      setConfigDetails(prev => {
        const next = new Map(prev);
        next.set(created.id, created);
        return next;
      });
    } catch (e) {
      addToast('error', `Diagnostic failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  const handleRemove = async (host: string, configIds: Set<string>) => {
    try {
      await Promise.all(Array.from(configIds).map(id => api.deleteTestConfig(id)));
      addToast('success', `Removed ${host} from watchlist`);
      // Reload data
      setAllRuns(prev => prev.filter(r => !configIds.has(r.test_config_id)));
      setConfigs(prev => prev.filter(c => !configIds.has(c.id)));
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

  // Reset page when filter changes
  useEffect(() => { setPage(1); }, [filter, sort]);

  // ── Render ────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-8 max-w-[1200px]">
      {/* Page header */}
      <div className="flex items-start justify-between mb-6">
        <div>
          <h1 className="text-[22px] font-semibold text-gray-200 tracking-tight">Diagnostics</h1>
          <p className="text-xs text-gray-500 mt-1">Quick connectivity tests for any URL</p>
        </div>
      </div>

      {/* Run a Diagnostic input bar */}
      <div className="border border-gray-800 rounded p-4 mb-7">
        <div className="text-[11px] tracking-wider text-gray-600 mb-2.5">Run a Diagnostic</div>
        <div className="flex items-center gap-2.5">
          <div className="flex-1 flex items-center gap-2">
            <label htmlFor="diag-url" className="text-xs text-gray-500 flex-shrink-0">URL:</label>
            <input
              ref={inputRef}
              id="diag-url"
              type="text"
              value={url}
              onChange={e => setUrl(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' && url.trim()) handleRun(); }}
              placeholder="Enter URL to test..."
              className="flex-1 bg-[var(--bg-input,#0e1319)] border border-gray-800 rounded px-3 py-2 text-[13px] font-mono text-cyan-400 focus:outline-none focus:border-cyan-500/50 placeholder:text-gray-600 transition-colors"
              aria-label="URL or hostname to test"
            />
          </div>
          <label htmlFor="diag-preset" className="text-xs text-gray-500">Preset</label>
          <select
            id="diag-preset"
            value={preset}
            onChange={e => setPreset(e.target.value as DiagPreset)}
            className="bg-[var(--bg-input,#0e1319)] border border-gray-800 rounded px-3 py-2 text-xs font-mono text-gray-400 focus:outline-none appearance-none pr-7 cursor-pointer"
            style={{
              backgroundImage: `url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%23475569' stroke-width='1.5'/%3E%3C/svg%3E")`,
              backgroundRepeat: 'no-repeat',
              backgroundPosition: 'right 10px center',
            }}
          >
            {(['quick', 'standard', 'full'] as DiagPreset[]).map(p => (
              <option key={p} value={p}>
                {p.charAt(0).toUpperCase() + p.slice(1)} ({DIAG_PRESET_LABELS[p].time})
              </option>
            ))}
          </select>
          <button
            onClick={() => handleRun()}
            disabled={submitting || !url.trim()}
            className="flex items-center justify-center w-9 h-9 bg-cyan-400 text-gray-900 rounded font-bold text-base hover:opacity-85 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity flex-shrink-0"
            aria-label="Run diagnostic"
            title="Run diagnostic"
          >
            {submitting ? (
              <span className="w-4 h-4 border-2 border-gray-900/30 border-t-gray-900 rounded-full motion-safe:animate-spin" />
            ) : (
              '\u25B6'
            )}
          </button>
        </div>
      </div>

      {/* Recent host chips */}
      {recentHosts.length > 0 && (
        <div className="mb-6 flex flex-wrap gap-1.5 items-center">
          <span className="text-xs text-gray-600 mr-1">Recent:</span>
          {recentHosts.map(host => (
            <button
              key={host}
              onClick={() => handleHostClick(host)}
              className={`text-xs px-2 py-1 rounded border transition-colors font-mono ${
                extractHost(url) === host
                  ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-400'
                  : 'border-gray-800 text-gray-500 hover:border-gray-600 hover:text-gray-400'
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
          <span className="text-gray-500">
            <strong className="text-gray-400 font-medium">{summary.total}</strong> {summary.total === 1 ? 'URL' : 'URLs'}
          </span>
          <span className="text-gray-700">&middot;</span>
          <span className="text-gray-500">
            <strong className="text-emerald-400 font-medium">{summary.healthy}</strong> healthy
          </span>
          <span className="text-gray-700">&middot;</span>
          <span className="text-gray-500">
            <strong className="text-red-400 font-medium">{summary.failed}</strong> failed
          </span>
          <span className="text-gray-700">&middot;</span>
          <span className="text-gray-500">
            <strong className="text-amber-400 font-medium">{summary.stale}</strong> stale (no check in 24h)
          </span>
        </div>
      )}

      {/* Toolbar */}
      <div className="flex items-center justify-between mb-3">
        <span className="text-[11px] tracking-wider text-gray-600">
          Watched URLs ({filteredGroups.length})
        </span>
        <div className="flex items-center gap-3">
          {/* Filter toggle */}
          <div className="flex border border-gray-800 rounded overflow-hidden">
            {(['all', 'healthy', 'failed'] as FilterMode[]).map(f => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={`px-3 py-1 text-[11px] font-mono border-r border-gray-800 last:border-r-0 transition-colors ${
                  filter === f
                    ? 'bg-white/5 text-gray-200'
                    : 'text-gray-600 hover:text-gray-400'
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
            className="bg-transparent border border-gray-800 rounded px-2.5 py-1 text-[11px] font-mono text-gray-500 focus:outline-none appearance-none pr-6 cursor-pointer"
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
          <p className="text-gray-500 text-sm">
            {filter !== 'all'
              ? `No ${filter} URLs found. Try changing the filter.`
              : 'No diagnostics yet. Enter a URL above to run your first test.'}
          </p>
        </div>
      ) : (
        <div>
          {paginatedGroups.map(group => (
            <UrlCard
              key={group.host}
              group={group}
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
          <span className="text-xs text-gray-600">
            Showing {(safePage - 1) * PAGE_SIZE + 1}-{Math.min(safePage * PAGE_SIZE, filteredGroups.length)} of{' '}
            {filteredGroups.length} URLs
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setPage(p => Math.max(1, p - 1))}
              disabled={safePage <= 1}
              className="px-3 py-1 text-xs border border-gray-800 rounded text-gray-500 hover:text-gray-300 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              Previous
            </button>
            <span className="text-xs text-gray-500 tabular-nums px-2">
              {safePage} / {totalPages}
            </span>
            <button
              onClick={() => setPage(p => Math.min(totalPages, p + 1))}
              disabled={safePage >= totalPages}
              className="px-3 py-1 text-xs border border-gray-800 rounded text-gray-500 hover:text-gray-300 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
