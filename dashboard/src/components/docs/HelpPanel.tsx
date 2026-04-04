import { useState, useEffect, useRef, useCallback } from 'react';
import { useDocsStore } from '../../stores/docsStore';
import { DOC_ENTRIES, DOC_CATEGORIES, type DocCategory } from '../../lib/docs/content';
import { searchDocs, filterByCategory } from '../../lib/docs/search';
import { DocEntryView } from './DocEntryView';

const CATEGORY_KEYS = ['1', '2', '3', '4', '5'];

export default function HelpPanel() {
  const { closeHelp } = useDocsStore();
  const [query, setQuery] = useState('');
  const [activeCategory, setActiveCategory] = useState<DocCategory | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [isInsertMode, setIsInsertMode] = useState(true);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const isKeyboardNav = useRef(false);
  const pendingG = useRef<number | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const filtered = searchDocs(filterByCategory(DOC_ENTRIES, activeCategory), query);

  // Scroll selected item into view
  useEffect(() => {
    if (!listRef.current) return;
    const items = listRef.current.querySelectorAll('[data-help-item]');
    items[selectedIndex]?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex]);

  const toggleExpand = useCallback((id: string) => {
    setExpandedId((prev) => (prev === id ? null : id));
  }, []);

  const setCategory = useCallback((cat: DocCategory | null) => {
    setActiveCategory(cat);
    setSelectedIndex(0);
    setExpandedId(null);
  }, []);

  const enterNormalMode = useCallback(() => {
    inputRef.current?.blur();
    setIsInsertMode(false);
  }, []);

  const enterInsertMode = useCallback(() => {
    inputRef.current?.focus();
    setIsInsertMode(true);
  }, []);

  // Document-level key handler (NORMAL mode)
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;

      if (e.key === 'q' || e.key === 'Escape') {
        e.preventDefault();
        closeHelp();
        return;
      }

      if (e.key === 'Tab') {
        e.preventDefault();
        enterInsertMode();
        return;
      }

      // Number keys for category selection: 0=All, 1-5=categories
      if (e.key === '0') {
        e.preventDefault();
        setCategory(null);
        return;
      }
      const catIdx = CATEGORY_KEYS.indexOf(e.key);
      if (catIdx !== -1 && catIdx < DOC_CATEGORIES.length) {
        e.preventDefault();
        const cat = DOC_CATEGORIES[catIdx];
        setCategory(activeCategory === cat.id ? null : cat.id);
        return;
      }

      // gg sequence: first g sets pending, second g within 500ms executes
      if (e.key === 'g') {
        e.preventDefault();
        const now = Date.now();
        if (pendingG.current && now - pendingG.current < 500) {
          pendingG.current = null;
          setSelectedIndex(0);
          isKeyboardNav.current = true;
        } else {
          pendingG.current = now;
        }
        return;
      }
      pendingG.current = null;

      isKeyboardNav.current = true;

      if (e.key === 'j') {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
      } else if (e.key === 'k') {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === 'G') {
        e.preventDefault();
        setSelectedIndex(filtered.length - 1);
      } else if (e.key === 'Enter' || e.key === 'l') {
        e.preventDefault();
        if (filtered[selectedIndex]) {
          toggleExpand(filtered[selectedIndex].id);
        }
      } else if (e.key === 'h') {
        e.preventDefault();
        setExpandedId(null);
      } else if (e.key === '/' || e.key === 'i') {
        e.preventDefault();
        enterInsertMode();
      }
    }

    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [activeCategory, closeHelp, enterInsertMode, filtered, selectedIndex, setCategory, toggleExpand]);

  // Input-level key handler (INSERT mode)
  const handleInputKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        enterNormalMode();
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        enterNormalMode();
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
        isKeyboardNav.current = true;
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
        isKeyboardNav.current = true;
      } else if (e.key === 'Enter' && filtered[selectedIndex]) {
        e.preventDefault();
        toggleExpand(filtered[selectedIndex].id);
        enterNormalMode();
      }
    },
    [enterNormalMode, filtered, selectedIndex, toggleExpand],
  );

  const activeCategoryLabel = activeCategory
    ? DOC_CATEGORIES.find((c) => c.id === activeCategory)?.label ?? 'All'
    : 'All';

  return (
    <div className="fixed inset-0 z-50 flex">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/60 docs-backdrop-enter"
        onClick={closeHelp}
        aria-hidden="true"
      />

      {/* Panel — no border-radius per zero-chrome principle */}
      <div className="docs-panel-enter relative mx-auto my-8 flex w-full max-w-4xl flex-col overflow-hidden border border-[var(--border-default)] bg-[var(--bg-surface)]">
        {/* Header */}
        <div className="flex items-center gap-3 border-b border-[var(--border-default)] px-4 py-2.5">
          <span className="text-cyan-400 text-sm font-medium">? docs</span>
          <input
            ref={inputRef}
            tabIndex={1}
            type="text"
            value={query}
            onChange={(e) => { setQuery(e.target.value); setSelectedIndex(0); }}
            onKeyDown={handleInputKeyDown}
            onFocus={() => setIsInsertMode(true)}
            onBlur={() => setIsInsertMode(false)}
            placeholder="Search... (p95, throughput, benchmark phases)"
            className="flex-1 bg-[var(--bg-base)] border border-[var(--border-default)] px-3 py-1.5 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-cyan-500/50"
          />
          <span className={`text-[10px] font-medium whitespace-nowrap transition-colors duration-100 ${
            isInsertMode ? 'text-cyan-500/50' : 'text-gray-600'
          }`}>
            -- {isInsertMode ? 'INSERT' : 'NORMAL'} --
          </span>
        </div>

        <div className="flex flex-1 overflow-hidden">
          {/* Category sidebar */}
          <div className="hidden md:flex w-40 flex-col border-r border-[var(--border-default)] p-2 overflow-y-auto">
            <button
              onClick={() => setCategory(null)}
              className={`text-left px-2 py-1.5 text-xs mb-0.5 flex items-center gap-2 ${
                activeCategory === null
                  ? 'bg-gray-800/60 text-gray-100 border-l-2 border-cyan-500 -ml-0.5 pl-1.5'
                  : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/20 border-l-2 border-transparent -ml-0.5 pl-1.5'
              }`}
            >
              <span className="text-gray-600 text-[10px] w-3">0</span>
              <span>All</span>
              <span className="text-gray-600 ml-auto">{DOC_ENTRIES.length}</span>
            </button>
            {DOC_CATEGORIES.map((cat, idx) => {
              const count = DOC_ENTRIES.filter((e) => e.category === cat.id).length;
              const isActive = activeCategory === cat.id;
              return (
                <button
                  key={cat.id}
                  onClick={() => setCategory(isActive ? null : cat.id)}
                  className={`text-left px-2 py-1.5 text-xs mb-0.5 flex items-center gap-2 ${
                    isActive
                      ? 'bg-gray-800/60 text-gray-100 border-l-2 border-cyan-500 -ml-0.5 pl-1.5'
                      : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/20 border-l-2 border-transparent -ml-0.5 pl-1.5'
                  }`}
                >
                  <span className="text-gray-600 text-[10px] w-3">{idx + 1}</span>
                  <span aria-hidden="true">{cat.icon}</span>
                  <span>{cat.label}</span>
                  <span className="text-gray-600 ml-auto">{count}</span>
                </button>
              );
            })}

            <div className="mt-auto pt-3 border-t border-[var(--border-default)] mt-3">
              <div className="px-2 text-[10px] text-gray-600 leading-relaxed">
                <kbd className="text-gray-500">j</kbd>/<kbd className="text-gray-500">k</kbd> navigate
                {' '}<kbd className="text-gray-500">l</kbd> expand
                <br />
                <kbd className="text-gray-500">0-5</kbd> category
                {' '}<kbd className="text-gray-500">q</kbd> close
                <br />
                <kbd className="text-gray-500">/</kbd> search
                {' '}<kbd className="text-gray-500">Tab</kbd> toggle mode
              </div>
            </div>
          </div>

          {/* Mobile category tabs */}
          <div className="md:hidden flex gap-1 px-2 py-2 border-b border-[var(--border-default)] overflow-x-auto flex-shrink-0">
            <button
              onClick={() => setCategory(null)}
              className={`px-2 py-1 text-[10px] whitespace-nowrap ${
                activeCategory === null ? 'bg-gray-800/60 text-gray-100' : 'text-gray-500'
              }`}
            >
              All
            </button>
            {DOC_CATEGORIES.map((cat) => (
              <button
                key={cat.id}
                onClick={() => setCategory(activeCategory === cat.id ? null : cat.id)}
                className={`px-2 py-1 text-[10px] whitespace-nowrap ${
                  activeCategory === cat.id ? 'bg-gray-800/60 text-gray-100' : 'text-gray-500'
                }`}
              >
                {cat.icon} {cat.label}
              </button>
            ))}
          </div>

          {/* Entry list */}
          <div ref={listRef} className="flex-1 overflow-y-auto p-3">
            {filtered.length === 0 ? (
              <div className="py-8 px-4 text-center">
                <div className="text-gray-600 text-sm">
                  No manual entry for &ldquo;{query}&rdquo;
                </div>
                {query && (
                  <button
                    onClick={() => { setQuery(''); setSelectedIndex(0); }}
                    className="mt-2 text-xs text-gray-500 hover:text-gray-300"
                  >
                    Clear search
                  </button>
                )}
              </div>
            ) : (
              <div className="space-y-0.5">
                {filtered.map((entry, i) => {
                  const isSelected = i === selectedIndex;
                  const isExpanded = expandedId === entry.id;
                  return (
                    <button
                      key={entry.id}
                      data-help-item
                      onClick={() => { setSelectedIndex(i); toggleExpand(entry.id); }}
                      onMouseMove={() => {
                        if (!isKeyboardNav.current) setSelectedIndex(i);
                      }}
                      onMouseDown={() => { isKeyboardNav.current = false; }}
                      className={`w-full text-left px-3 py-2 transition-colors duration-75 ${
                        isSelected
                          ? 'bg-gray-800/60 border-l-2 border-cyan-500'
                          : 'hover:bg-gray-800/20 border-l-2 border-transparent'
                      }`}
                    >
                      <DocEntryView
                        entry={entry}
                        compact={!isExpanded}
                        showCategory={!isExpanded}
                      />
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>

        {/* Status bar — vim-style */}
        <div className="flex items-center justify-between border-t border-[var(--border-default)] px-3 py-1 text-[10px] text-gray-600">
          <span>
            {activeCategoryLabel}
            {query && ` \u00b7 "${query}"`}
          </span>
          <span>
            {filtered.length > 0 && `${selectedIndex + 1}/${filtered.length}`}
            {filtered.length === 0 && 'No matches'}
          </span>
        </div>
      </div>
    </div>
  );
}
