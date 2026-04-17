// ── Mode family mapping ──────────────────────────────────────────────────
//
// Source of truth for "what color is mode X" — shared by ModeChip
// (render-time lookup) and the NetworkTestPage / EndpointRunsPage
// family-group rendering (render family headers by id).
//
// Keep this list aligned with networker-common/src/test_config.rs Protocol.
// Kept in its own file (not ModeChip.tsx) so the component file stays
// component-only, which HMR fast-refresh requires.

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
export const CHIP_CLASSES: Record<ModeFamily, string> = {
  net:   'bg-green-400/[.14]  text-green-300  border-green-400/50',
  http:  'bg-cyan-400/[.14]   text-cyan-300   border-cyan-400/50',
  thru:  'bg-violet-400/[.16] text-violet-300 border-violet-400/55',
  page:  'bg-amber-400/[.14]  text-amber-300  border-amber-400/50',
  other: 'bg-gray-700/30      text-gray-400   border-gray-700',
};
