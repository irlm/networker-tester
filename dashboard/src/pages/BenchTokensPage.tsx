import { useState, useCallback, useMemo, useEffect } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchTokenInfo } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { Breadcrumb } from '../components/common/Breadcrumb';

// ── Helpers ─────────────────────────────────────────────────────────────

function ttlMs(expires: string | null): number {
  if (!expires) return 0;
  return new Date(expires).getTime() - Date.now();
}

function ttlColor(ms: number): string {
  if (ms <= 0) return 'text-red-400';
  if (ms < 1800_000) return 'text-red-400';
  if (ms < 3600_000) return 'text-yellow-400';
  return 'text-green-400';
}

function ttlCssColor(ms: number): string {
  if (ms <= 0) return '#f87171';
  if (ms < 1800_000) return '#f87171';
  if (ms < 3600_000) return '#facc15';
  return '#4ade80';
}

function ttlLabel(ms: number): string {
  if (ms <= 0) return 'expired';
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem > 0 ? `${hrs}h ${rem}m` : `${hrs}h`;
}

function ttlPercent(ms: number): number {
  return Math.max(0, Math.min(100, (ms / (4 * 3600_000)) * 100));
}

function relativeDate(iso: string | null): string {
  if (!iso) return '\u2014';
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.floor(hrs / 24);
  return `${days}d ago`;
}

function healthDotColor(ms: number): string {
  if (ms <= 0) return 'bg-red-400';
  if (ms < 1800_000) return 'bg-red-400';
  if (ms < 3600_000) return 'bg-yellow-400';
  return 'bg-green-400';
}

// ── Types ───────────────────────────────────────────────────────────────

interface RunGroup {
  configId: string;
  tokens: BenchTokenInfo[];
}

// ── Component ───────────────────────────────────────────────────────────

