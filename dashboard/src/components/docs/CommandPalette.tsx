import { useState, useEffect, useMemo, useRef, useCallback } from 'react';
import { useNavigate } from 'react-router';
import { useDocsStore } from '../../stores/docsStore';
import { useAuthStore } from '../../stores/authStore';
import { useProjectStore } from '../../stores/projectStore';
import { DOC_ENTRIES, DOC_CATEGORIES, type DocEntry } from '../../lib/docs/content';
import { searchDocs } from '../../lib/docs/search';
import { DocEntryView } from './DocEntryView';
import {
  NAV_ENTRIES,
  PALETTE_EXTRA_ENTRIES,
  navEntryVisible,
  resolveNavPath,
  type NavEntry,
} from '../layout/nav-config';

/** A palette row: a page to jump to, or a manual entry to read. */
type PaletteResult =
  | { kind: 'nav'; nav: NavEntry; path: string }
  | { kind: 'doc'; doc: DocEntry };

/**
 * Rank pages against the query: label prefix > label substring > keyword
 * substring. Empty query surfaces the sidebar pages in nav order — the
 * palette then doubles as a keyboard-only sidebar.
 */
function searchNav(
  query: string,
  ctx: { pid: string | null; isAdmin: boolean; isPlatformAdmin: boolean },
): PaletteResult[] {
  const q = query.trim().toLowerCase();
  const candidates = [...NAV_ENTRIES, ...PALETTE_EXTRA_ENTRIES]
    .filter(entry => navEntryVisible(entry, ctx));

  const scored: Array<{ score: number; entry: NavEntry }> = [];
  for (const entry of candidates) {
    const label = entry.label.toLowerCase();
    let score = 0;
    if (!q) {
      score = 1;
    } else if (label.startsWith(q)) {
      score = 3;
    } else if (label.includes(q)) {
      score = 2;
    } else if (entry.keywords?.some(k => k.toLowerCase().includes(q))) {
      score = 1;
    }
    if (score > 0) scored.push({ score, entry });
  }
  scored.sort((a, b) => b.score - a.score);

  const limit = q ? 6 : 8;
  return scored.slice(0, limit).flatMap(({ entry }) => {
    const path = resolveNavPath(entry.path, ctx.pid);
    return path ? [{ kind: 'nav' as const, nav: entry, path }] : [];
  });
}

