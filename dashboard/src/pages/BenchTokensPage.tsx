import { useState, useCallback, useMemo } from 'react';
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

  const refresh = useCallback(async () => {
    try {
      const data = await api.listBenchTokens();
      setTokens(data);
      setError(null);
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
    <div className="p-4 md:p-6 h-full flex flex-col">
      <Breadcrumb items={[{ label: 'Active Tokens' }]} />

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
            placeholder="Filter config or VM..."
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

      {/* Master-detail layout */}
      {!loading && !error && activeTokens.length > 0 && (
        <div className="flex flex-1 border border-gray-800 rounded overflow-hidden min-h-0">
          {/* Left panel — run list */}
          <div className="w-[35%] min-w-[220px] border-r border-gray-800 overflow-y-auto">
            {filteredRuns.length === 0 && (
              <div className="p-4 text-xs text-gray-500 text-center">No matching runs</div>
            )}
            {filteredRuns.map((run) => {
              const isSelected = run.configId === effectiveSelectedRun;
              return (
                <button
                  key={run.configId}
                  onClick={() => setSelectedRun(run.configId)}
                  className={`w-full text-left px-3 py-2.5 border-b border-gray-800/50 transition-colors ${
                    isSelected ? 'bg-cyan-500/10' : 'hover:bg-gray-800/30'
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-sm text-cyan-400 truncate">{run.configId}</span>
                    <span className="text-[10px] text-gray-500 ml-2 shrink-0">
                      {run.tokens.length} VM{run.tokens.length !== 1 ? 's' : ''}
                    </span>
                  </div>
                  {/* Health dots */}
                  <div className="flex gap-0.5 mt-1.5">
                    {run.tokens.map((t) => (
                      <span
                        key={t.name}
                        className={`inline-block w-2 h-2 rounded-[1px] ${healthDotColor(ttlMs(t.expires))}`}
                        title={`${t.testbed_id}: ${ttlLabel(ttlMs(t.expires))}`}
                      />
                    ))}
                  </div>
                  {/* Revoke run link */}
                  <div className="mt-1">
                    <span
                      onClick={(e) => {
                        e.stopPropagation();
                        handleRevokeRun(run.configId);
                      }}
                      className={`text-[10px] text-red-400/60 hover:text-red-400 transition-colors cursor-pointer ${
                        revokingRun === run.configId ? 'opacity-30 pointer-events-none' : ''
                      }`}
                    >
                      {revokingRun === run.configId ? 'revoking...' : 'revoke run'}
                    </span>
                  </div>
                </button>
              );
            })}
          </div>

          {/* Right panel — VM detail */}
          <div className="w-[65%] overflow-y-auto">
            {!selectedRunData && (
              <div className="p-8 text-center text-gray-500 text-sm">Select a run</div>
            )}
            {selectedRunData && (
              <>
                {/* Detail header */}
                <div className="flex items-center justify-between px-4 py-3 border-b border-gray-800 bg-gray-800/20">
                  <div className="flex items-center gap-3">
                    <span className="font-mono text-sm text-cyan-400">{selectedRunData.configId}</span>
                    <span className="text-xs text-gray-500">
                      {selectedRunData.tokens.length} VM{selectedRunData.tokens.length !== 1 ? 's' : ''}
                    </span>
                  </div>
                  <button
                    onClick={() => handleRevokeRun(selectedRunData.configId)}
                    disabled={revokingRun === selectedRunData.configId}
                    className="px-2.5 py-1 text-xs rounded border border-red-700/50 text-red-400 hover:bg-red-500/10 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
                  >
                    {revokingRun === selectedRunData.configId ? 'Revoking...' : 'Revoke Run'}
                  </button>
                </div>

                {/* VM table */}
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-gray-800 text-left text-[10px] text-gray-500 uppercase tracking-wider">
                      <th className="px-4 py-2">VM</th>
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
                            {t.testbed_id || t.name}
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
      )}

      {/* Footer */}
      {!loading && !error && activeTokens.length > 0 && (
        <div className="mt-2 text-[10px] text-gray-600">
          auto-refresh 30s
        </div>
      )}
    </div>
  );
}
