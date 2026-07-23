// ── Mode ↔ target capability gating ──────────────────────────────────────────
//
// Prevents launching probe modes that can only FAIL against the chosen target —
// e.g. `apibench` needs the application-benchmark reference APIs, `sdkprobe`
// needs a customer-embedded LagHound SDK endpoint (Server-Timing), and the
// throughput / UDP / page-load modes need a networker-endpoint server (their
// specific routes + ports), not an arbitrary URL. Selecting those against the
// wrong target produces a run that errors every time.
//
// First slice: this classification lives in the frontend. The cross-stack source
// of truth is intended to move to a `requires` field in shared/modes.json (behind
// the existing manifest drift guards) so the backend and every SDK agree — see
// the "capability model" design note. Keep the ids here aligned with
// shared/modes.json / mode-family.ts (guarded by modes-manifest.test.ts).

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

  // NOTE: `udp` (echo RTT), `pageload*` (native page fetch), and `browser*`
  // (Chrome) are 'any' — the URL Probe runs all of them against arbitrary URLs
  // (they load a real page / probe a real host), so they must NOT be gated as
  // endpoint-only. Chrome-on-tester is a tester capability, not a target one —
  // a separate axis for a later slice.
};

export function requirementOf(mode: string): ModeRequirement {
  return MODE_REQUIREMENT[mode.toLowerCase()] ?? 'any';
}

/** What kind of target the run is aimed at. */
export type TargetKind =
  | 'url' //      an arbitrary URL / host (URL Diagnostics)
  | 'endpoint' // a provisioned networker-endpoint (deployment / proxy stack)
  | 'sdk'; //     a customer-embedded LagHound SDK endpoint

export interface TargetCapabilities {
  kind: TargetKind;
}

const REASON: Record<Exclude<ModeRequirement, 'any'>, string> = {
  'networker-endpoint':
    'Needs a networker-endpoint target (throughput / UDP / page-load servers) — not an arbitrary URL.',
  'sdk-endpoint':
    'Needs a customer LagHound SDK endpoint (Server-Timing) — use the SDK / Application flow.',
  'reference-apis':
    'Needs the application-benchmark reference APIs — use the Application Benchmark flow.',
};

/**
 * `null` when the target can run this mode; otherwise a human-readable reason it
 * cannot (shown as a tooltip on the disabled picker row).
 */
export function unsupportedReason(mode: string, caps: TargetCapabilities): string | null {
  const req = requirementOf(mode);
  switch (req) {
    case 'any':
      return null;
    case 'networker-endpoint':
      // A provisioned endpoint (and, by definition, an SDK endpoint host) serves
      // these; only a raw URL cannot.
      return caps.kind === 'url' ? REASON[req] : null;
    case 'sdk-endpoint':
      return caps.kind === 'sdk' ? null : REASON[req];
    case 'reference-apis':
      // The reference-API suite is its own test type; no target kind here runs it.
      return REASON[req];
    default:
      return null;
  }
}

export function isModeSupported(mode: string, caps: TargetCapabilities): boolean {
  return unsupportedReason(mode, caps) === null;
}
