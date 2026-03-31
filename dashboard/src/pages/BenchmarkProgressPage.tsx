import { useState, useCallback, useEffect, useRef, useMemo } from 'react';
import { useParams, Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkConfigSummary } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { useLiveStore } from '../stores/liveStore';
import type { BenchmarkLive } from '../stores/liveStore';
import { useToast } from '../hooks/useToast';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';

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

interface CellDetail {
  cell_id: string;
  cloud: string;
  region: string;
  topology: string;
  languages: string[];
}

const EMPTY_LIVE: BenchmarkLive = {
  logs: [],
  cells: {},
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
  const [cells, setCells] = useState<CellDetail[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [elapsedStr, setElapsedStr] = useState('--:--');

  // Log viewer state
  const logContainerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

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

  // Load cell details on mount
  useEffect(() => {
    if (!configId || !projectId) return;
    api.getBenchmarkConfig(projectId, configId)
      .then((data) => {
        setConfig(data);
        // Extract cell details from config
        const configData = data as unknown as Record<string, unknown>;
        const rawCells = (configData.cells ?? configData.cell_configs ?? []) as CellDetail[];
        if (Array.isArray(rawCells)) {
          setCells(rawCells);
        }
      })
      .catch(() => {})
      .finally(() => setLoading(false));
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

  // Derive effective status from WS or polled config
  const effectiveStatus = live.configStatus || config?.status || 'pending';
  const isActive = ['running', 'provisioning', 'deploying', 'pending'].includes(effectiveStatus);
  const isDone = ['completed', 'failed', 'cancelled'].includes(effectiveStatus);

  // Parse result artifacts for the results table
  const resultRows = useMemo(() => {
    return live.results.map((r) => {
      const a = r.artifact || {};
      // Try to extract common metrics from the artifact
      const summary = (a.summary ?? a) as Record<string, unknown>;
      return {
        language: r.language,
        run_id: r.run_id,
        mean_latency: typeof summary.mean_latency_ms === 'number' ? summary.mean_latency_ms : null,
        p50: typeof summary.p50_ms === 'number' ? summary.p50_ms : null,
        p99: typeof summary.p99_ms === 'number' ? summary.p99_ms : null,
        success_rate: typeof summary.success_rate === 'number' ? summary.success_rate : null,
        runtime_ms: typeof summary.runtime_ms === 'number' ? summary.runtime_ms : null,
      };
    }).sort((a, b) => (a.mean_latency ?? Infinity) - (b.mean_latency ?? Infinity));
  }, [live.results]);

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
      {isActive && (
        <div className="mb-6 border border-gray-800 rounded p-4 bg-[var(--bg-card)]">
          <div className="flex items-center gap-1 mb-3">
            {['queued', 'provisioning', 'deploying', 'running', 'collecting', 'done'].map((phase, i) => {
              const phases = ['queued', 'provisioning', 'deploying', 'running', 'collecting', 'done'];
              const currentIdx = phases.indexOf(effectiveStatus === 'pending' ? 'queued' : effectiveStatus === 'completed' ? 'done' : effectiveStatus);
              const isCurrentPhase = i === currentIdx;
              const isPast = i < currentIdx;
              return (
                <div key={phase} className="flex items-center gap-1 flex-1">
                  <div className={`h-1.5 flex-1 rounded-full transition-colors ${
                    isPast ? 'bg-cyan-500' :
                    isCurrentPhase ? 'bg-cyan-500 animate-pulse' :
                    'bg-gray-800'
                  }`} />
                  {i < phases.length - 1 && <span className="text-gray-700 text-[8px]">{'\u25B8'}</span>}
                </div>
              );
            })}
          </div>
          <div className="flex items-center justify-between text-[10px] text-gray-600 font-mono">
            <span>queued</span>
            <span>provision</span>
            <span>deploy</span>
            <span className={effectiveStatus === 'running' ? 'text-cyan-400' : ''}>running</span>
            <span>collect</span>
            <span>done</span>
          </div>
        </div>
      )}

      {/* Config Summary (what's being benchmarked) */}
      {config && isActive && (
        <div className="mb-6 border border-gray-800/50 rounded p-3 bg-gray-900/30">
          <div className="flex items-center gap-4 text-xs text-gray-500">
            <span>
              <span className="text-gray-400">{cells.length}</span> cell{cells.length !== 1 ? 's' : ''}
            </span>
            <span>{'\u00B7'}</span>
            <span>
              <span className="text-gray-400">{cells[0]?.languages?.length ?? '?'}</span> languages
            </span>
            <span>{'\u00B7'}</span>
            <span className="text-gray-400">
              {(() => {
                const cfg = (config as unknown as Record<string, unknown>).config_json as Record<string, unknown> | undefined;
                const meth = cfg?.methodology as Record<string, unknown> | undefined;
                if (meth) return `${meth.warmup_runs ?? '?'} warmup + ${meth.measured_runs ?? meth.min_measured ?? '?'} measured`;
                return 'methodology loading...';
              })()}
            </span>
            <span>{'\u00B7'}</span>
            <span>
              est. {(() => {
                const langCount = cells[0]?.languages?.length ?? 3;
                const cellCount = cells.length || 1;
                const runsPerLang = 15; // rough estimate
                const secsPerProbe = 2;
                const total = langCount * cellCount * runsPerLang * secsPerProbe;
                if (total > 3600) return `${Math.round(total / 3600)}h`;
                return `${Math.round(total / 60)}m`;
              })()}
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

      {/* Cell Status Cards */}
      {cells.length > 0 && (
        <div className="mb-6">
          <p className="text-xs text-gray-500 tracking-wider font-medium mb-3">cell status</p>
          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
            {cells.map((cell, idx) => {
              const cellLive = live.cells[cell.cell_id];
              const cellStatus = cellLive?.status || 'pending';
              const currentLang = cellLive?.current_language;
              const langIdx = cellLive?.language_index;
              const langTotal = cellLive?.language_total ?? cell.languages?.length ?? 0;
              const progressPct = langTotal > 0 && langIdx != null
                ? Math.round(((langIdx + 1) / langTotal) * 100)
                : 0;
              const cellDone = cellStatus === 'completed' || cellStatus === 'failed';

              return (
                <div
                  key={cell.cell_id || idx}
                  className={`rounded-lg p-4 transition-all duration-500 ${
                    cellStatus === 'running'
                      ? 'border border-cyan-500/40 bg-cyan-950/10 shadow-[0_0_12px_rgba(71,191,255,0.06)]'
                      : cellStatus === 'completed'
                        ? 'border border-green-500/30 bg-green-950/10'
                        : cellStatus === 'failed'
                          ? 'border border-red-500/30 bg-red-950/10'
                          : 'border border-gray-800 bg-[var(--bg-surface)]/40'
                  }`}
                >
                  <div className="flex items-center justify-between mb-2">
                    <div className="flex items-center gap-2">
                      <span className="text-xs font-bold text-gray-300 bg-gray-800 rounded px-1.5 py-0.5">
                        {CLOUD_ICONS[cell.cloud] || cell.cloud}
                      </span>
                      <span className="text-xs text-gray-400 font-mono">{cell.region}</span>
                    </div>
                    <span className={`text-xs font-medium ${STATUS_COLORS[cellStatus] || 'text-gray-500'}`}>
                      {cellStatus}
                      {cellStatus === 'running' && (
                        <span className="inline-block w-1.5 h-1.5 rounded-full bg-cyan-400 ml-1.5 motion-safe:animate-pulse" />
                      )}
                    </span>
                  </div>

                  <div className="text-[11px] text-gray-500 mb-2">{cell.topology}</div>

                  {currentLang && !cellDone && (
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
                        cellDone
                          ? cellStatus === 'completed' ? 'bg-green-500' : 'bg-red-500'
                          : 'bg-cyan-500'
                      }`}
                      style={{ width: `${cellDone ? 100 : progressPct}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Results Table */}
      {resultRows.length > 0 && (
        <div className="mb-6">
          <p className="text-xs text-gray-500 tracking-wider font-medium mb-3">results</p>
          <div className="border border-gray-800 rounded-lg overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-[var(--bg-surface)]/60 text-gray-500 text-xs">
                  <th className="text-left px-4 py-2 font-medium">Language</th>
                  <th className="text-right px-4 py-2 font-medium">Mean Latency</th>
                  <th className="text-right px-4 py-2 font-medium">p50</th>
                  <th className="text-right px-4 py-2 font-medium">p99</th>
                  <th className="text-right px-4 py-2 font-medium">Success Rate</th>
                  <th className="text-right px-4 py-2 font-medium">Runtime</th>
                </tr>
              </thead>
              <tbody>
                {resultRows.map((row, i) => (
                  <tr
                    key={row.run_id || i}
                    className={`border-t border-gray-800/50 ${i === 0 ? 'bg-cyan-500/5' : ''}`}
                  >
                    <td className="px-4 py-2 text-gray-200 font-mono text-xs">{row.language}</td>
                    <td className="px-4 py-2 text-right text-gray-300 font-mono text-xs">
                      {row.mean_latency != null ? `${row.mean_latency.toFixed(2)} ms` : '--'}
                    </td>
                    <td className="px-4 py-2 text-right text-gray-400 font-mono text-xs">
                      {row.p50 != null ? `${row.p50.toFixed(2)} ms` : '--'}
                    </td>
                    <td className="px-4 py-2 text-right text-gray-400 font-mono text-xs">
                      {row.p99 != null ? `${row.p99.toFixed(2)} ms` : '--'}
                    </td>
                    <td className="px-4 py-2 text-right text-gray-400 font-mono text-xs">
                      {row.success_rate != null ? `${(row.success_rate * 100).toFixed(1)}%` : '--'}
                    </td>
                    <td className="px-4 py-2 text-right text-gray-500 font-mono text-xs">
                      {row.runtime_ms != null ? `${(row.runtime_ms / 1000).toFixed(1)}s` : '--'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Live Log Viewer */}
      <div className="mb-6">
        <div className="flex items-center justify-between mb-2">
          <p className="text-xs text-gray-500 tracking-wider font-medium">live log</p>
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
          {live.logs.length === 0 ? (
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
                          : 'Orchestrator running — log output will stream here...'}
                  </p>
                  <p className="text-gray-700 text-[10px]">
                    Logs stream in real-time via WebSocket. If nothing appears after 30 seconds, check System {'>'} Logs for errors.
                  </p>
                </>
              ) : (
                <p>No log output was captured for this benchmark.</p>
              )}
            </div>
          ) : (
            live.logs.map((line, i) => (
              <div key={i} className="text-gray-300 whitespace-pre-wrap break-all">
                {line}
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
