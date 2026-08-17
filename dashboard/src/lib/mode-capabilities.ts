// ── Mode ↔ target capability gating ──────────────────────────────────────────
//
// Prevents launching probe modes that can only FAIL against the chosen target —
// e.g. `apibench` needs the application-benchmark reference APIs, `sdkprobe`
// needs a customer-embedded LagHound SDK endpoint (Server-Timing), and the
// throughput / UDP / page-load modes need a networker-endpoint server (their
// specific routes + ports), not an arbitrary URL. Selecting those against the
// wrong target produces a run that errors every time.
//
// The classification is the `requires` field in shared/modes.json (guarded by
// modes-manifest.test.ts here and ModesManifestTests C#-side; served by
// GET /api/modes) — MODE_REQUIREMENT below is the dashboard hand-copy. Since
// v0.28.208 the gate also knows three more axes, all fail-open on unknowns:
//   - HTTP/3 by proxy stack (shared/http-stacks.json via lib/http-stacks.ts):
//     apache / haproxy / traefik have no QUIC, so http3 / pageload3 / browser3 /
//     download3 / upload3 are off for them (the server rejects with 422 too);
//   - the target's LIVE self-report (GET …/deployments/{id}/capabilities) — the
//     server applies it at config-create too (v0.28.211, cache-only, fail-open);
//   - the pinned RUNNER's tool inventory (agent heartbeat `capabilities`):
//     browser* modes need Chrome on the runner.
// The server mirror is Endpoints/ModeTargetCompatibility.cs.

import { anyStackHasH3, isH3Mode, stackHasH3, stacksWithoutH3 } from './http-stacks';

/** What a probe mode needs from the target it runs against. */
export type ModeRequirement =
  | 'any' //               any reachable host/URL (tcp, dns, tls, http1/2/3, curl, …)
  | 'networker-endpoint' // the endpoint server's routes/ports (throughput, UDP, page-load)
  | 'sdk-endpoint' //      a customer LagHound SDK endpoint with Server-Timing → sdkprobe
  | 'reference-apis'; //   the application-benchmark reference API suite       → apibench

/**
 * Explicit non-`any` requirements. Anything not listed defaults to `any`
 * (the network + HTTP primitives, which probe any reachable server).
 */
export const MODE_REQUIREMENT: Readonly<Record<string, ModeRequirement>> = {
  sdkprobe: 'sdk-endpoint',
  apibench: 'reference-apis',

  // Throughput — needs the endpoint's /download, /upload, and UDP-throughput
  // (:9998) servers, which an arbitrary URL doesn't run.
  download: 'networker-endpoint',
  upload: 'networker-endpoint',
  download1: 'networker-endpoint',
  download2: 'networker-endpoint',
  download3: 'networker-endpoint',
  upload1: 'networker-endpoint',
  upload2: 'networker-endpoint',
  upload3: 'networker-endpoint',
  webdownload: 'networker-endpoint',
  webupload: 'networker-endpoint',
  udpdownload: 'networker-endpoint',
  udpupload: 'networker-endpoint',

  // Latency-under-load (bufferbloat / RPM) — needs the endpoint's /download
  // route to saturate the link AND its UDP echo server (:9999).
  rpm: 'networker-endpoint',

  // Draft-conformant responsiveness — needs the endpoint's /download and
  // /upload routes for the H2 load ramp + 1-byte probe object.
  responsiveness: 'networker-endpoint',

  // STAMP (RFC 8762) — needs the endpoint's Session-Reflector (:9997);
  // arbitrary hosts run no STAMP reflector.
  stamp: 'networker-endpoint',

  // Multi-connection capacity — needs the endpoint's /download and /upload
  // routes for the parallel H2 load ramp (time-boxed stages).
  mthroughput: 'networker-endpoint',

  // WebSocket probe — needs the endpoint's /ws echo route for the
  // message-RTT phase; an arbitrary URL has no frame-echoing WS server.
  websocket: 'networker-endpoint',

  // UDP echo RTT — needs the endpoint's UDP echo server (:9999); an
  // arbitrary URL has nothing speaking the echo protocol, so every probe is
  // "lost" by construction (proven live 2026-08-12: 10/10 lost against a
  // Cloudflare-fronted site while everything TCP was healthy).
  udp: 'networker-endpoint',

  // Native page-load ladder — fetches the endpoint's synthetic
  // `/asset?id=N&bytes=M` route; against a real website all assets 404 and
  // the 2xx-only success rule (v0.28.81) correctly fails the attempt. Real
  // websites are measured by the `browser*` modes, which load the actual
  // page in headless Chrome and stay 'any'.
  pageload: 'networker-endpoint',
  pageload2: 'networker-endpoint',
  pageload3: 'networker-endpoint',

  // NOTE: `pmtud` is 'any' — DF-bit path-MTU discovery concludes from ICMP
  // fragmentation-needed errors alone; the endpoint's UDP echo (:9999) only
  // upgrades the evidence when present.

  // NOTE: `browser*` (Chrome) stay 'any' — they load the real page.
  // Chrome-on-tester is a tester capability, not a target one — a separate
  // axis for a later slice.
};

export function requirementOf(mode: string): ModeRequirement {
  return MODE_REQUIREMENT[mode.toLowerCase()] ?? 'any';
}

