// ── HTTP-stack capabilities (shared/http-stacks.json mirror) ─────────────────
//
// Which comparison proxy stacks the installers configure WITH HTTP/3, and which
// probe modes need HTTP/3 on the target. The canonical file is the repo-root
// shared/http-stacks.json — embedded by the Rust tester (http_stacks.rs), by
// the control plane (HttpStackCatalog → GET /api/http-stacks, GET /api/modes
// `stacks`/`h3_modes`, and the 422 config-create gate) and read by
// lab/validate.sh. This is the dashboard's hand-copy, guarded byte-for-byte by
// http-stacks-manifest.test.ts so it cannot drift (the modes.json pattern).
//
// The lab measured http3 / pageload3 0/N through apache, haproxy and traefik:
// the mode pickers grey the h3 modes out for those stacks (mode-capabilities.ts)
// and the server rejects them at config-create.

/** One row of shared/http-stacks.json (ports omitted — the UI needs only h3). */
export interface HttpStackInfo {
  id: string;
  h3: boolean;
}

/** Stacks in manifest order (`endpoint` = the bare networker-endpoint). */
export const HTTP_STACKS: readonly HttpStackInfo[] = [
  { id: 'endpoint', h3: true },
  { id: 'nginx', h3: true },
  { id: 'iis', h3: true },
  { id: 'caddy', h3: true },
  { id: 'traefik', h3: false },
  { id: 'haproxy', h3: false },
  { id: 'apache', h3: false },
];

/** Probe modes that need HTTP/3 (QUIC) on the target — manifest `h3_modes`. */
export const H3_MODES: readonly string[] = ['http3', 'pageload3', 'browser3', 'download3', 'upload3'];

const H3_MODE_SET = new Set(H3_MODES);
const STACK_BY_ID = new Map(HTTP_STACKS.map(s => [s.id, s] as const));

/** True when the mode needs HTTP/3 on the target. */
export function isH3Mode(mode: string): boolean {
  return H3_MODE_SET.has(mode.toLowerCase());
}

/**
 * Whether a stack serves HTTP/3: `true` / `false` for manifest stacks,
 * `null` for unknown / blank ids (callers fail open — never fabricate).
 */
export function stackHasH3(stack: string | null | undefined): boolean | null {
  if (!stack) return null;
  return STACK_BY_ID.get(stack.trim().toLowerCase())?.h3 ?? null;
}

/**
 * Over a set of stacks (a comparison deployment, a multi-proxy testbed), h3 is
 * unavailable only when EVERY known stack lacks it — a matrix over nginx +
 * apache still runs h3 modes on the nginx cell (the server drops them per cell).
 * Returns `null` when no stack is known.
 */
export function anyStackHasH3(stacks: readonly string[]): boolean | null {
  let known = false;
  for (const s of stacks) {
    const h3 = stackHasH3(s);
    if (h3 === null) continue;
    known = true;
    if (h3) return true;
  }
  return known ? false : null;
}

/** The subset of `stacks` (manifest-known) that has no HTTP/3. */
export function stacksWithoutH3(stacks: readonly string[]): string[] {
  return stacks.filter(s => stackHasH3(s) === false);
}
