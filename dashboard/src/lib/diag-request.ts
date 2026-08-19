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
export function buildDiagRequest(rawInput: string, preset: DiagPreset): DiagRequest | null {
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

  const presetLabel = preset.charAt(0).toUpperCase() + preset.slice(1);
  const configName = isSet
    ? `Diag set: ${host} +${entries.length - 1} (${presetLabel})`
    : `Diag: ${host} (${presetLabel})`;
  // Probe the URL as entered (root by default) — a bare host would get
  // `/health` appended by the agent (E2E P1-4). `host` stays bare for the
  // display name / watchlist grouping.
  const endpoint: EndpointRef = isSet
    ? { kind: 'network', host: toProbeUrl(entries[0]), hosts: entries.map(toProbeUrl) }
    : { kind: 'network', host: toProbeUrl(entries[0]) };
  const workload: Workload = {
    modes: DIAG_PRESETS[preset],
    runs: 1,
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
  };
  return { host, entries, isSet, configName, config };
}
