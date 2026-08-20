// URL Probe request building — extracted from DiagnosticsPage so the config
// shape (and its #812 idempotency flag) is a pure, unit-testable function.

import type { EndpointRef, TestConfigCreate, Workload } from '../api/types';

export type DiagPreset = 'quick' | 'standard' | 'full' | 'route';

// Endpoint-only modes (udp echo, native pageload asset ladder) are excluded:
// URL diagnostics always target arbitrary URLs, where those modes fail by
// construction (user-caught 2026-08-12 — 4 guaranteed-failed attempts per Full
// run). Real-site page load is covered by the browser* modes. Keep in lockstep
// with mode-capabilities.ts / shared/modes.json `requires`.
export const DIAG_PRESETS: Record<DiagPreset, string[]> = {
  quick: ['dns', 'tcp', 'tls', 'http2'],
  standard: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3'],
  full: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'curl', 'browser1', 'browser2', 'browser3'],
  // Reachability & route diagnostics (v0.28.78 modes — all `any`-target).
  // ping may need ICMP privileges on the runner (Linux ping_group_range);
  // a denial surfaces as an honest per-attempt Config error, not a hang.
  route: ['ping', 'path', 'dualstack', 'pmtud'],
};

export function extractHost(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return '';
  try {
    if (trimmed.includes('://')) {
      return new URL(trimmed).hostname;
    }
    const candidate = new URL(`https://${trimmed}`);
    return candidate.hostname;
  } catch {
    return trimmed;
  }
}

/**
 * Split a (possibly multi-entry) probe input into hostnames — the multi-URL
 * twin of {@link extractHost}. Entries are separated by whitespace, commas, or
 * newlines; each is reduced to its hostname; duplicates collapse.
 */
export function extractHosts(input: string): string[] {
  const hosts = input
    .split(/[\s,]+/)
    .map(e => extractHost(e))
    .filter(Boolean);
  return [...new Set(hosts)];
}

/**
 * `?host=` query-param value for a probe input (#820): hostnames only,
 * comma-joined with NO spaces. The old sync stored the raw input verbatim —
 * "example.com, www.cloudflare.com" — whose space then percent-encoded (and,
 * via links built from already-encoded values, DOUBLE-encoded to `%2520`).
 * Hostnames and bare commas round-trip through URLSearchParams cleanly.
 */
export function hostsToQueryParam(input: string): string {
  return extractHosts(input).join(',');
}

/**
 * Read a `?host=` param defensively (#820): `searchParams.get()` already
 * decoded once, but links written by the old sync (or pasted from them) can
 * still carry another layer or two of percent-encoding (`%2C%2520` → `,%20`).
 * Hostnames can never legitimately contain `%`, so keep decoding while a
 * percent-escape remains, then normalize separators to ", " for display.
 */
export function decodeHostQueryParam(value: string | null): string {
  if (!value) return '';
  let out = value;
  for (let i = 0; i < 3 && /%[0-9a-fA-F]{2}/.test(out); i++) {
    try {
      const decoded = decodeURIComponent(out);
      if (decoded === out) break;
      out = decoded;
    } catch {
      break;
    }
  }
  return extractHosts(out).join(', ');
}

/**
 * Full URL to actually probe — the URL Probe hits the URL AS ENTERED (root when
 * no path is given), not `<host>/health`. A bare host becomes `https://<host>/`;
 * a full URL is preserved. Passed as the config's endpoint host so the agent
 * uses it verbatim (a bare host would get `/health` appended — the E2E P1-4
 * false-failure on arbitrary sites like example.com).
 */
function toProbeUrl(input: string): string {
  const t = input.trim();
  try {
    const u = t.includes('://') ? new URL(t) : new URL(`https://${t}`);
    return u.toString();
  } catch {
    return t;
  }
}

/**
 * Rough wall-clock seconds one URL costs per sample, by preset — aligned with
 * the page's own DIAG_PRESET_LABELS estimates (~3s/~12s/~45s/~45s), padded.
 * Feeds the watchdog headroom calculation in {@link buildDiagRequest}.
 */
export const PRESET_EST_SECS: Record<DiagPreset, number> = {
  quick: 5,
  standard: 15,
  full: 45,
  route: 45,
};

/**
 * FNV-1a 32-bit over a string. Used to fingerprint a set's full membership in
 * the config name (the find_or_create reuse key) — first-host+count alone let
 * "a.com b.com" and "a.com c.com" collide, silently probing the wrong URLs.
 */