/** What kind of target the run is aimed at. */
export type TargetKind =
  | 'url' //      an arbitrary URL / host (URL Diagnostics)
  | 'endpoint' // a provisioned networker-endpoint (deployment / proxy stack)
  | 'sdk'; //     a customer-embedded LagHound SDK endpoint

/** Runner (agent) tool inventory, from the agent heartbeat (v0.28.208+). */
export interface RunnerCapabilities {
  /** Chrome/Chromium present — the `browser*` modes spawn it. `null` = unknown. */
  chrome?: boolean | null;
  /** tshark present — packet capture needs it. `null` = unknown. */
  tshark?: boolean | null;
}

/**
 * Everything the picker knows about where a run will go. Only `kind` is
 * required; every other axis narrows the answer ONLY on positive knowledge —
 * an unknown stack, a missing live report or an unpinned runner never disables
 * anything (fail-open, matching the server gate).
 */
export interface TargetCapabilities {
  kind: TargetKind;
  /**
   * The proxy stack(s) the target resolves to — ids from shared/http-stacks.json
   * (`nginx`, `apache`, …). One stack for a single target; several for a matrix
   * / comparison deployment, where the h3 modes stay offered as long as ONE
   * stack serves HTTP/3 (the server drops them per cell on the others).
   */
  stack?: string | readonly string[] | null;
  /**
   * Live per-target self-report (deployment capabilities endpoint →
   * unsupported_modes): mode → reason for modes the target's endpoint has
   * disabled (udp / stamp listeners off, no page-asset route, …).
   */
  unsupported?: ReadonlyMap<string, string> | Readonly<Record<string, string>> | null;
  /** The pinned runner's inventory; omit / null when auto-picking. */
  runner?: RunnerCapabilities | null;
}

const REASON: Record<Exclude<ModeRequirement, 'any'>, string> = {
  'networker-endpoint':
    'Needs a networker-endpoint target (throughput / UDP / page-load servers) — not an arbitrary URL.',
  'sdk-endpoint':
    'Needs a customer LagHound SDK endpoint (Server-Timing) — use the SDK / Application flow.',
  'reference-apis':
    'Needs the application-benchmark reference APIs — use the Application Benchmark flow.',
};

/** Modes that spawn headless Chrome on the runner. */
const BROWSER_MODES = new Set(['browser', 'browser1', 'browser2', 'browser3']);

/** The reason an h3 mode is off for a stack without QUIC (mirrors the server's 422 text). */
export function h3StackReason(stack: string | readonly string[]): string {
  const list = typeof stack === 'string' ? [stack] : stacksWithoutH3(stack);
  const named = list.length > 0 ? list.join(', ') : 'this stack';
  return `Needs HTTP/3 (QUIC): ${named} has no HTTP/3 (see shared/http-stacks.json).`;
}

/** The reason a browser mode is off for a runner without Chrome. */
export const RUNNER_NO_CHROME_REASON =
  'Needs Chrome/Chromium on the runner — the pinned runner reports none (browser modes load the page in headless Chrome).';

/**
 * `null` when the target can run this mode; otherwise a human-readable reason it
 * cannot (shown as a tooltip on the disabled picker row). Checks, in order:
 * the manifest `requires` rule (kind), the HTTP/3-by-stack rule
 * (shared/http-stacks.json), the target's live self-report, and the pinned
 * runner's Chrome.
 */
export function unsupportedReason(mode: string, caps: TargetCapabilities): string | null {
  const id = mode.toLowerCase();
  const req = requirementOf(id);
  switch (req) {
    case 'any':
      break;
    case 'networker-endpoint':
      // A provisioned endpoint (and, by definition, an SDK endpoint host) serves
      // these; only a raw URL cannot.
      if (caps.kind === 'url') return REASON[req];
      break;
    case 'sdk-endpoint':
      if (caps.kind !== 'sdk') return REASON[req];
      break;
    case 'reference-apis':
      // The reference-API suite is its own test type; no target kind here runs it.
      return REASON[req];
    default:
      break;
  }

  // HTTP/3 by stack: apache / haproxy / traefik are installed h1/h2 only.
  if (caps.stack && isH3Mode(id)) {
    const h3 = typeof caps.stack === 'string' ? stackHasH3(caps.stack) : anyStackHasH3(caps.stack);
    if (h3 === false) return h3StackReason(caps.stack);
  }

  // Live self-report: only narrows on positive knowledge from the target.
  if (caps.unsupported) {
    const live = caps.unsupported instanceof Map
      ? caps.unsupported.get(id)
      : (caps.unsupported as Readonly<Record<string, string>>)[id];
    if (live) return live;
  }

  // Runner: browser modes need Chrome on the (pinned) runner.
  if (caps.runner?.chrome === false && BROWSER_MODES.has(id)) {
    return RUNNER_NO_CHROME_REASON;
  }

  return null;
}

export function isModeSupported(mode: string, caps: TargetCapabilities): boolean {
  return unsupportedReason(mode, caps) === null;
}

/**
 * The subset of `modes` the target cannot run, as mode → reason. Convenience
 * for pickers that hold a precomputed map (NetworkTestPage) and for tests.
 */
export function unsupportedModes(modes: Iterable<string>, caps: TargetCapabilities): Map<string, string> {
  const off = new Map<string, string>();
  for (const m of modes) {
    const why = unsupportedReason(m, caps);
    if (why !== null) off.set(m, why);
  }
  return off;
}