export default function CommandPalette() {
  const { closePalette } = useDocsStore();
  const navigate = useNavigate();
  const isPlatformAdmin = useAuthStore(s => s.isPlatformAdmin);
  const authRole = useAuthStore(s => s.role);
  const activeProjectId = useProjectStore(s => s.activeProjectId);
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [detailEntry, setDetailEntry] = useState<DocEntry | null>(null);
  const [isInsertMode, setIsInsertMode] = useState(true);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  const isKeyboardNav = useRef(false);
  // Track pending `g` for gg (go-to-top) vim sequence
  const pendingG = useRef<number | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const gateCtx = useMemo(() => ({
    pid: activeProjectId ?? null,
    isAdmin: authRole === 'admin' || isPlatformAdmin,
    isPlatformAdmin,
  }), [activeProjectId, authRole, isPlatformAdmin]);
  const navResults = useMemo(() => searchNav(query, gateCtx), [query, gateCtx]);
  const visibleResults: PaletteResult[] = useMemo(() => {
    const docResults = searchDocs(DOC_ENTRIES, query);
    const maxVisible = 20;
    return [
      ...navResults,
      ...docResults.slice(0, maxVisible - navResults.length).map(doc => ({ kind: 'doc' as const, doc })),
    ];
  }, [query, navResults]);

  const activate = useCallback((result: PaletteResult) => {
    if (result.kind === 'nav') {
      closePalette();
      navigate(result.path);
    } else {
      setDetailEntry(result.doc);
    }
  }, [closePalette, navigate]);

  useEffect(() => {
    if (!listRef.current) return;
    const items = listRef.current.querySelectorAll('[data-palette-item]');
    items[selectedIndex]?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex]);

  const goBack = useCallback(() => {
    setDetailEntry(null);
    setTimeout(() => {
      inputRef.current?.focus();
      setIsInsertMode(true);
    }, 0);
  }, []);

  const enterNormalMode = useCallback(() => {
    inputRef.current?.blur();
    setIsInsertMode(false);
  }, []);

  const enterInsertMode = useCallback(() => {
    inputRef.current?.focus();
    setIsInsertMode(true);
  }, []);

  const handleInputKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (detailEntry) {
          goBack();
        } else if (query) {
          e.preventDefault();
          setQuery('');
          setSelectedIndex(0);
          setDetailEntry(null);
        } else {
          e.preventDefault();
          enterNormalMode();
        }
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        enterNormalMode();
        return;
      }

      if (detailEntry) return;

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, visibleResults.length - 1));
        isKeyboardNav.current = true;
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
        isKeyboardNav.current = true;
      } else if (e.key === 'Enter' && visibleResults[selectedIndex]) {
        e.preventDefault();
        const result = visibleResults[selectedIndex];
        activate(result);
        if (result.kind === 'doc') enterNormalMode();
      }
    },
    [activate, detailEntry, enterNormalMode, goBack, query, visibleResults, selectedIndex],
  );

  useEffect(() => {
    function handler(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;

      if (e.key === 'q' || e.key === 'Escape') {
        e.preventDefault();
        if (detailEntry) {
          goBack();
        } else {
          closePalette();
        }
        return;
      }

      if (e.key === 'Tab') {
        e.preventDefault();
        enterInsertMode();
        return;
      }

      // gg sequence: first g sets pending, second g within 500ms executes
      if (e.key === 'g') {
        e.preventDefault();
        const now = Date.now();
        if (pendingG.current && now - pendingG.current < 500) {
          pendingG.current = null;
          if (detailEntry) {
            detailRef.current?.scrollTo({ top: 0 });
          } else {
            setSelectedIndex(0);
            isKeyboardNav.current = true;
          }
        } else {
          pendingG.current = now;
        }
        return;
      }
      // Any other key cancels pending g
      pendingG.current = null;

      if (detailEntry) {
        if (e.key === 'j') {
          detailRef.current?.scrollBy({ top: 60 });
        } else if (e.key === 'k') {
          detailRef.current?.scrollBy({ top: -60 });
        } else if (e.key === 'G') {
          detailRef.current?.scrollTo({ top: detailRef.current.scrollHeight });
        } else if (e.key === '/' || e.key === 'i') {
          e.preventDefault();
          goBack();
        }
        return;
      }

      isKeyboardNav.current = true;

      if (e.key === 'j') {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, visibleResults.length - 1));
      } else if (e.key === 'k') {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === 'G') {
        e.preventDefault();
        setSelectedIndex(visibleResults.length - 1);
      } else if (e.key === 'Enter' && visibleResults[selectedIndex]) {
        e.preventDefault();
        activate(visibleResults[selectedIndex]);
      } else if (e.key === '/' || e.key === 'i') {
        e.preventDefault();
        enterInsertMode();
      }
    }

    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [activate, closePalette, detailEntry, enterInsertMode, goBack, visibleResults, selectedIndex]);

  const modeLabel = detailEntry ? 'DETAIL' : isInsertMode ? 'INSERT' : 'NORMAL';
  // Prompt changes with mode: > for INSERT (typing), : for NORMAL (commands)
  const prompt = isInsertMode ? '>' : ':';

  return (
    <div className="fixed inset-0 z-50 flex justify-center pt-[15vh]">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/50 docs-backdrop-enter"
        onClick={closePalette}
        aria-hidden="true"
      />

      {/* Modal */}
      <div
        className="docs-panel-enter relative w-full max-w-xl mx-4 flex flex-col bg-[var(--bg-surface)] border border-[var(--border-default)] overflow-hidden"
        style={{ maxHeight: '60vh' }}
      >
        {/* Input row */}
        <div className="flex items-center border-b border-[var(--border-default)] px-3">
          <span className={`text-sm mr-2 select-none transition-colors duration-100 ${isInsertMode ? 'text-cyan-400' : 'text-[#863bff]'}`}>
            {prompt}
          </span>
          {/* Blinking block cursor shown in NORMAL mode when input is empty */}
          {!isInsertMode && !query && (
            <span className="vim-cursor text-cyan-400 text-sm mr-1">{'\u2588'}</span>
          )}
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => { setQuery(e.target.value); setSelectedIndex(0); setDetailEntry(null); }}
            onKeyDown={handleInputKeyDown}
            onFocus={() => setIsInsertMode(true)}
            onBlur={() => setIsInsertMode(false)}
            placeholder={isInsertMode ? 'Jump to a page or search docs... (runs, p95, man man)' : ''}
            className="flex-1 bg-transparent py-2.5 text-sm text-gray-200 placeholder-gray-600 outline-none"
            style={{ caretColor: '#22d3ee' }}
            spellCheck={false}
            autoComplete="off"
          />
          <span className={`text-[10px] ml-2 whitespace-nowrap font-medium transition-colors duration-100 ${
            isInsertMode ? 'text-cyan-500/50' : detailEntry ? 'text-[#863bff]/60' : 'text-gray-500'
          }`}>
            -- {modeLabel} --
          </span>
        </div>

        {/* Detail view */}
        {detailEntry ? (
          <div ref={detailRef} className="flex-1 overflow-y-auto p-4">
            <button
              onClick={goBack}
              className="text-xs text-gray-400 hover:text-gray-300 mb-3 flex items-center gap-1"
            >
              <span className="text-gray-500">&larr;</span> back
              <span className="text-gray-700 ml-1">q</span>
            </button>
            <DocEntryView entry={detailEntry} />
          </div>
        ) : (
          /* Results list */
          <div ref={listRef} className="flex-1 overflow-y-auto">
            {visibleResults.length === 0 && query ? (
              <div className="py-8 px-4 text-center">
                <div className="text-gray-500 text-sm">
                  No manual entry for &ldquo;{query}&rdquo;
                </div>
                <div className="text-gray-700 text-xs mt-1">
                  Try: p95, throughput, benchmark, or man man
                </div>
              </div>
            ) : (
              visibleResults.map((result, i) => {
                const isSelected = i === selectedIndex;
                const rowClass = `w-full text-left px-4 py-2 flex items-center gap-3 text-sm transition-colors duration-75 ${
                  isSelected
                    ? 'bg-gray-800/60 text-gray-100 border-l-2 border-cyan-500'
                    : 'text-gray-400 hover:bg-gray-800/20 border-l-2 border-transparent'
                }`;
                const rowProps = {
                  'data-palette-item': true,
                  onMouseMove: () => { if (!isKeyboardNav.current) setSelectedIndex(i); },
                  onMouseDown: () => { isKeyboardNav.current = false; },
                  className: rowClass,
                };
                if (result.kind === 'nav') {
                  const gKey = result.nav.gKey;
                  return (
                    <button key={`nav:${result.path}`} {...rowProps} onClick={() => activate(result)}>
                      <span className="text-[10px] uppercase tracking-wider text-gray-500 w-16 flex-shrink-0">
                        go to
                      </span>
                      <span className="text-gray-600 flex-shrink-0" aria-hidden="true">{result.nav.icon}</span>
                      <span className="text-cyan-400 flex-shrink-0">{result.nav.label}</span>
                      {gKey && (
                        <span className="text-gray-600 text-xs ml-auto flex-shrink-0">g {gKey}</span>
                      )}
                    </button>
                  );
                }
                const entry = result.doc;
                const cat = DOC_CATEGORIES.find((c) => c.id === entry.category);
                return (
                  <button key={entry.id} {...rowProps} onClick={() => activate(result)}>
                    {cat && (
                      <span className="text-[10px] uppercase tracking-wider text-[#863bff] w-16 flex-shrink-0">
                        {cat.label}
                      </span>
                    )}
                    <span className="text-cyan-400 flex-shrink-0">{entry.title}</span>
                    <span className="text-gray-500 text-xs truncate">{entry.brief}</span>
                  </button>
                );
              })
            )}
          </div>
        )}

        {/* Status bar */}
        {!detailEntry && (
          <div className="flex items-center justify-between border-t border-[var(--border-default)] px-3 py-1 text-[10px] text-gray-500">
            <span>
              {query
                ? `${visibleResults.length} result${visibleResults.length !== 1 ? 's' : ''}`
                : `${navResults.length} pages · ${DOC_ENTRIES.length} manual entries`}
            </span>
            <span>
              {visibleResults.length > 0 && `${selectedIndex + 1}/${visibleResults.length}`}
              {!isInsertMode && <span className="ml-2">j/k navigate</span>}
              {isInsertMode && <span className="ml-2">Tab for vim keys</span>}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
