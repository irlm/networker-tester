import { useState, useRef, useEffect, useCallback, useMemo } from 'react';

export interface ComboboxOption {
  value: string;
  label: string;
  /** Secondary text shown to the right */
  detail?: string;
  /** Group header this option belongs to */
  group?: string;
}

interface ComboboxProps {
  /** Current selected value (empty string = nothing selected) */
  value: string;
  onChange: (value: string) => void;
  options: ComboboxOption[];
  placeholder?: string;
  /** Label for screen readers */
  ariaLabel?: string;
  /** Allow clearing the selection */
  clearable?: boolean;
  /** CSS class for the outer wrapper */
  className?: string;
  /** Disable the input */
  disabled?: boolean;
  /** Show a loading spinner in the dropdown */
  loading?: boolean;
  /** Compact size variant */
  compact?: boolean;
}

/** Match score: higher = better match. Returns 0 for no match. */
function fuzzyScore(text: string, query: string): number {
  const lower = text.toLowerCase();
  const q = query.toLowerCase();
  // Exact match
  if (lower === q) return 100;
  // Starts with query
  if (lower.startsWith(q)) return 80;
  // Word-start match (e.g. "east" matches "tester-eastus")
  const words = lower.split(/[\s\-_.]+/);
  if (words.some(w => w.startsWith(q))) return 60;
  // Contains substring
  if (lower.includes(q)) return 40;
  return 0;
}

/** Max items shown in dropdown to avoid rendering performance issues */
const MAX_VISIBLE = 50;

export function Combobox({
  value,
  onChange,
  options,
  placeholder = 'Select...',
  ariaLabel,
  clearable = true,
  className = '',
  disabled = false,
  loading = false,
  compact = false,
}: ComboboxProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);

  const selectedOption = useMemo(
    () => options.find(o => o.value === value),
    [options, value],
  );

  const filtered = useMemo(() => {
    if (!query.trim()) return options.slice(0, MAX_VISIBLE);
    // Score each option, filter out non-matches, sort by best match first
    return options
      .map(o => ({
        option: o,
        score: Math.max(
          fuzzyScore(o.label, query),
          o.detail ? fuzzyScore(o.detail, query) : 0,
        ),
      }))
      .filter(x => x.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, MAX_VISIBLE)
      .map(x => x.option);
  }, [options, query]);

  // Group the filtered results
  const grouped = useMemo(() => {
    const groups: { group: string; items: ComboboxOption[] }[] = [];
    const seen = new Set<string>();
    for (const opt of filtered) {
      const g = opt.group || '';
      if (!seen.has(g)) {
        seen.add(g);
        groups.push({ group: g, items: [] });
      }
      groups.find(x => x.group === g)!.items.push(opt);
    }
    return groups;
  }, [filtered]);

  // Flat list for keyboard nav + O(1) index lookup
  const flatFiltered = useMemo(() => grouped.flatMap(g => g.items), [grouped]);
  const flatIndexMap = useMemo(
    () => new Map(flatFiltered.map((item, idx) => [item.value, idx])),
    [flatFiltered],
  );

  // Close on outside click
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (wrapperRef.current && !wrapperRef.current.contains(e.target as Node)) {
        setOpen(false);
        setQuery('');
      }
    }
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, []);

  // Scroll active item into view
  useEffect(() => {
    if (activeIndex >= 0 && listRef.current) {
      const el = listRef.current.children[activeIndex] as HTMLElement | undefined;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [activeIndex]);

  const select = useCallback(
    (val: string) => {
      onChange(val);
      setOpen(false);
      setQuery('');
      setActiveIndex(-1);
    },
    [onChange],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (!open) {
          setOpen(true);
          setActiveIndex(0);
        } else {
          setActiveIndex(i => Math.min(i + 1, flatFiltered.length - 1));
        }
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActiveIndex(i => Math.max(i - 1, 0));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (open && activeIndex >= 0 && flatFiltered[activeIndex]) {
          select(flatFiltered[activeIndex].value);
        } else if (!open) {
          setOpen(true);
        }
      } else if (e.key === 'Escape') {
        setOpen(false);
        setQuery('');
        setActiveIndex(-1);
      }
    },
    [open, activeIndex, flatFiltered, select],
  );

  const py = compact ? 'py-1' : 'py-1.5';

  return (
    <div ref={wrapperRef} className={`relative ${className}`}>
      <div className="relative">
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-expanded={open}
          aria-label={ariaLabel || placeholder}
          aria-autocomplete="list"
          aria-activedescendant={activeIndex >= 0 ? `cb-opt-${activeIndex}` : undefined}
          value={open ? query : selectedOption?.label || ''}
          placeholder={placeholder}
          disabled={disabled}
          className={`w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 ${py} text-sm text-gray-300 pr-8 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600 disabled:opacity-50 disabled:cursor-not-allowed font-mono`}
          onFocus={() => {
            setOpen(true);
            setQuery('');
            setActiveIndex(-1);
          }}
          onChange={e => {
            setQuery(e.target.value);
            setActiveIndex(0);
            if (!open) setOpen(true);
          }}
          onKeyDown={handleKeyDown}
        />
        {/* Chevron / clear button */}
        {clearable && value ? (
          <button
            type="button"
            tabIndex={-1}
            className="absolute right-1.5 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300 p-0.5"
            onClick={e => {
              e.stopPropagation();
              onChange('');
              setQuery('');
              inputRef.current?.focus();
            }}
            aria-label="Clear selection"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.5">
              <path d="M4 4l6 6M10 4l-6 6" />
            </svg>
          </button>
        ) : (
          <span className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-600 pointer-events-none text-xs">
            {loading ? '...' : '\u25BE'}
          </span>
        )}
      </div>

      {open && (
        <ul
          ref={listRef}
          role="listbox"
          className="absolute z-50 mt-1 w-full max-h-60 overflow-auto bg-[var(--bg-surface,#1a1a2e)] border border-gray-700 rounded shadow-lg"
        >
          {loading && (
            <li className="px-3 py-2 text-xs text-gray-500 motion-safe:animate-pulse">Loading...</li>
          )}
          {!loading && flatFiltered.length === 0 && (
            <li className="px-3 py-2 text-xs text-gray-500">No matches</li>
          )}
          {!loading &&
            grouped.map(g => (
              <li key={g.group} role="presentation">
                {g.group && (
                  <div className="px-3 pt-2 pb-1 text-[10px] uppercase tracking-wider text-gray-500 font-medium">
                    {g.group}
                  </div>
                )}
                <ul role="group">
                  {g.items.map(opt => {
                    const flatIdx = flatIndexMap.get(opt.value) ?? -1;
                    const isActive = flatIdx === activeIndex;
                    const isSelected = opt.value === value;
                    return (
                      <li
                        key={opt.value}
                        id={`cb-opt-${flatIdx}`}
                        role="option"
                        aria-selected={isSelected}
                        className={`px-3 py-1.5 text-sm cursor-pointer flex items-center justify-between gap-2 ${
                          isActive
                            ? 'bg-cyan-500/15 text-gray-100'
                            : isSelected
                              ? 'text-cyan-400'
                              : 'text-gray-300 hover:bg-gray-700/40'
                        }`}
                        onMouseEnter={() => setActiveIndex(flatIdx)}
                        onMouseDown={e => {
                          e.preventDefault();
                          select(opt.value);
                        }}
                      >
                        <span className="truncate font-mono">{opt.label}</span>
                        {opt.detail && (
                          <span className="text-xs text-gray-500 flex-shrink-0">{opt.detail}</span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </li>
            ))}
        </ul>
      )}
    </div>
  );
}