export function BenchTokensPage() {
  usePageTitle('Active Tokens');
  const toast = useToast();

  const [tokens, setTokens] = useState<BenchTokenInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedRun, setSelectedRun] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [revoking, setRevoking] = useState<string | null>(null);
  const [revokingAll, setRevokingAll] = useState(false);
  const [revokingRun, setRevokingRun] = useState<string | null>(null);
  const [lastRefresh, setLastRefresh] = useState<number>(Date.now());
  const [, setTick] = useState(0); // force re-render for relative time

  // Tick every 5s so "refreshed Xs ago" updates
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), 5000);
    return () => clearInterval(id);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const data = await api.listBenchTokens();
      setTokens(data);
      setError(null);
      setLastRefresh(Date.now());
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to fetch tokens';
      if (msg.includes('404') || msg.includes('501')) {
        setError('Key Vault not configured');
      } else {
        setError(msg);
      }
      setTokens([]);
    } finally {
      setLoading(false);
    }
  }, []);

  usePolling(refresh, 30_000);

  // ── Derived state ───────────────────────────────────────────────────

  const activeTokens = useMemo(() => tokens.filter((t) => t.enabled), [tokens]);

  const runs: RunGroup[] = useMemo(() => {
    const map = new Map<string, BenchTokenInfo[]>();
    for (const t of activeTokens) {
      const key = t.config_id || 'unknown';
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(t);
    }
    // Sort runs: newest first (by earliest created token)
    const entries = Array.from(map.entries()).map(([configId, toks]) => ({
      configId,
      tokens: toks.sort((a, b) => ttlMs(b.expires) - ttlMs(a.expires)),
    }));
    entries.sort((a, b) => {
      const aCreated = a.tokens[0]?.created ?? '';
      const bCreated = b.tokens[0]?.created ?? '';
      return bCreated.localeCompare(aCreated);
    });
    return entries;
  }, [activeTokens]);

  const filteredRuns = useMemo(() => {
    if (!filter) return runs;
    const q = filter.toLowerCase();
    return runs.filter(
      (r) =>
        r.configId.toLowerCase().includes(q) ||
        r.tokens.some((t) => t.testbed_id.toLowerCase().includes(q)),
    );
  }, [runs, filter]);

  // Auto-select first run if nothing selected or selected run gone
  const effectiveSelectedRun = useMemo(() => {
    if (selectedRun && filteredRuns.some((r) => r.configId === selectedRun)) {
      return selectedRun;
    }
    return filteredRuns.length > 0 ? filteredRuns[0].configId : null;
  }, [selectedRun, filteredRuns]);

  const selectedRunData = useMemo(
    () => filteredRuns.find((r) => r.configId === effectiveSelectedRun) ?? null,
    [filteredRuns, effectiveSelectedRun],
  );

  const totalVms = activeTokens.length;
  const totalRuns = runs.length;

  // ── Actions ─────────────────────────────────────────────────────────

  const handleRevoke = async (name: string) => {
    if (!window.confirm(`Revoke token "${name}"?`)) return;
    setRevoking(name);
    try {
      await api.revokeBenchToken(name);
      toast('success', `Token revoked`);
      refresh();
    } catch {
      toast('error', `Failed to revoke token`);
    } finally {
      setRevoking(null);
    }
  };

  const handleRevokeRun = async (configId: string) => {
    const run = runs.find((r) => r.configId === configId);
    if (!run) return;
    if (!window.confirm(`Revoke all ${run.tokens.length} tokens for run ${configId}?`)) return;
    setRevokingRun(configId);
    try {
      for (const t of run.tokens) {
        await api.revokeBenchToken(t.name);
      }
      toast('success', `Revoked ${run.tokens.length} tokens for ${configId}`);
      refresh();
    } catch {
      toast('error', `Failed to revoke run ${configId}`);
    } finally {
      setRevokingRun(null);
    }
  };

  const handleRevokeAll = async () => {
    if (!window.confirm(`Revoke ALL ${totalVms} active tokens? This cannot be undone.`)) return;
    setRevokingAll(true);
    try {
      const result = await api.revokeAllBenchTokens();
      toast('success', `Revoked ${result.deleted} token${result.deleted !== 1 ? 's' : ''}`);
      refresh();
    } catch {
      toast('error', 'Failed to revoke tokens');
    } finally {
      setRevokingAll(false);
    }
  };

  // ── Render ──────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Admin' }, { label: 'Tokens' }]} />

      {/* Top bar */}
      <div className="flex items-center justify-between mb-4 flex-wrap gap-2">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-bold text-gray-100">Active Tokens</h1>
          {!loading && !error && (
            <span className="text-xs text-gray-500">
              {totalRuns} run{totalRuns !== 1 ? 's' : ''} &middot; {totalVms} VM{totalVms !== 1 ? 's' : ''}
            </span>
          )}
        </div>
        <div className="flex items-center gap-3">
          <input
            type="text"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter config, VM, user..."
            className="px-2.5 py-1.5 text-xs bg-gray-800 border border-gray-700 rounded text-gray-200 placeholder-gray-600 focus:outline-none focus:border-cyan-500/50 w-48"
          />
          {totalVms > 0 && (
            <button
              onClick={handleRevokeAll}
              disabled={revokingAll}
              className="px-3 py-1.5 text-xs rounded border border-red-700 text-red-400 hover:bg-red-500/10 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
            >
              {revokingAll ? 'Revoking...' : `Revoke All (${totalVms})`}
            </button>
          )}
          <Link
            to="/bench-tokens/history"
            className="text-xs text-gray-400 hover:text-cyan-400 transition-colors"
          >
            History &rarr;
          </Link>
        </div>
      </div>

      {/* Loading state */}
      {loading && (
        <div className="py-16 text-center">
          <span className="text-gray-500 text-sm motion-safe:animate-pulse">Loading tokens...</span>
        </div>
      )}

      {/* Error state */}
      {!loading && error && (
        <div className="py-16 text-center">
          <p className="text-gray-500 text-sm">{error}</p>
        </div>
      )}

      {/* Empty state */}
      {!loading && !error && activeTokens.length === 0 && (
        <div className="py-16 text-center">
          <p className="text-gray-500 text-sm">No active benchmark tokens</p>
          {tokens.length > 0 && (
            <Link
              to="/bench-tokens/history"
              className="text-xs text-gray-500 hover:text-cyan-400 transition-colors mt-2 inline-block"
            >
              View {tokens.length} historical token{tokens.length !== 1 ? 's' : ''} &rarr;
            </Link>
          )}
        </div>
      )}

      {/* Adaptive layout: pills for <4 runs, master-detail panel for 4+ */}
      {!loading && !error && activeTokens.length > 0 && (
        <div className="flex flex-col">
          {/* Run selector: horizontal pills when few runs, vertical panel when many */}
          {filteredRuns.length < 4 ? (
            /* Horizontal pills */
            <div className="flex gap-2 mb-3 flex-wrap">
              {filteredRuns.map((run) => {
                const isSelected = run.configId === effectiveSelectedRun;
                const label = run.configId === 'unknown' ? 'Ungrouped' : run.configId;
                return (
                  <button
                    key={run.configId}
                    onClick={() => setSelectedRun(run.configId)}
                    className={`flex items-center gap-2 px-3 py-1.5 rounded border text-xs transition-colors ${
                      isSelected
                        ? 'bg-cyan-500/10 border-cyan-700 text-cyan-300'
                        : 'border-gray-700 text-gray-400 hover:border-gray-600'
                    }`}
                  >
                    <span className="font-mono">{label}</span>
                    <div className="flex gap-0.5">
                      {run.tokens.map((t) => (
                        <span
                          key={t.name}
                          className={`inline-block w-2.5 h-2.5 rounded-[1px] ${healthDotColor(ttlMs(t.expires))}`}
                          title={`${t.testbed_id}: ${ttlLabel(ttlMs(t.expires))}`}
                        />
                      ))}
                    </div>
                    <span className="text-gray-600">{run.tokens.length}</span>
                  </button>
                );
              })}
            </div>
          ) : null}

          <div className="flex border border-gray-800 rounded overflow-hidden">
            {/* Left panel — only show for 4+ runs */}
            {filteredRuns.length >= 4 && (
              <div className="w-[25%] min-w-[180px] border-r border-gray-800 overflow-y-auto">
                {filteredRuns.map((run) => {
                  const isSelected = run.configId === effectiveSelectedRun;
                  const label = run.configId === 'unknown' ? 'Ungrouped' : run.configId;
                  return (
                    <button
                      key={run.configId}
                      onClick={() => setSelectedRun(run.configId)}
                      className={`w-full text-left px-3 py-2 border-b border-gray-800/50 transition-colors ${
                        isSelected ? 'bg-cyan-500/10' : 'hover:bg-gray-800/30'
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <span className={`font-mono text-xs truncate ${run.configId === 'unknown' ? 'text-gray-500 italic' : 'text-cyan-400'}`}>
                          {label}
                        </span>
                        <span className="text-[10px] text-gray-600 ml-2 shrink-0">
                          {run.tokens.length}
                        </span>
                      </div>
                      <div className="flex gap-0.5 mt-1">
                        {run.tokens.map((t) => (
                          <span
                            key={t.name}
                            className={`inline-block w-2.5 h-2.5 rounded-[1px] ${healthDotColor(ttlMs(t.expires))}`}
                            title={`${t.testbed_id}: ${ttlLabel(ttlMs(t.expires))}`}
                          />
                        ))}
                      </div>
                    </button>
                  );
                })}
              </div>
            )}

            {/* Detail panel — VM table */}
            <div className={`${filteredRuns.length >= 4 ? 'w-[75%]' : 'w-full'} overflow-y-auto`}>
            {!selectedRunData && (
              <div className="p-8 text-center text-gray-500 text-sm">Select a run</div>
            )}
            {selectedRunData && (
              <>
                {/* Detail header */}
                <div className="flex items-center justify-between px-4 py-3 border-b border-gray-800 bg-gray-800/20">
                  <div className="flex items-center gap-3">
                    <span className={`font-mono text-sm ${selectedRunData.configId === 'unknown' ? 'text-gray-500 italic' : 'text-cyan-400'}`}>
                      {selectedRunData.configId === 'unknown' ? 'Ungrouped' : selectedRunData.configId}
                    </span>
                    <span className="text-xs text-gray-500">
                      {selectedRunData.tokens.length} VM{selectedRunData.tokens.length !== 1 ? 's' : ''}
                    </span>
                  </div>
                  {/* Only show Revoke Run when there are 2+ runs (otherwise Revoke All suffices) */}
                  {filteredRuns.length > 1 && (
                    <button
                      onClick={() => handleRevokeRun(selectedRunData.configId)}
                      disabled={revokingRun === selectedRunData.configId}
                      className="px-2.5 py-1 text-xs rounded border border-red-700/50 text-red-400 hover:bg-red-500/10 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
                    >
                      {revokingRun === selectedRunData.configId ? 'Revoking...' : 'Revoke Run'}
                    </button>
                  )}
                </div>

                {/* VM table */}
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-gray-800 text-left text-[10px] text-gray-500 uppercase tracking-wider">
                      <th className="px-4 py-2">VM</th>
                      <th className="px-4 py-2">User</th>
                      <th className="px-4 py-2">Created</th>
                      <th className="px-4 py-2">TTL</th>
                      <th className="px-4 py-2 text-right">Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selectedRunData.tokens.map((t) => {
                      const ms = ttlMs(t.expires);
                      const pct = ttlPercent(ms);
                      const isCritical = ms > 0 && ms < 1800_000;
                      return (
                        <tr
                          key={t.name}
                          className={`border-b border-gray-800/50 transition-colors ${
                            isCritical ? 'bg-red-500/5' : 'hover:bg-gray-800/20'
                          }`}
                        >
                          <td className="px-4 py-2.5 font-mono text-xs text-gray-300" title={t.name}>
                            {t.testbed_id || t.name.replace(/^bench-[^-]+-vm-/, '')}
                          </td>
                          <td className="px-4 py-2.5 text-xs text-gray-500 truncate max-w-[140px]" title={t.user ?? undefined}>
                            {t.user ?? '\u2014'}
                          </td>
                          <td className="px-4 py-2.5 text-xs text-gray-500 whitespace-nowrap">
                            {relativeDate(t.created)}
                          </td>
                          <td className="px-4 py-2.5 whitespace-nowrap">
                            <span className={`text-xs font-mono ${ttlColor(ms)}`}>
                              {ttlLabel(ms)}
                            </span>
                            <div className="inline-block w-8 h-0.5 bg-gray-800 rounded-sm align-middle ml-1">
                              <div
                                className="h-0.5 rounded-sm"
                                style={{ width: `${pct}%`, backgroundColor: ttlCssColor(ms) }}
                              />
                            </div>
                          </td>
                          <td className="px-4 py-2.5 text-right">
                            <button
                              onClick={() => handleRevoke(t.name)}
                              disabled={revoking === t.name}
                              className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
                            >
                              {revoking === t.name ? '...' : 'Revoke'}
                            </button>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </>
            )}
          </div>
          </div>
        </div>
      )}

      {/* Status summary + footer */}
      {!loading && !error && activeTokens.length > 0 && (() => {
        const critical = activeTokens.filter(t => { const ms = ttlMs(t.expires); return ms > 0 && ms < 1800_000; }).length;
        const warning = activeTokens.filter(t => { const ms = ttlMs(t.expires); return ms >= 1800_000 && ms < 3600_000; }).length;
        const healthy = activeTokens.length - critical - warning;
        const nextExpiry = Math.min(...activeTokens.map(t => ttlMs(t.expires)).filter(ms => ms > 0));
        const s = Math.floor((Date.now() - lastRefresh) / 1000);

        return (
          <div className="mt-3 flex items-center justify-between text-[10px] text-gray-600 border-t border-gray-800/50 pt-2">
            <div className="flex items-center gap-4">
              {healthy > 0 && <span><span className="text-green-400">{healthy}</span> healthy</span>}
              {warning > 0 && <span><span className="text-yellow-400">{warning}</span> expiring &lt;1h</span>}
              {critical > 0 && <span><span className="text-red-400">{critical}</span> critical &lt;30m</span>}
              {nextExpiry > 0 && nextExpiry < Infinity && (
                <span>next expiry in <span className={ttlColor(nextExpiry)}>{ttlLabel(nextExpiry)}</span></span>
              )}
            </div>
            <span>{s < 5 ? 'refreshed just now' : `refreshed ${s}s ago`}</span>
          </div>
        );
      })()}
    </div>
  );
}
