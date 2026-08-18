import { useEffect, useState, type ReactNode } from 'react';

/**
 * The list-page status strip (status-footer/v3 pick, v1 scope): four zones —
 * live/paused toggle · status pills · freshness · refresh. Pages own the
 * polling (usePolling's `enabled` flag is the pause switch); this component
 * owns the presentation and the r/p keyboard actions from the pick's
 * keymap (bare keys, inert while typing — same discipline as the g-keys).
 *
 * Deliberately NOT in v1 (recorded): per-pill drill-down drawers, the
 * inline trend sparkline, error-state acknowledgement.
 */
interface StatusFooterProps {
  paused: boolean;
  onPauseToggle: () => void;
  onRefresh: () => void;
  /** Epoch ms of the last successful data load; null before first load. */
  lastUpdatedAt: number | null;
  intervalMs: number;
  /** Status pill summary slot (counts, severity chips). */
  pills?: ReactNode;
}

function agoLabel(lastUpdatedAt: number | null, now: number): string {
  if (lastUpdatedAt === null) return '—';
  const secs = Math.max(0, Math.floor((now - lastUpdatedAt) / 1000));
  if (secs < 5) return 'just now';
  if (secs < 60) return `${secs}s ago`;
  return `${Math.floor(secs / 60)}m ago`;
}

export function StatusFooter({
  paused,
  onPauseToggle,
  onRefresh,
  lastUpdatedAt,
  intervalMs,
  pills,
}: StatusFooterProps) {
  // Tick so the "Xs ago" label moves without new data.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 5000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
      if ((e.target as HTMLElement)?.isContentEditable) return;
      if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
      if (e.key === 'r') {
        e.preventDefault();
        onRefresh();
      } else if (e.key === 'p') {
        e.preventDefault();
        onPauseToggle();
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [onRefresh, onPauseToggle]);

  return (
    <div className="flex items-center justify-between gap-4 mt-4 px-3 py-2 border border-gray-800 bg-[var(--bg-surface)] text-xs">
      <div className="flex items-center gap-3 min-w-0">
        <button
          onClick={onPauseToggle}
          title={`${paused ? 'Resume' : 'Pause'} auto-refresh (p)`}
          className={`flex items-center gap-1.5 px-2 py-0.5 border transition-colors ${
            paused
              ? 'border-yellow-500/40 text-yellow-400'
              : 'border-gray-700 text-gray-300 hover:border-gray-600'
          }`}
        >
          <span
            aria-hidden="true"
            className={`inline-block w-1.5 h-1.5 rounded-full ${paused ? 'bg-yellow-400' : 'bg-green-400 motion-safe:animate-pulse'}`}
          />
          {paused ? 'PAUSED' : 'LIVE'}
        </button>
        {pills && <div className="flex items-center gap-2 min-w-0 truncate">{pills}</div>}
      </div>
      <div className="flex items-center gap-3 text-faint whitespace-nowrap">
        <span>
          updated {agoLabel(lastUpdatedAt, now)}
          {!paused && ` · every ${Math.round(intervalMs / 1000)}s`}
        </span>
        <button
          onClick={onRefresh}
          title="Refresh now (r)"
          className="text-gray-400 hover:text-cyan-400 transition-colors"
        >
          ↻ refresh
        </button>
      </div>
    </div>
  );
}
