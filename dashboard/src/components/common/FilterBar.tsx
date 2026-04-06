import type { ReactNode } from 'react';

interface FilterChipProps {
  label: string;
  value: string;
  onClear: () => void;
}

export function FilterChip({ label, value, onClear }: FilterChipProps) {
  return (
    <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-cyan-500/10 border border-cyan-500/20 text-xs text-cyan-400">
      <span className="text-gray-500">{label}:</span>
      <span className="font-mono">{value}</span>
      <button
        type="button"
        onClick={onClear}
        className="ml-0.5 text-cyan-500/60 hover:text-cyan-300"
        aria-label={`Clear ${label} filter`}
      >
        &times;
      </button>
    </span>
  );
}

interface ScopeChipProps {
  label: string;
}

/** Non-dismissible chip showing viewer scoping */
export function ScopeChip({ label }: ScopeChipProps) {
  return (
    <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-purple-500/10 border border-purple-500/20 text-xs text-purple-400">
      <span className="w-1.5 h-1.5 rounded-full bg-purple-400" />
      {label}
    </span>
  );
}

interface FilterBarProps {
  children: ReactNode;
  /** Active filter chips to display below the filter controls */
  chips?: ReactNode;
  /** Count of active filters */
  activeCount?: number;
  /** Called when "Clear all" is clicked */
  onClearAll?: () => void;
}

export function FilterBar({ children, chips, activeCount = 0, onClearAll }: FilterBarProps) {
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">{children}</div>
      {(chips || activeCount > 0) && (
        <div className="flex flex-wrap items-center gap-1.5">
          {chips}
          {activeCount > 1 && onClearAll && (
            <button
              type="button"
              onClick={onClearAll}
              className="text-[11px] text-gray-500 hover:text-gray-300 ml-1"
            >
              Clear all
            </button>
          )}
        </div>
      )}
    </div>
  );
}
