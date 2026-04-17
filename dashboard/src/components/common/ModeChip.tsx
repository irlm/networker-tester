// ── ModeChip ─────────────────────────────────────────────────────────────
//
// Single-mode chip color-coded by family. Same color language across the
// endpoint runs list, preset cards, filter chips, and any future surface
// that lists test modes.
//
// Family mapping is the source of truth for "what color is mode X" — keep
// this list aligned with networker-common/src/test_config.rs Protocol enum.

export type ModeFamily = 'net' | 'http' | 'thru' | 'page' | 'other';

const FAMILY_BY_MODE: Record<string, ModeFamily> = {
  // Network primitives
  tcp: 'net', dns: 'net', tls: 'net', tlsresume: 'net', nativetls: 'net', udp: 'net',
  // HTTP semantics
  http1: 'http', http2: 'http', http3: 'http', curl: 'http',
  // Throughput
  download: 'thru', upload: 'thru',
  downloadh1: 'thru', downloadh2: 'thru', downloadh3: 'thru',
  uploadh1: 'thru', uploadh2: 'thru', uploadh3: 'thru',
  webdownload: 'thru', webupload: 'thru',
  udpdownload: 'thru', udpupload: 'thru',
  // Page load (native + browser)
  pageload: 'page', pageload1: 'page', pageload2: 'page', pageload3: 'page',
  browser: 'page', browser1: 'page', browser2: 'page', browser3: 'page',
};

export function familyOf(mode: string): ModeFamily {
  return FAMILY_BY_MODE[mode.toLowerCase()] ?? 'other';
}

// WCAG-AA chip colors on dark base. Background opacity ~.14, text uses
// Tailwind 300-level shade so every family has visible weight.
const CHIP_CLASSES: Record<ModeFamily, string> = {
  net:   'bg-green-400/[.14]  text-green-300  border-green-400/50',
  http:  'bg-cyan-400/[.14]   text-cyan-300   border-cyan-400/50',
  thru:  'bg-violet-400/[.16] text-violet-300 border-violet-400/55',
  page:  'bg-amber-400/[.14]  text-amber-300  border-amber-400/50',
  other: 'bg-gray-700/30      text-gray-400   border-gray-700',
};

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
