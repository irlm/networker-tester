// ── ModeChip ─────────────────────────────────────────────────────────────
//
// Single-mode chip color-coded by family. Same color language across the
// endpoint runs list, preset cards, filter chips, and any future surface
// that lists test modes.
//
// Color mapping + familyOf live in ./mode-family.ts so this file stays
// component-only (required for react-refresh HMR).

import { familyOf, CHIP_CLASSES } from './mode-family';

export interface ModeChipProps {
  mode: string;
  /** Override label (default: mode itself). Useful for "+3" overflow chips. */
  label?: string;
}

export function ModeChip({ mode, label }: ModeChipProps) {
  const cls = CHIP_CLASSES[familyOf(mode)];
  return (
    <span
      className={`inline-flex items-center px-1.5 py-0.5 text-[9px] font-mono leading-tight border rounded-sm ${cls}`}
      title={mode}
    >
      {label ?? mode}
    </span>
  );
}

/** Render a list of mode chips. Truncates after `max` with a "+N" overflow. */
export function ModeChipList({ modes, max = 24 }: { modes: string[]; max?: number }) {
  if (modes.length === 0) {
    return <span className="text-[10px] text-gray-600 font-mono">no modes</span>;
  }
  const shown = modes.slice(0, max);
  const overflow = modes.length - shown.length;
  return (
    <span className="inline-flex flex-wrap gap-1 items-center">
      {shown.map((m) => (
        <ModeChip key={m} mode={m} />
      ))}
      {overflow > 0 && (
        <span className="inline-flex items-center px-1.5 py-0.5 text-[9px] font-mono leading-tight border rounded-sm bg-gray-700/30 text-gray-400 border-gray-700">
          +{overflow}
        </span>
      )}
    </span>
  );
}
