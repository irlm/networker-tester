import React, { useState, useCallback, useEffect, useRef, useMemo } from 'react';
import { useParams, Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkConfigSummary, BenchmarkLanguageProgress, LogEntry } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { useLiveStore } from '../stores/liveStore';
import type { BenchmarkLive } from '../stores/liveStore';
import { useToast } from '../hooks/useToast';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { PhaseBar } from '../components/PhaseBar';

// TODO(persistent-testers): wire usePhaseSubscription(projectId, testerId, 'benchmark', configId)
// once the benchmark config response reliably includes the tester id that owns
// the run. Today the progress page derives phase from HTTP polling + the live
// store, which predates the tester_queue WebSocket. Until then, PhaseBar is
// driven from `effectiveStatus` so the UI stays consistent with the rest of
// the persistent-testers work.
const DEFAULT_STAGES = ['queued', 'starting', 'deploy', 'running', 'collect', 'done'] as const;
type PhaseStage = typeof DEFAULT_STAGES[number];
type PhaseOutcome = 'success' | 'partial_success' | 'failure' | 'cancelled';

function mapEffectiveStatusToPhase(status: string): { phase: PhaseStage; outcome: PhaseOutcome | null } {
  switch (status) {
    case 'pending':
    case 'queued':
      return { phase: 'queued', outcome: null };
    case 'provisioning':
      return { phase: 'starting', outcome: null };
    case 'deploying':
      return { phase: 'deploy', outcome: null };
    case 'running':
      return { phase: 'running', outcome: null };
    case 'collecting':
      return { phase: 'collect', outcome: null };
    case 'completed':
      return { phase: 'done', outcome: 'success' };
    case 'completed_with_errors':
      return { phase: 'done', outcome: 'partial_success' };
    case 'failed':
      return { phase: 'done', outcome: 'failure' };
    case 'cancelled':
      return { phase: 'done', outcome: 'cancelled' };
    default:
      return { phase: 'queued', outcome: null };
  }
}

// ── Helpers ─────────────────────────────────────────────────────────────

