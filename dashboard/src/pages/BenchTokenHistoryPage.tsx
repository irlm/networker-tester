import { useState, useCallback, useMemo } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchTokenInfo } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { Breadcrumb } from '../components/common/Breadcrumb';

// ── Helpers ─────────────────────────────────────────────────────────────

const MAX_TTL_MS = 4 * 3600_000; // 4 hours assumed max

function formatAbsDate(iso: string | null): string {
  if (!iso) return '\u2014';
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function livedMs(created: string | null, expires: string | null): number | null {
  if (!created || !expires) return null;
  return new Date(expires).getTime() - new Date(created).getTime();
}

function livedLabel(ms: number | null): string {
  if (ms === null) return '\u2014';
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem > 0 ? `${hrs}h ${String(rem).padStart(2, '0')}m` : `${hrs}h`;
}

function howRevoked(token: BenchTokenInfo): { label: string; color: string } {
  if (token.enabled) return { label: 'active', color: 'text-green-400' };
  const lived = livedMs(token.created, token.expires);
  // If it lived less than 87.5% of max TTL, it was likely manually revoked
  if (lived !== null && lived < MAX_TTL_MS * 0.875) {
    return { label: 'revoked', color: 'text-red-400' };
  }
  return { label: 'auto-expired', color: 'text-gray-500' };
}

type TimeFilter = 'all' | '24h' | '7d' | '30d';

const TIME_FILTERS: { value: TimeFilter; label: string }[] = [
  { value: 'all', label: 'All time' },
  { value: '24h', label: 'Last 24h' },
  { value: '7d', label: 'Last 7d' },
  { value: '30d', label: 'Last 30d' },
];

function filterByTime(tokens: BenchTokenInfo[], tf: TimeFilter): BenchTokenInfo[] {
  if (tf === 'all') return tokens;
  const now = Date.now();
  const cutoff: Record<TimeFilter, number> = {
    all: 0,
    '24h': now - 24 * 3600_000,
    '7d': now - 7 * 24 * 3600_000,
    '30d': now - 30 * 24 * 3600_000,
  };
  return tokens.filter((t) => {
    if (!t.created) return true;
    return new Date(t.created).getTime() >= cutoff[tf];
  });
}

const PAGE_SIZE = 10;

// ── Component ───────────────────────────────────────────────────────────

export function BenchTokenHistoryPage() {
  usePageTitle('Token History');

  const [tokens, setTokens] = useState<BenchTokenInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [timeFilter, setTimeFilter] = useState<TimeFilter>('all');
  const [page, setPage] = useState(0);

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

  // Only show disabled/expired tokens (history)
  const historyTokens = useMemo(
    () => tokens.filter((t) => !t.enabled),
    [tokens],
  );

  const filtered = useMemo(() => {
    let result = filterByTime(historyTokens, timeFilter);
    if (search) {
      const q = search.toLowerCase();
      result = result.filter(
        (t) =>
          t.config_id.toLowerCase().includes(q) ||
          t.testbed_id.toLowerCase().includes(q) ||
          (t.created && t.created.toLowerCase().includes(q)) ||
          t.name.toLowerCase().includes(q),
      );
    }
    // Sort newest first by created date
    result.sort((a, b) => {
      const ac = a.created ?? '';
      const bc = b.created ?? '';
      return bc.localeCompare(ac);
    });
    return result;
  }, [historyTokens, timeFilter, search]);

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const safePage = Math.min(page, totalPages - 1);
  const pageTokens = filtered.slice(safePage * PAGE_SIZE, (safePage + 1) * PAGE_SIZE);
  const rangeStart = filtered.length > 0 ? safePage * PAGE_SIZE + 1 : 0;
  const rangeEnd = Math.min((safePage + 1) * PAGE_SIZE, filtered.length);

  // Reset page on filter change
  const handleSearchChange = (v: string) => {
    setSearch(v);
    setPage(0);
  };
  const handleTimeChange = (v: TimeFilter) => {
    setTimeFilter(v);
    setPage(0);
  };

  return (
    <div className="p-4 md:p-6 max-w-6xl">
      <Breadcrumb
        items={[
          { label: 'Active Tokens', to: '/bench-tokens' },
          { label: 'History' },
        ]}
      />

      {/* Top bar */}
      <div className="flex items-center justify-between mb-4 flex-wrap gap-2">
        <div className="flex items-center gap-3">
          <Link
            to="/bench-tokens"
            className="text-xs text-gray-400 hover:text-cyan-400 transition-colors"
          >
            &larr; Active Tokens
          </Link>
          <h1 className="text-lg font-bold text-gray-100">Token History</h1>
          {!loading && !error && (
            <span className="text-xs text-gray-500">
              {filtered.length} token{filtered.length !== 1 ? 's' : ''}
            </span>
          )}
        </div>
      </div>

      {/* Filters row */}
      <div className="flex items-center gap-3 mb-4">
        <input
          type="text"
          value={search}
          onChange={(e) => handleSearchChange(e.target.value)}
          placeholder="Search config, VM, date..."
          className="px-2.5 py-1.5 text-xs bg-gray-800 border border-gray-700 rounded text-gray-200 placeholder-gray-600 focus:outline-none focus:border-cyan-500/50 w-56"
        />
        <select
          value={timeFilter}
          onChange={(e) => handleTimeChange(e.target.value as TimeFilter)}
          className="px-2.5 py-1.5 text-xs bg-gray-800 border border-gray-700 rounded text-gray-200 focus:outline-none focus:border-cyan-500/50 cursor-pointer"
        >
          {TIME_FILTERS.map((tf) => (
            <option key={tf.value} value={tf.value}>
              {tf.label}
            </option>
          ))}
        </select>
      </div>

      {/* Loading */}
      {loading && (
        <div className="py-16 text-center">
          <span className="text-gray-500 text-sm motion-safe:animate-pulse">Loading history...</span>
        </div>
      )}

      {/* Error */}
      {!loading && error && (
        <div className="py-16 text-center">
          <p className="text-gray-500 text-sm">{error}</p>
        </div>
      )}

      {/* Empty */}
      {!loading && !error && filtered.length === 0 && (
        <div className="py-16 text-center">
          <p className="text-gray-500 text-sm">No historical tokens found</p>
        </div>
      )}

      {/* Table */}
      {!loading && !error && filtered.length > 0 && (
        <div className="border border-gray-800 rounded overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800 text-left text-[10px] text-gray-500 uppercase tracking-wider">
                <th className="px-4 py-2">Config</th>
                <th className="px-4 py-2">VM</th>
                <th className="px-4 py-2">Created</th>
                <th className="px-4 py-2">Expired</th>
                <th className="px-4 py-2">Lived</th>
                <th className="px-4 py-2">How</th>
              </tr>
            </thead>
            <tbody>
              {pageTokens.map((t) => {
                const lived = livedMs(t.created, t.expires);
                const how = howRevoked(t);
                const isShortLived = lived !== null && lived < MAX_TTL_MS * 0.875;
                return (
                  <tr
                    key={t.name}
                    className="border-b border-gray-800/50 opacity-70"
                  >
                    <td className="px-4 py-2.5 font-mono text-xs text-cyan-400">
                      {t.config_id || '\u2014'}
                    </td>
                    <td className="px-4 py-2.5 font-mono text-xs text-gray-400">
                      {t.testbed_id || '\u2014'}
                    </td>
                    <td className="px-4 py-2.5 text-xs text-gray-500 whitespace-nowrap">
                      {formatAbsDate(t.created)}
                    </td>
                    <td className={`px-4 py-2.5 text-xs whitespace-nowrap ${how.label === 'revoked' ? 'text-red-400' : 'text-gray-500'}`}>
                      {formatAbsDate(t.expires)}
                    </td>
                    <td className={`px-4 py-2.5 text-xs font-mono whitespace-nowrap ${isShortLived ? 'text-yellow-400' : 'text-gray-500'}`}>
                      {livedLabel(lived)}
                    </td>
                    <td className={`px-4 py-2.5 text-xs ${how.color}`}>
                      {how.label}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>

          {/* Pagination */}
          <div className="flex items-center justify-between px-4 py-2.5 border-t border-gray-800 bg-gray-800/20">
            <span className="text-[10px] text-gray-500">
              {rangeStart}-{rangeEnd} of {filtered.length}
            </span>
            <div className="flex items-center gap-1">
              <button
                onClick={() => setPage(Math.max(0, safePage - 1))}
                disabled={safePage === 0}
                className="px-2 py-0.5 text-[10px] rounded text-gray-400 hover:text-gray-200 hover:bg-gray-700/50 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
              >
                Prev
              </button>
              {Array.from({ length: Math.min(totalPages, 7) }, (_, i) => {
                // Show pages around current
                let pageNum: number;
                if (totalPages <= 7) {
                  pageNum = i;
                } else if (safePage < 4) {
                  pageNum = i;
                } else if (safePage > totalPages - 4) {
                  pageNum = totalPages - 7 + i;
                } else {
                  pageNum = safePage - 3 + i;
                }
                return (
                  <button
                    key={pageNum}
                    onClick={() => setPage(pageNum)}
                    className={`px-1.5 py-0.5 text-[10px] rounded transition-colors ${
                      pageNum === safePage
                        ? 'bg-cyan-500/20 text-cyan-400'
                        : 'text-gray-500 hover:text-gray-300 hover:bg-gray-700/30'
                    }`}
                  >
                    {pageNum + 1}
                  </button>
                );
              })}
              <button
                onClick={() => setPage(Math.min(totalPages - 1, safePage + 1))}
                disabled={safePage >= totalPages - 1}
                className="px-2 py-0.5 text-[10px] rounded text-gray-400 hover:text-gray-200 hover:bg-gray-700/50 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
              >
                Next
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
