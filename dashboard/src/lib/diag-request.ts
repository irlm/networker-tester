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
  const configName = isSet
    ? `Diag set: ${host} +${entries.length - 1} (${presetLabel})`
    : `Diag: ${host} (${presetLabel})`;
  // Probe the URL as entered (root by default) — a bare host would get
  // `/health` appended by the agent (E2E P1-4). `host` stays bare for the
  // display name / watchlist grouping.
  const endpoint: EndpointRef = isSet
    ? { kind: 'network', host: toProbeUrl(entries[0]), hosts: entries.map(toProbeUrl) }
    : { kind: 'network', host: toProbeUrl(entries[0]) };
  // Each iteration re-probes every mode — the tester's `--runs N` loop already
  // publishes ALL logical attempts (retry collapsing is per logical attempt),
  // so a burst yields N samples per mode per URL, and the run-detail p50/p95
  // stats become meaningful within a single point (#782 P2).
  const workload: Workload = {
    modes: DIAG_PRESETS[preset],
    runs: samples,
    concurrency: 1,
    timeout_ms: 5000,
    payload_sizes: [],
    capture_mode: 'headers-only',
  };
  const config: TestConfigCreate = {
    name: configName,
    test_kind: 'url_probe',
    endpoint,
    workload,
    find_or_create: true,
    // A x5 Full burst across a set can brush the 900s default watchdog cap
    // (~45s x 5 x N URLs) — give bursts honest headroom instead of a
    // mid-flight kill.
    ...(samples > 1 ? { max_duration_secs: 1800 } : {}),
  };
  return { host, entries, isSet, configName, config };
}
