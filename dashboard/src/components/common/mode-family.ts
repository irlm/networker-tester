// ── Mode family mapping ──────────────────────────────────────────────────
//
// Source of truth for "what color is mode X" — shared by ModeChip
// (render-time lookup) and the NetworkTestPage / EndpointRunsPage
// family-group rendering (render family headers by id).
//
// Keep this list aligned with shared/modes.json (canonical manifest generated
// from the engine's Protocol enum in crates/networker-tester/src/metrics.rs).
// Guarded by dashboard/src/lib/modes-manifest.test.ts — ids here must be
// manifest ids (or documented CLI aliases) and families must match.
// Kept in its own file (not ModeChip.tsx) so the component file stays
// component-only, which HMR fast-refresh requires.

export type ModeFamily = 'net' | 'http' | 'thru' | 'page' | 'other';

export const FAMILY_BY_MODE: Record<string, ModeFamily> = {
  // Network primitives
  tcp: 'net', dns: 'net', tls: 'net', tlsresume: 'net', native: 'net', udp: 'net',
  // HTTP semantics
  http1: 'http', http2: 'http', http3: 'http', curl: 'http', sdkprobe: 'http',
  // Throughput
  download: 'thru', upload: 'thru',
  download1: 'thru', download2: 'thru', download3: 'thru',
  upload1: 'thru', upload2: 'thru', upload3: 'thru',
  webdownload: 'thru', webupload: 'thru',
  udpdownload: 'thru', udpupload: 'thru',
  // Page load (native + browser); pageload1 is the CLI alias for pageload.
  pageload: 'page', pageload1: 'page', pageload2: 'page', pageload3: 'page',
  browser: 'page', browser1: 'page', browser2: 'page', browser3: 'page',
};

export function familyOf(mode: string): ModeFamily {
  return FAMILY_BY_MODE[mode.toLowerCase()] ?? 'other';
}

// One accent for selected/active mode chips (north-star color discipline —
// audit F12: four category hues collided with the green/red/yellow status
// ramp and put green+cyan chips side by side in one configured form).
// Family grouping still carries the taxonomy; color stays a selection signal.
export const CHIP_CLASSES: Record<ModeFamily, string> = {
  net:   'bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50',
  http:  'bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50',
  thru:  'bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50',
  page:  'bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50',
  other: 'bg-gray-700/30    text-gray-400 border-gray-700',
};

// Display labels for opaque mode ids (audit P2: `pageload2` chips vs the Full
// Stack wizard's "Page Load H2" checkboxes — one vocabulary, wizard's wins).
// Ids sent to the backend are unchanged; this is display-only.
export const MODE_LABELS: Record<string, string> = {
  tlsresume: 'tls resume',
  native: 'native tls',
  pageload: 'pageload native',
  pageload1: 'pageload native',
  pageload2: 'pageload h2',
  pageload3: 'pageload h3',
  browser1: 'browser h1',
  browser2: 'browser h2',
  browser3: 'browser h3',
};

export function modeLabel(mode: string): string {
  return MODE_LABELS[mode.toLowerCase()] ?? mode;
}