function formatElapsed(startedAt: string | null): string {
  if (!startedAt) return '--:--';
  const start = new Date(startedAt).getTime();
  const elapsed = Math.max(0, Math.floor((Date.now() - start) / 1000));
  const h = Math.floor(elapsed / 3600);
  const m = Math.floor((elapsed % 3600) / 60);
  const s = elapsed % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m ${String(s).padStart(2, '0')}s`;
  return `${m}m ${String(s).padStart(2, '0')}s`;
}

const CLOUD_ICONS: Record<string, string> = {
  Azure: 'Az',
  AWS: 'AWS',
  GCP: 'GCP',
};

const STATUS_COLORS: Record<string, string> = {
  provisioning: 'text-yellow-400',
  deploying: 'text-yellow-400',
  running: 'text-cyan-400',
  completed: 'text-green-400',
  failed: 'text-red-400',
  cancelled: 'text-gray-500',
  pending: 'text-gray-500',
};

function levelToString(level: number): string {
  switch (level) {
    case 1: return 'ERROR';
    case 2: return 'WARN';
    case 3: return 'INFO';
    case 4: return 'DEBUG';
    case 5: return 'TRACE';
    default: return `L${level}`;
  }
}

function logLevelColor(level: number): string {
  switch (level) {
    case 1: return 'text-red-400';
    case 2: return 'text-yellow-400';
    default: return 'text-gray-400';
  }
}

interface TestbedDetail {
  testbed_id: string;
  cloud: string;
  region: string;
  os?: string;
  topology: string;
  languages: string[];
  status?: string;
}

const EMPTY_LIVE: BenchmarkLive = {
  logs: [],
  testbeds: {},
  results: [],
  configStatus: null,
  completedAt: null,
  errorMessage: null,
};

// ── Component ───────────────────────────────────────────────────────────

export function BenchmarkProgressPage() {
  const { projectId } = useProject();
  const { configId } = useParams<{ configId: string }>();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('Benchmark Progress');

  const [config, setConfig] = useState<BenchmarkConfigSummary | null>(null);
  const [testbeds, setTestbeds] = useState<TestbedDetail[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [elapsedStr, setElapsedStr] = useState('--:--');

  // Log viewer state
  const logContainerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  // Historical logs from /api/logs (visible after benchmark finishes)
  const [historicalLogs, setHistoricalLogs] = useState<LogEntry[]>([]);

  // Already-completed results fetched from API (survives page reload)
  const [savedResults, setSavedResults] = useState<Array<{ language: string; run_id: string; started_at?: string | null; finished_at?: string | null }>>([]);

  // Per-mode progress from progress endpoint
  const [langProgress, setLangProgress] = useState<BenchmarkLanguageProgress[]>([]);

  // Live data from WebSocket
  const live = useLiveStore(
    useCallback((s) => (configId ? s.benchmarks[configId] : undefined) ?? EMPTY_LIVE, [configId])
  );

  // Poll config for status updates (in case WS misses something)
  const loadConfig = useCallback(async () => {
    if (!configId || !projectId) return;
    try {
      const data = await api.getBenchmarkConfig(projectId, configId);
      setConfig(data);
      setError(null);
    } catch {
      setError('Failed to load benchmark config');
    } finally {
      setLoading(false);
    }
  }, [configId, projectId]);

  usePolling(loadConfig, 5000, !!configId);

  // Load config + testbed details on mount and poll while active
  const fetchConfig = useCallback(() => {
    if (!configId || !projectId) return;
    api.getBenchmarkConfig(projectId, configId)
      .then((data) => {
        setConfig(data);
        const configData = data as unknown as Record<string, unknown>;
        const rawTestbeds = (configData.testbeds ?? configData.testbed_configs ?? []) as TestbedDetail[];
        if (Array.isArray(rawTestbeds)) {
          setTestbeds(rawTestbeds);
        }
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [configId, projectId]);

  useEffect(() => {
    fetchConfig();
  }, [fetchConfig]);

  // Poll config + testbed status while benchmark is active
  const configStatus = config?.status || 'pending';
  const stillRunning = ['running', 'provisioning', 'deploying', 'pending', 'queued'].includes(configStatus);
  useEffect(() => {
    if (!stillRunning) return;
    const interval = setInterval(fetchConfig, 5000);
    return () => clearInterval(interval);
  }, [stillRunning, fetchConfig]);

  // Fetch saved results from API (handles page reload — WS results are ephemeral)
  useEffect(() => {
    if (!configId || !projectId) return;
    const fetchSaved = () => {
      api.getBenchmarkConfigResults(projectId, configId)
        .then((data) => {
          if (data.results && data.results.length > 0) {
            setSavedResults(data.results.map((r: { language: string; run_id: string; started_at?: string | null; finished_at?: string | null }) => ({
              language: r.language,
              run_id: r.run_id,
              started_at: r.started_at,
              finished_at: r.finished_at,
            })));
          }
        })
        .catch(() => {});
    };
    fetchSaved();
    const interval = setInterval(fetchSaved, 10000);
    return () => clearInterval(interval);
  }, [configId, projectId]);

  // Elapsed time timer
  useEffect(() => {
    const isActive = config?.status === 'running' || config?.status === 'provisioning' || config?.status === 'deploying';
    if (!isActive || !config?.started_at) return;
    const interval = setInterval(() => {
      setElapsedStr(formatElapsed(config.started_at));
    }, 1000);
    setElapsedStr(formatElapsed(config.started_at));
    return () => clearInterval(interval);
  }, [config?.status, config?.started_at]);

  // Auto-scroll log container
  useEffect(() => {
    if (autoScroll && logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [live.logs, autoScroll]);

  const handleLogScroll = () => {
    if (!logContainerRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = logContainerRef.current;
    setAutoScroll(scrollHeight - scrollTop - clientHeight < 50);
  };

  const handleCancel = async () => {
    if (!configId || !projectId) return;
    try {
      await api.cancelBenchmarkConfig(projectId, configId);
      addToast('info', 'Benchmark cancellation requested');
      loadConfig();
    } catch {
      addToast('error', 'Failed to cancel benchmark');
    }
  };

  // Derive effective status from WS or polled config, refined by testbed phase.
  // When config says "running" but testbeds are still provisioning/deploying,
  // show the actual phase so the user knows what's happening.
  const rawStatus = live.configStatus || config?.status || 'pending';
  const effectiveStatus = (() => {
    if (rawStatus !== 'running') return rawStatus;
    // Refine "running" based on testbed statuses
    const tbStatuses = (testbeds || []).map((tb) => tb.status);
    if (tbStatuses.length === 0) return 'queued';
    if (tbStatuses.every(s => s === 'pending')) return 'queued';
    if (tbStatuses.some(s => s === 'provisioning')) return 'provisioning';
    if (tbStatuses.some(s => s === 'deploying')) return 'deploying';
    return 'running';
  })();
  const isActive = ['running', 'provisioning', 'deploying', 'pending', 'queued'].includes(effectiveStatus);
  const isDone = ['completed', 'failed', 'cancelled', 'completed_with_errors'].includes(effectiveStatus);

  // Fetch historical orchestrator logs for this config (fallback when WS logs are empty)
  useEffect(() => {
    if (!configId) return;
    const fetchHistoricalLogs = async () => {
      try {
        // Try with config_id first (new orchestrator versions set it)
        const data = await api.getSystemLogs({ config_id: configId, service: 'orchestrator', limit: 500 });
        if (data.entries.length > 0) {
          setHistoricalLogs(data.entries);
        } else {
          // Fallback: get recent orchestrator logs without config_id filter
          const fallback = await api.getSystemLogs({ service: 'orchestrator', limit: 100 });
          setHistoricalLogs(fallback.entries);
        }
      } catch { /* ignore */ }
    };
    fetchHistoricalLogs();
    // Poll while active, stop when done
    const interval = isActive ? setInterval(fetchHistoricalLogs, 10000) : null;
    return () => { if (interval) clearInterval(interval); };
  }, [configId, isActive]);

  // Poll per-mode progress while benchmark is active
  useEffect(() => {
    if (!configId || !projectId || !isActive) return;
    const fetchProgress = () => {
      api.getBenchmarkProgress(projectId, configId)
        .then(data => setLangProgress(data.progress ?? []))
        .catch(() => {});
    };
    fetchProgress();
    const interval = setInterval(fetchProgress, 5000);
    return () => clearInterval(interval);
  }, [configId, projectId, isActive]);

  // Extract methodology for progress estimates
  const methodology = useMemo(() => {
    const cfg = (config as unknown as Record<string, unknown>)?.config_json as Record<string, unknown> | undefined;
    const meth = cfg?.methodology as Record<string, unknown> | undefined;
    if (!meth) return null;
    const warmup = (meth.warmup_runs ?? 10) as number;
    const measured = (meth.measured_runs ?? meth.min_measured ?? 50) as number;
    const modes = (meth.modes as string[]) ?? ['http1'];
    const hasPayloadModes = modes.some(m => m.startsWith('download') || m.startsWith('upload') || m.startsWith('udp'));
    const payloadMultiplier = hasPayloadModes ? 3 : 1; // 4k, 64k, 1m
    return { warmup, measured, modes, modeCount: modes.length, payloadMultiplier };
  }, [config]);

  // Compute progress stats
  const progressStats = useMemo(() => {
    const totalLangs = testbeds[0]?.languages?.length ?? 0;
    const totalTestbeds = testbeds.length || 1;
    const totalRuns = totalLangs * totalTestbeds;
    const completedRuns = savedResults.length;
    const uniqueCompleted = new Set(savedResults.map(r => r.language)).size;

    // Last completed result
    const sorted = [...savedResults].filter(r => r.finished_at).sort((a, b) =>
      new Date(b.finished_at!).getTime() - new Date(a.finished_at!).getTime()
    );
    const lastResult = sorted[0] ?? null;
    let lastResultAge: string | null = null;
    if (lastResult?.finished_at) {
      const ago = Math.floor((Date.now() - new Date(lastResult.finished_at).getTime()) / 60000);
      if (ago < 1) lastResultAge = 'just now';
      else if (ago < 60) lastResultAge = `${ago}m ago`;
      else lastResultAge = `${Math.floor(ago / 60)}h ${ago % 60}m ago`;
    }

    return { totalLangs, totalTestbeds, totalRuns, completedRuns, uniqueCompleted, lastResult, lastResultAge };
  }, [testbeds, savedResults]);

  // Merge live WS results with saved DB results (DB survives page reload)
  const resultRows = useMemo(() => {
    const seen = new Set<string>();
    const rows: Array<{
      language: string;
      run_id?: string;
      mean_latency: number | null;
      p50: number | null;
      p99: number | null;
      success_rate: number | null;
      runtime_ms: number | null;
    }> = [];

    // Live WS results first (may have artifact metrics)
    for (const r of live.results) {
      seen.add(r.language);
      const a = r.artifact || {};
      const summary = (a.summary ?? a) as Record<string, unknown>;
      rows.push({
        language: r.language,
        run_id: r.run_id,
        mean_latency: typeof summary.mean_latency_ms === 'number' ? summary.mean_latency_ms : null,
        p50: typeof summary.p50_ms === 'number' ? summary.p50_ms : null,
        p99: typeof summary.p99_ms === 'number' ? summary.p99_ms : null,
        success_rate: typeof summary.success_rate === 'number' ? summary.success_rate : null,
        runtime_ms: typeof summary.runtime_ms === 'number' ? summary.runtime_ms : null,
      });
    }

    // Add saved DB results not already in live (completed before page load)
    for (const sr of savedResults) {
      if (!seen.has(sr.language)) {
        seen.add(sr.language);
        rows.push({
          language: sr.language,
          run_id: sr.run_id,
          mean_latency: null, p50: null, p99: null,
          success_rate: null, runtime_ms: null,
        });
      }
    }

    return rows.sort((a, b) => (a.mean_latency ?? Infinity) - (b.mean_latency ?? Infinity));
  }, [live.results, savedResults]);

  // ── Render ──────────────────────────────────────────────────────────────

  if (loading && !config) {
    return (
      <div className="p-4 md:p-6">
        <p className="text-gray-500">Loading benchmark...</p>
      </div>
    );
  }

  if (error && !config) {
    return (
      <div className="p-4 md:p-6">
        <div className="bg-red-500/10 border border-red-500/30 rounded p-3">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6 max-w-6xl">
      {/* Breadcrumb */}
      <div className="flex items-center gap-2 text-sm text-gray-500 mb-4">
        <Link to={`/projects/${projectId}/benchmarks`} className="hover:text-gray-300">
          Benchmarks
        </Link>
        <span>/</span>
        <span className="text-gray-300">{config?.name || configId?.slice(0, 8)}</span>
      </div>

      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-3">
          <h2 className="text-xl font-bold text-gray-100">{config?.name || 'Benchmark'}</h2>
          <StatusBadge status={effectiveStatus} />
          <span className="text-sm font-mono text-gray-400">{elapsedStr}</span>
        </div>
        <div className="flex gap-2">
          {isActive && (
            <button
              onClick={handleCancel}
              className="bg-red-600/20 border border-red-500/30 hover:bg-red-600/30 text-red-400 px-4 py-1.5 rounded text-sm transition-colors"
            >
              Cancel
            </button>
          )}
          {isDone && (
            <>
              <button
                onClick={() => navigate(`/projects/${projectId}/benchmark-configs/${configId}/results`)}
                className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
              >
                View Results
              </button>
              <button
                onClick={() => navigate(`/projects/${projectId}/benchmarks`)}
                className="border border-gray-700 hover:border-cyan-500 text-gray-300 hover:text-cyan-400 px-4 py-1.5 rounded text-sm transition-colors"
              >
                Back to Benchmarks
              </button>
            </>
          )}
        </div>
      </div>

      {/* Pipeline Indicator */}
      {(isActive || isDone) && (
        <div className="mb-6 border border-gray-800 rounded p-4 bg-[var(--bg-card)]">
          <PhaseBar
            phase={mapEffectiveStatusToPhase(effectiveStatus).phase}
            outcome={mapEffectiveStatusToPhase(effectiveStatus).outcome}
            appliedStages={[...DEFAULT_STAGES]}
          />
        </div>
      )}

      {/* Config Summary + Progress */}
      {config && isActive && (
        <div className="mb-6 border border-gray-800/50 rounded p-3 bg-gray-900/30 space-y-2">
          <div className="flex items-center gap-4 text-xs text-gray-500 flex-wrap">
            <span>
              <span className="text-gray-400">{progressStats.totalTestbeds}</span> testbed{progressStats.totalTestbeds !== 1 ? 's' : ''}
            </span>
            <span>{'\u00B7'}</span>
            <span>
              <span className="text-gray-400">{progressStats.totalLangs}</span> languages
            </span>
            <span>{'\u00B7'}</span>
            {methodology && (
              <span className="text-gray-400">
                {methodology.warmup} warmup + {methodology.measured} measured &times; {methodology.modeCount} mode{methodology.modeCount !== 1 ? 's' : ''}
                {methodology.payloadMultiplier > 1 ? ` \u00D7 ${methodology.payloadMultiplier} sizes` : ''}
              </span>
            )}
            <span>{'\u00B7'}</span>
            <span>
              <span className="text-cyan-400">{progressStats.completedRuns}</span>
              <span className="text-gray-500">/{progressStats.totalRuns} runs</span>
            </span>
            {progressStats.lastResult && (
              <>
                <span>{'\u00B7'}</span>
                <span className="text-gray-500">
                  last: <span className="text-gray-400">{progressStats.lastResult.language}</span>
                  {progressStats.lastResultAge && <span className="text-gray-600"> ({progressStats.lastResultAge})</span>}
                </span>
              </>
            )}
          </div>
          {/* Overall progress bar */}
          {progressStats.totalRuns > 0 && (
            <div className="h-1 bg-gray-800 rounded-full overflow-hidden">
              <div
                className="h-full bg-cyan-500/70 rounded-full transition-all duration-1000"
                style={{ width: `${Math.round((progressStats.completedRuns / progressStats.totalRuns) * 100)}%` }}
              />
            </div>
          )}
        </div>
      )}

      {/* Activity indicator — shown when running but no logs streaming */}
      {isActive && effectiveStatus === 'running' && live.logs.length === 0 && savedResults.length === 0 && (
        <div className="mb-6 border border-gray-800/50 rounded p-3 bg-gray-900/20">
          <div className="flex items-center gap-2 text-xs text-gray-500">
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-cyan-500 animate-pulse" />
            <span className="text-gray-400">
              Provisioning VMs and deploying first language server. Results will appear as each language completes.
            </span>
          </div>
        </div>
      )}

      {/* Error message */}
      {live.errorMessage && (
        <div className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4">
          <p className="text-red-400 text-sm">{live.errorMessage}</p>
        </div>
      )}

      {/* Testbed Status Cards */}
      {testbeds.length > 0 && (
        <div className="mb-6">
          <p className="text-xs text-gray-500 tracking-wider font-medium mb-3">testbed status</p>
          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
            {testbeds.map((testbed, idx) => {
              const testbedLive = live.testbeds[testbed.testbed_id];
              const testbedStatus = testbedLive?.status || 'pending';
              const currentLang = testbedLive?.current_language;
              const langIdx = testbedLive?.language_index;
              const langTotal = testbedLive?.language_total ?? testbed.languages?.length ?? 0;
              const progressPct = langTotal > 0 && langIdx != null
                ? Math.round(((langIdx + 1) / langTotal) * 100)
                : 0;
              const testbedDone = testbedStatus === 'completed' || testbedStatus === 'failed';

              return (
                <div
                  key={testbed.testbed_id || idx}
                  className={`rounded-lg p-4 transition-all duration-500 ${
                    testbedStatus === 'running'
                      ? 'border border-cyan-500/40 bg-cyan-950/10 shadow-[0_0_12px_rgba(71,191,255,0.06)]'
                      : testbedStatus === 'completed'
                        ? 'border border-green-500/30 bg-green-950/10'
                        : testbedStatus === 'failed'
                          ? 'border border-red-500/30 bg-red-950/10'
                          : 'border border-gray-800 bg-[var(--bg-surface)]/40'
                  }`}
                >
                  <div className="flex items-center justify-between mb-2">
                    <div className="flex items-center gap-2">
                      <span className="text-xs font-bold text-gray-300 bg-gray-800 rounded px-1.5 py-0.5">
                        {CLOUD_ICONS[testbed.cloud] || testbed.cloud}
                      </span>
                      <span className="text-xs text-gray-400 font-mono">{testbed.region}</span>
                      {testbed.os && (
                        <span className={`text-[10px] px-1.5 py-0.5 rounded border ${
                          testbed.os === 'windows'
                            ? 'border-blue-500/30 text-blue-300'
                            : 'border-green-500/30 text-green-300'
                        }`}>
                          {testbed.os === 'windows' ? 'Windows' : 'Linux'}
                        </span>
                      )}
                    </div>
                    <span className={`text-xs font-medium ${STATUS_COLORS[testbedStatus] || 'text-gray-500'}`}>
                      {testbedStatus}
                      {testbedStatus === 'running' && (
                        <span className="inline-block w-1.5 h-1.5 rounded-full bg-cyan-400 ml-1.5 motion-safe:animate-pulse" />
                      )}
                    </span>
                  </div>

                  <div className="text-[11px] text-gray-500 mb-2">{testbed.topology}</div>

                  {currentLang && !testbedDone && (
                    <div className="flex items-center gap-2 mb-2">
                      <span className="text-xs text-cyan-300 font-mono">{currentLang}</span>
                      {langIdx != null && langTotal > 0 && (
                        <span className="text-[11px] text-gray-600">
                          ({langIdx + 1} of {langTotal})
                        </span>
                      )}
                    </div>
                  )}

                  {/* Progress bar */}
                  <div className="h-1.5 bg-gray-800 rounded-full overflow-hidden">
                    <div
                      className={`h-full rounded-full transition-all duration-500 ${
                        testbedDone
                          ? testbedStatus === 'completed' ? 'bg-green-500' : 'bg-red-500'
                          : 'bg-cyan-500'
                      }`}
                      style={{ width: `${testbedDone ? 100 : progressPct}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Language Progress — show all languages with completed/pending status */}
      {testbeds.length > 0 && (
        <div className="mb-6">
          <p className="text-xs text-gray-500 tracking-wider font-medium mb-3">language progress</p>
          <div className="border border-gray-800 rounded-lg overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-[var(--bg-surface)]/60 text-gray-500 text-xs">
                  <th className="text-left px-4 py-2 font-medium">Language</th>
                  <th className="text-left px-4 py-2 font-medium">Status</th>
                  <th className="text-right px-4 py-2 font-medium">Mean Latency</th>
                  <th className="text-right px-4 py-2 font-medium">p50</th>
                  <th className="text-right px-4 py-2 font-medium">p99</th>
                  <th className="text-right px-4 py-2 font-medium">Success Rate</th>
                  <th className="text-right px-4 py-2 font-medium">Runtime</th>
                </tr>
              </thead>
              <tbody>
                {(() => {
                  // Build per-language status across all testbeds
                  const completedSet = new Set(savedResults.map(r => r.language));
                  const uniqueLangs = [...new Set(testbeds.flatMap(tb => tb.languages ?? []))];
                  // Count how many testbeds completed each language
                  const langCompletedCount = new Map<string, number>();
                  for (const r of savedResults) {
                    langCompletedCount.set(r.language, (langCompletedCount.get(r.language) ?? 0) + 1);
                  }
                  const totalTestbeds = testbeds.length;

                  return uniqueLangs.flatMap((lang, i) => {
                    const result = resultRows.find(r => r.language === lang);
                    const doneCount = langCompletedCount.get(lang) ?? 0;
                    const allDone = doneCount >= totalTestbeds;
                    const partialDone = doneCount > 0 && !allDone;
                    const currentLang = Object.values(live.testbeds).find(t => t.current_language === lang);
                    const isRunning = !!currentLang;

                    // Per-mode progress for this language
                    const langModes = langProgress
                      .filter(lp => lp.language === lang)
                      .flatMap(lp => lp.modes);
                    const langTotalCompleted = langModes.reduce((sum, m) => sum + m.completed, 0);
                    const langTotalExpected = langModes.reduce((sum, m) => sum + m.total, 0);
                    const hasProgress = langTotalCompleted > 0;

                    const rows: React.ReactNode[] = [];

                    rows.push(
                      <tr
                        key={lang + i}
                        className={`border-t border-gray-800/50 ${
                          isRunning ? 'bg-cyan-500/5' : ''
                        }`}
                      >
                        <td className={`px-4 py-2 font-mono text-xs ${completedSet.has(lang) ? 'text-gray-200' : 'text-gray-600'}`}>
                          {lang}
                        </td>
                        <td className="px-4 py-2 text-xs">
                          {allDone ? (
                            <span className="text-green-400">{totalTestbeds > 1 ? `done (${doneCount}/${totalTestbeds})` : 'done'}</span>
                          ) : partialDone ? (
                            <span className="text-yellow-400">{doneCount}/{totalTestbeds} done</span>
                          ) : hasProgress && !allDone ? (
                            <span className="text-cyan-400 flex items-center gap-1.5">
                              running ({langTotalCompleted}/{langTotalExpected})
                              <span className="inline-block w-1.5 h-1.5 rounded-full bg-cyan-400 animate-pulse" />
                            </span>
                          ) : isRunning ? (
                            <span className="text-cyan-400 flex items-center gap-1.5">
                              running
                              <span className="inline-block w-1.5 h-1.5 rounded-full bg-cyan-400 animate-pulse" />
                            </span>
                          ) : (
                            <span className="text-gray-600">pending</span>
                          )}
                        </td>
                        <td className="px-4 py-2 text-right text-gray-300 font-mono text-xs">
                          {result?.mean_latency != null ? `${result.mean_latency.toFixed(2)} ms` : doneCount > 0 ? 'done' : '--'}
                        </td>
                        <td className="px-4 py-2 text-right text-gray-400 font-mono text-xs">
                          {result?.p50 != null ? `${result.p50.toFixed(2)} ms` : '--'}
                        </td>
                        <td className="px-4 py-2 text-right text-gray-400 font-mono text-xs">
                          {result?.p99 != null ? `${result.p99.toFixed(2)} ms` : '--'}
                        </td>
                        <td className="px-4 py-2 text-right text-gray-400 font-mono text-xs">
                          {result?.success_rate != null ? `${(result.success_rate * 100).toFixed(1)}%` : '--'}
                        </td>
                        <td className="px-4 py-2 text-right text-gray-500 font-mono text-xs">
                          {result?.runtime_ms != null ? `${(result.runtime_ms / 1000).toFixed(1)}s` : '--'}
                        </td>
                      </tr>
                    );

                    // Per-mode progress sub-row
                    if (langModes.length > 0) {
                      rows.push(
                        <tr key={`${lang}-modes`}>
                          <td colSpan={7} className="px-4 py-1.5 bg-gray-900/30">
                            <div className="space-y-1">
                              {langModes.map(m => {
                                const pct = m.total > 0 ? (m.completed / m.total) * 100 : 0;
                                return (
                                  <div key={m.mode} className="flex items-center gap-3 text-[11px] font-mono">
                                    <span className="w-20 text-gray-500 text-right">{m.mode}</span>
                                    <div className="flex-1 h-1.5 bg-gray-800 rounded-full overflow-hidden">
                                      <div
                                        className="h-full bg-cyan-500/60 rounded-full transition-all duration-500"
                                        style={{ width: `${pct}%` }}
                                      />
                                    </div>
                                    <span className="w-20 text-gray-400">{m.completed}/{m.total}</span>
                                    {m.p50_ms != null && (
                                      <span className="w-28 text-gray-500">
                                        p50: {m.p50_ms < 1 ? `${(m.p50_ms * 1000).toFixed(0)}\u00B5s` : `${m.p50_ms.toFixed(2)}ms`}
                                      </span>
                                    )}
                                  </div>
                                );
                              })}
                            </div>
                          </td>
                        </tr>
                      );
                    }

                    return rows;
                  });
                })()}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Live Log Viewer */}
      <div className="mb-6">
        <div className="flex items-center justify-between mb-2">
          <div className="flex items-center gap-2">
            <p className="text-xs text-gray-500 tracking-wider font-medium">
              {live.logs.length > 0 ? 'live log' : historicalLogs.length > 0 ? 'orchestrator log' : 'live log'}
            </p>
            {live.logs.length === 0 && historicalLogs.length > 0 && (
              <span className="text-[10px] text-gray-600 font-mono">(historical — queried from /api/logs)</span>
            )}
          </div>
          {!autoScroll && (
            <button
              onClick={() => {
                setAutoScroll(true);
                if (logContainerRef.current) {
                  logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
                }
              }}
              className="text-xs text-cyan-400 hover:text-cyan-300"
            >
              Scroll to bottom
            </button>
          )}
        </div>
        <div
          ref={logContainerRef}
          onScroll={handleLogScroll}
          className="bg-[var(--bg-base)] border border-gray-800 rounded-lg p-4 h-[400px] overflow-y-auto font-mono text-xs leading-5"
        >
          {live.logs.length > 0 ? (
            live.logs.map((line, i) => (
              <div key={i} className="text-gray-300 whitespace-pre-wrap break-all">
                {line}
              </div>
            ))
          ) : historicalLogs.length > 0 ? (
            historicalLogs.map((entry, i) => {
              const levelStr = levelToString(entry.level);
              const ts = (() => {
                try { return new Date(entry.ts).toTimeString().slice(0, 8); } catch { return entry.ts; }
              })();
              return (
                <div key={i} className={logLevelColor(entry.level)}>
                  <span className="text-gray-600">[{ts}]</span>{' '}
                  <span className="font-bold">{levelStr.padEnd(5)}</span>{' '}
                  &mdash; {entry.message}
                </div>
              );
            })
          ) : (
            <div className="text-gray-600 space-y-2">
              {isActive ? (
                <>
                  <p className="flex items-center gap-2">
                    <span className="inline-block w-2 h-2 rounded-full bg-cyan-500 animate-pulse" />
                    {effectiveStatus === 'pending' || effectiveStatus === 'queued'
                      ? 'Benchmark queued — worker will pick it up shortly...'
                      : effectiveStatus === 'provisioning'
                        ? 'Provisioning VM — this may take 1-2 minutes...'
                        : effectiveStatus === 'deploying'
                          ? 'Deploying language servers to VM...'
                          : savedResults.length > 0
                            ? `${progressStats.completedRuns}/${progressStats.totalRuns} runs done. Logs appear between language transitions.${progressStats.lastResult ? ` Last completed: ${progressStats.lastResult.language}${progressStats.lastResultAge ? ` (${progressStats.lastResultAge})` : ''}.` : ''}`
                            : 'Orchestrator running — log output will stream here...'}
                  </p>
                  {effectiveStatus === 'running' && savedResults.length === 0 && (
                    <p className="text-gray-700 text-[10px]">
                      Logs stream between language deployments. During a benchmark run, the runner executes silently. Check the language progress table above for completed results.
                    </p>
                  )}
                </>
              ) : (
                <p>No log output was captured for this benchmark.</p>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
