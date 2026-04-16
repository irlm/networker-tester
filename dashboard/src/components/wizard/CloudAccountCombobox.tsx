import { useState, useRef, useEffect, useMemo, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import type { CloudAccountSummary } from '../../api/types';

// ── Provider dot ─────────────────────────────────────────────────────────

const PROVIDER_DOT: Record<string, string> = {
  azure: 'bg-[#0078d4]',
  aws:   'bg-[#ff9900]',
  gcp:   'bg-[#4285f4]',
};

function dotClass(provider: string): string {
  return PROVIDER_DOT[provider.toLowerCase()] ?? 'bg-gray-500';
}

function statusDotClass(status: string): string {
  if (status === 'active') return 'bg-green-400';
  if (status === 'error') return 'bg-red-400';
  return 'bg-amber-400';
}

// ── Highlight matching substrings (case-insensitive) ────────────────────

function HighlightedText({ text, query }: { text: string; query: string }) {
  if (!query) return <>{text}</>;
  const lower = text.toLowerCase();
  const q = query.toLowerCase();
  const idx = lower.indexOf(q);
  if (idx === -1) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <span className="bg-cyan-500/20 text-cyan-300">{text.slice(idx, idx + query.length)}</span>
      {text.slice(idx + query.length)}
    </>
  );
}

// ── Component ────────────────────────────────────────────────────────────

export interface CloudAccountComboboxProps {
  projectId: string;
  cloudAccounts: CloudAccountSummary[];
  selectedAccountId: string;
  onSelect: (account: CloudAccountSummary) => void;
}