function fnv1a(input: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/** Burst-sampling choices offered by the probe page (#782 P2). */
export const DIAG_SAMPLE_CHOICES = [1, 3, 5] as const;
export type DiagSamples = (typeof DIAG_SAMPLE_CHOICES)[number];

/** What {@link buildDiagRequest} resolves the probe input box into. */
export interface DiagRequest {
  /** Display host (watchlist grouping / toasts) — the first entry's hostname. */
  host: string;
  /** De-duplicated URL/host entries as typed. */
  entries: string[];
  /** True when the input held more than one entry (multi-URL set, #782). */
  isSet: boolean;
  configName: string;
  config: TestConfigCreate;
}

/**
 * Build the URL-probe create request from the raw input (one URL/host, or a
 * multi-URL set separated by whitespace/commas/newlines — #782). Returns null
 * when there is nothing to probe.
 *
 * The config is sent with `find_or_create: true` (#812): re-running a
 * diagnostic against the same host+preset must reuse the existing config
 * (UNIQUE(project_id, name)), and the client-side reuse fast path over the
 * config list is capped at the 200 NEWEST configs — an old `Diag:` name that
 * fell out of that window used to 409 the create. The server now returns the
 * existing row (200, same shape as a fresh create) instead.
 */
export function buildDiagRequest(
  rawInput: string,
  preset: DiagPreset,
  samples: DiagSamples = 1,
): DiagRequest | null {
  const rawEntries = rawInput
    .split(/[\s,]+/)
    .map(e => e.trim())
    .filter(Boolean);
  const entries = [...new Set(rawEntries)];
  if (entries.length === 0) return null;
  // entries[0], not rawInput: after de-duplication a repeated single entry
  // ("example.com example.com") must resolve like the single entry it is.
  const host = extractHost(entries[0]);
  if (!host) return null;
  const isSet = entries.length > 1;

  // Burst sampling (#782 P2) is part of the reuse key: a x5 config must not
  // find_or_create-collide with the single-sample config of the same host —
  // the server would return the existing runs:1 row and silently drop the burst.
  const presetLabel = preset.charAt(0).toUpperCase() + preset.slice(1)
    + (samples > 1 ? ` x${samples}` : '');
  // Probe the URL as entered (root by default) — a bare host would get
  // `/health` appended by the agent (E2E P1-4). `host` stays bare for the
  // display name / watchlist grouping.
  const probeUrls = entries.map(toProbeUrl);
  // Set names carry a membership hash: first-host+count is NOT a sufficient
  // reuse key ("a.com b.com" vs "a.com c.com" collided, and find_or_create
  // silently probed the first set's URLs). Hash the SORTED full probe URLs —
  // order-insensitive, but same hosts with different paths ARE different sets.
  const setHash = fnv1a([...probeUrls].sort().join('\n'))
    .toString(16)
    .padStart(8, '0')
    .slice(0, 6);
  const configName = isSet
    ? `Diag set: ${host} +${entries.length - 1} [${setHash}] (${presetLabel})`
    : `Diag: ${host} (${presetLabel})`;
  const endpoint: EndpointRef = isSet
    ? { kind: 'network', host: probeUrls[0], hosts: probeUrls }
    : { kind: 'network', host: probeUrls[0] };
  // Burst sampling (#782 P2) rides `workload.samples` → the tester's
  // `--samples N`, NOT `runs`: a burst is N back-to-back samples of the same
  // logical attempt (tightly time-correlated, each published with its own
  // sample_index), whereas `runs` is N full passes over every mode. Both would
  // give N rows per point; only the burst gives a median of five readings
  // taken under the same conditions, which is the statistic the run-detail
  // median/spread section reports.
  const workload: Workload = {
    modes: DIAG_PRESETS[preset],
    runs: 1,
    samples,
    concurrency: 1,
    timeout_ms: 5000,
    payload_sizes: [],
    capture_mode: 'headers-only',
  };
  // Watchdog headroom scales with the workload: wall clock is roughly
  // samples × URLs × per-preset cost, so a flat cap keyed on samples alone
  // let a Full x5 over 8+ URLs breach 1800s and get killed mid-flight, while
  // a single-sample multi-URL Full set got no headroom at all (900s default).
  // 2× the estimate, floored at the 900s default; ceiling 7200s is a runaway
  // guard — a set large enough to breach it should be split. The server
  // watchdog grants max_duration + grace.
  const estSecs = PRESET_EST_SECS[preset] * samples * entries.length;
  const maxDurationSecs = Math.max(900, Math.min(7200, estSecs * 2));
  const config: TestConfigCreate = {
    name: configName,
    test_kind: 'url_probe',
    endpoint,
    workload,
    find_or_create: true,
    max_duration_secs: maxDurationSecs,
  };
  return { host, entries, isSet, configName, config };
}
