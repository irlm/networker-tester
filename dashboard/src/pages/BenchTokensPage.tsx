import { useState, useCallback } from 'react';
import { api } from '../api/client';
import type { BenchTokenInfo } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { Breadcrumb } from '../components/common/Breadcrumb';

function formatDate(iso: string | null): string {
  if (!iso) return '\u2014';
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function expiryColor(iso: string | null): string {
  if (!iso) return 'text-gray-500';
  const remaining = new Date(iso).getTime() - Date.now();
  if (remaining <= 0) return 'text-red-400';
  if (remaining < 3600_000) return 'text-yellow-400';
  return 'text-green-400';
}

function expiryLabel(iso: string | null): string {
  if (!iso) return '';
  const remaining = new Date(iso).getTime() - Date.now();
  if (remaining <= 0) return 'expired';
  if (remaining < 3600_000) {
    const mins = Math.max(1, Math.floor(remaining / 60_000));
    return `${mins}m left`;
  }
  const hours = Math.floor(remaining / 3600_000);
  if (hours < 24) return `${hours}h left`;
  const days = Math.floor(hours / 24);
  return `${days}d left`;
}

export function BenchTokensPage() {
  usePageTitle('Benchmark Tokens');
  const toast = useToast();

  const [tokens, setTokens] = useState<BenchTokenInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [revokingAll, setRevokingAll] = useState(false);
  const [activeOpen, setActiveOpen] = useState(true);
  const [disabledOpen, setDisabledOpen] = useState(true);

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

  const handleRevoke = async (name: string) => {
    if (!window.confirm(`Revoke token "${name}"? This will immediately invalidate the token.`)) return;
    setRevoking(name);
    try {
      await api.revokeBenchToken(name);
      toast('success', `Token "${name}" revoked`);
      refresh();
    } catch {
      toast('error', `Failed to revoke "${name}"`);
    } finally {
      setRevoking(null);
    }
  };

  const handleRevokeAll = async () => {
    if (!window.confirm(`Revoke ALL ${tokens.length} benchmark tokens? This action cannot be undone.`)) return;
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

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <Breadcrumb items={[{ label: 'Benchmark Tokens' }]} />

      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-lg md:text-xl font-bold text-gray-100">Benchmark Tokens</h1>
        {tokens.length > 0 && (
          <button
            onClick={handleRevokeAll}
            disabled={revokingAll}
            className="px-3 py-1.5 text-xs rounded border border-red-700 text-red-400 hover:bg-red-500/10 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
          >
            {revokingAll ? 'Revoking...' : 'Revoke All'}
          </button>
        )}
      </div>

      {/* Loading state */}
      {loading && (
        <div className="py-16 text-center">
          <span className="text-gray-500 text-sm motion-safe:animate-pulse">Loading tokens...</span>
        </div>
      )}

      {/* Error / empty state */}
      {!loading && error && (
        <div className="py-16 text-center">
          <p className="text-gray-500 text-sm">{error}</p>
        </div>
      )}

      {!loading && !error && tokens.length === 0 && (
        <div className="py-16 text-center">
          <p className="text-gray-500 text-sm">No active benchmark tokens</p>
        </div>
      )}

      {/* Token sections */}
      {!loading && !error && tokens.length > 0 && (() => {
        const active = tokens.filter((t) => t.enabled);
        const disabled = tokens.filter((t) => !t.enabled);

        const renderTable = (items: BenchTokenInfo[]) => (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800 text-left text-xs text-gray-500 uppercase tracking-wider">
                <th className="px-3 py-2">Name</th>
                <th className="px-3 py-2">Config</th>
                <th className="px-3 py-2">Testbed</th>
                <th className="px-3 py-2">Created</th>
                <th className="px-3 py-2">Expires</th>
                <th className="px-3 py-2">Status</th>
                <th className="px-3 py-2 text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {items.map((t) => (
                <tr
                  key={t.name}
                  className="border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors"
                >
                  <td className="px-3 py-2.5 font-mono text-gray-200 truncate max-w-[200px]" title={t.name}>
                    {t.name}
                  </td>
                  <td className="px-3 py-2.5 text-gray-400 font-mono text-xs">
                    {t.config_id || '\u2014'}
                  </td>
                  <td className="px-3 py-2.5 text-gray-400 font-mono text-xs">
                    {t.testbed_id || '\u2014'}
                  </td>
                  <td className="px-3 py-2.5 text-gray-500 text-xs whitespace-nowrap">
                    {formatDate(t.created)}
                  </td>
                  <td className="px-3 py-2.5 whitespace-nowrap">
                    <span className={`text-xs ${expiryColor(t.expires)}`}>
                      {formatDate(t.expires)}
                    </span>
                    {t.expires && (
                      <span className={`ml-1.5 text-[10px] ${expiryColor(t.expires)} opacity-70`}>
                        {expiryLabel(t.expires)}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2.5">
                    {t.enabled ? (
                      <span className="inline-flex items-center px-2 py-0.5 text-xs rounded border bg-green-500/20 text-green-400 border-green-500/30">
                        <span className="w-1.5 h-1.5 rounded-full bg-current mr-1.5" aria-hidden="true" />
                        enabled
                      </span>
                    ) : (
                      <span className="inline-flex items-center px-2 py-0.5 text-xs rounded border bg-gray-500/20 text-gray-400 border-gray-500/30">
                        <span className="w-1.5 h-1.5 rounded-full bg-current mr-1.5" aria-hidden="true" />
                        disabled
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2.5 text-right">
                    <button
                      onClick={() => handleRevoke(t.name)}
                      disabled={revoking === t.name}
                      className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
                    >
                      {revoking === t.name ? 'Revoking...' : 'Revoke'}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        );

        return (
          <div className="space-y-4">
            {/* Active tokens */}
            {active.length > 0 && (
              <div className="border border-gray-800 rounded overflow-hidden">
                <button
                  onClick={() => setActiveOpen(!activeOpen)}
                  className="w-full flex items-center justify-between px-3 py-2.5 bg-gray-800/30 hover:bg-gray-800/50 transition-colors text-left"
                >
                  <span className="text-sm text-gray-200 flex items-center gap-2">
                    <span className="w-2 h-2 rounded-full bg-green-400" />
                    Active
                    <span className="text-xs text-gray-500">{active.length}</span>
                  </span>
                  <span className="text-gray-500 text-xs">{activeOpen ? '\u25B2' : '\u25BC'}</span>
                </button>
                {activeOpen && renderTable(active)}
              </div>
            )}

            {/* Disabled / expired tokens */}
            {disabled.length > 0 && (
              <div className="border border-gray-800 rounded overflow-hidden">
                <button
                  onClick={() => setDisabledOpen(!disabledOpen)}
                  className="w-full flex items-center justify-between px-3 py-2.5 bg-gray-800/30 hover:bg-gray-800/50 transition-colors text-left"
                >
                  <span className="text-sm text-gray-400 flex items-center gap-2">
                    <span className="w-2 h-2 rounded-full bg-gray-500" />
                    Disabled / Expired
                    <span className="text-xs text-gray-500">{disabled.length}</span>
                  </span>
                  <span className="text-gray-500 text-xs">{disabledOpen ? '\u25B2' : '\u25BC'}</span>
                </button>
                {disabledOpen && renderTable(disabled)}
              </div>
            )}
          </div>
        );
      })()}

      {/* Footer count */}
      {!loading && !error && tokens.length > 0 && (
        <div className="mt-3 text-xs text-gray-600">
          {tokens.length} token{tokens.length !== 1 ? 's' : ''} &middot; auto-refresh 30s
        </div>
      )}
    </div>
  );
}