export function CloudAccountCombobox({
  projectId,
  cloudAccounts,
  selectedAccountId,
  onSelect,
}: CloudAccountComboboxProps) {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [activeIdx, setActiveIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const selected = useMemo(
    () => cloudAccounts.find(a => a.account_id === selectedAccountId) ?? null,
    [cloudAccounts, selectedAccountId],
  );

  // Filter by name + provider + region (case-insensitive substring match).
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return cloudAccounts;
    return cloudAccounts.filter(a =>
      a.name.toLowerCase().includes(q) ||
      a.provider.toLowerCase().includes(q) ||
      (a.region_default ?? '').toLowerCase().includes(q),
    );
  }, [cloudAccounts, query]);

  // ── Close on outside click ─────────────────────────────────────────────
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
        setQuery('');
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  // ── Reset active index when filter changes ─────────────────────────────
  useEffect(() => { setActiveIdx(0); }, [query]);

  // ── Scroll active item into view ───────────────────────────────────────
  useEffect(() => {
    if (!open || !listRef.current) return;
    const el = listRef.current.querySelector<HTMLElement>(`[data-idx="${activeIdx}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  }, [activeIdx, open]);

  // ── Keyboard handlers ──────────────────────────────────────────────────
  const onKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!open) { setOpen(true); return; }
      setActiveIdx(i => Math.min(i + 1, Math.max(0, filtered.length - 1)));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActiveIdx(i => Math.max(i - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const target = filtered[activeIdx];
      if (target && target.status === 'active') {
        onSelect(target);
        setOpen(false);
        setQuery('');
        inputRef.current?.blur();
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      setOpen(false);
      setQuery('');
      inputRef.current?.blur();
    }
  }, [open, filtered, activeIdx, onSelect]);

  // ── Display text when collapsed ────────────────────────────────────────
  const placeholder = selected
    ? `${selected.name} · ${selected.provider.toUpperCase()} / ${selected.region_default ?? '—'}`
    : 'Search accounts… (try "azure" or "prod")';

  return (
    <div ref={wrapRef} className="relative" style={{ maxWidth: 420 }}>
      <div className="relative">
        {/* Selected provider dot pinned inside the input */}
        {selected && !open && (
          <span className={`absolute left-3 top-1/2 -translate-y-1/2 w-1.5 h-1.5 rounded-full ${dotClass(selected.provider)}`} aria-hidden="true" />
        )}
        <input
          ref={inputRef}
          type="text"
          value={open ? query : ''}
          placeholder={placeholder}
          onFocus={() => setOpen(true)}
          onChange={e => { setQuery(e.target.value); setOpen(true); }}
          onKeyDown={onKeyDown}
          className={`w-full bg-[var(--bg-base)] border ${
            open ? 'border-cyan-500/60' : selectedAccountId ? 'border-gray-700' : 'border-yellow-500/40'
          } px-3 py-1.5 text-xs font-mono text-gray-200 focus:outline-none placeholder:text-gray-500 ${
            selected && !open ? 'pl-7' : ''
          }`}
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls="cloud-account-listbox"
        />
        <span className="absolute right-3 top-1/2 -translate-y-1/2 text-[10px] text-gray-500 pointer-events-none">
          {open ? '⌄' : '/'}
        </span>
      </div>

      {open && (
        <div
          ref={listRef}
          id="cloud-account-listbox"
          role="listbox"
          className="absolute z-20 left-0 right-0 mt-1 border border-gray-700 bg-[var(--bg-surface)] max-h-60 overflow-y-auto"
        >
          {filtered.length === 0 ? (
            <div className="px-3 py-3 text-xs text-gray-500 font-mono">No accounts match "{query}"</div>
          ) : (
            filtered.map((acct, idx) => {
              const isActive = idx === activeIdx;
              const isDisabled = acct.status !== 'active';
              return (
                <div
                  key={acct.account_id}
                  data-idx={idx}
                  role="option"
                  aria-selected={isActive}
                  onMouseEnter={() => setActiveIdx(idx)}
                  onMouseDown={e => {
                    e.preventDefault();
                    if (isDisabled) return;
                    onSelect(acct);
                    setOpen(false);
                    setQuery('');
                    inputRef.current?.blur();
                  }}
                  className={`flex items-center gap-2.5 px-3 py-2 text-xs font-mono border-b border-gray-800/60 last:border-b-0 ${
                    isActive ? 'bg-cyan-500/10 text-gray-100' : 'text-gray-300'
                  } ${isDisabled ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'} ${
                    isActive ? 'shadow-[inset_2px_0_0_#22d3ee]' : ''
                  }`}
                >
                  <span className={`w-1.5 h-1.5 rounded-full ${dotClass(acct.provider)}`} aria-hidden="true" />
                  <span className="flex-1 truncate">
                    <HighlightedText text={acct.name} query={query} />
                    <span className="text-gray-600"> · </span>
                    <HighlightedText text={acct.provider.toUpperCase()} query={query} />
                  </span>
                  <span className="text-gray-500 text-[11px]">
                    <HighlightedText text={acct.region_default ?? '—'} query={query} />
                  </span>
                  <span className={`w-1.5 h-1.5 rounded-full ${statusDotClass(acct.status)}`} title={acct.status} aria-hidden="true" />
                </div>
              );
            })
          )}

          {/* Footer: add account */}
          <div
            onMouseDown={e => {
              e.preventDefault();
              setOpen(false);
              navigate(`/projects/${projectId}/cloud-accounts`);
            }}
            className="flex items-center gap-2 px-3 py-2 text-xs font-mono text-cyan-400 border-t border-gray-800 hover:bg-cyan-500/5 cursor-pointer"
          >
            + add cloud account…
          </div>
        </div>
      )}

      {/* Keyboard hints */}
      {open && (
        <div className="mt-1 flex gap-3 text-[10px] text-gray-600 font-mono">
          <span><kbd className="px-1 py-0.5 border border-gray-700 rounded">↑↓</kbd> navigate</span>
          <span><kbd className="px-1 py-0.5 border border-gray-700 rounded">↵</kbd> select</span>
          <span><kbd className="px-1 py-0.5 border border-gray-700 rounded">esc</kbd> close</span>
        </div>
      )}
    </div>
  );
}
