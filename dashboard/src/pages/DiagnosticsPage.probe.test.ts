// #812: URL Probe 409 "config already exists". The probe page reused configs
// via a CLIENT-SIDE name search over the project config list, but that list is
// capped at the 200 newest — old `Diag:` names fell out of the window, the
// find missed, the create hit UNIQUE(project_id, name), and the run died with
// a 409. The create request now carries `find_or_create: true` so the server
// returns the existing row (200, same shape as a fresh create) instead of
// conflicting. These tests pin that flag — and the config shape — for BOTH the
// single-URL path and the multi-URL set path (#788/#782 `Diag set:` naming).
//
// Review follow-ups on #820/#782:
// - Set names now carry a 6-hex membership hash over the SORTED full probe
//   URLs — first-host+count alone made "a.com b.com" and "a.com c.com" share a
//   reuse key, and find_or_create silently probed the first set's URLs.
// - max_duration_secs is always set and scales with samples × URLs × preset
//   cost — the flat 1800s (bursts only) let a Full x5 over 8+ URLs get
//   watchdog-killed mid-flight, and a single-sample multi-URL Full set got no
//   headroom at all.

import { describe, expect, it } from 'vitest';
import {
  buildDiagRequest,
  decodeHostQueryParam,
  extractHosts,
  hostsToQueryParam,
} from '../lib/diag-request';

describe('buildDiagRequest', () => {
  it('single URL: idempotent create flag is set (the 409-free path)', () => {
    const req = buildDiagRequest('microsoft.com', 'full');
    expect(req).not.toBeNull();
    expect(req!.config.find_or_create).toBe(true);
    expect(req!.isSet).toBe(false);
    expect(req!.host).toBe('microsoft.com');
    expect(req!.configName).toBe('Diag: microsoft.com (Full)');
    expect(req!.config.name).toBe('Diag: microsoft.com (Full)');
    expect(req!.config.test_kind).toBe('url_probe');
    // Probed as entered → root URL, not <host>/health (E2E P1-4).
    expect(req!.config.endpoint).toEqual({ kind: 'network', host: 'https://microsoft.com/' });
  });

  it('multi-URL set: idempotent create flag is set on the Diag set path too', () => {
    const req = buildDiagRequest('a.example.com, b.example.com\nc.example.com', 'quick');
    expect(req).not.toBeNull();
    expect(req!.config.find_or_create).toBe(true);
    expect(req!.isSet).toBe(true);
    expect(req!.host).toBe('a.example.com');
    expect(req!.configName).toMatch(/^Diag set: a\.example\.com \+2 \[[0-9a-f]{6}\] \(Quick\)$/);
    expect(req!.config.endpoint).toEqual({
      kind: 'network',
      host: 'https://a.example.com/',
      hosts: ['https://a.example.com/', 'https://b.example.com/', 'https://c.example.com/'],
    });
  });

  it('re-running the same host+preset builds the SAME config name (reuse key)', () => {
    const first = buildDiagRequest('https://example.com/pricing', 'standard');
    const again = buildDiagRequest('https://example.com/pricing', 'standard');
    // Single-URL names stay hash-less — no membership to encode.
    expect(first!.configName).toBe('Diag: example.com (Standard)');
    expect(again!.configName).toBe(first!.configName);
  });

  // ── Set membership hash (review follow-up on #820) ─────────────────────

  it('sets differing only in the SECOND member get different reuse keys', () => {
    // The collision that motivated the hash: same first host, same count —
    // the old "+N"-only name made "a.com c.com" reuse "a.com b.com"'s config
    // and silently probe the wrong URLs.
    const ab = buildDiagRequest('a.example.com b.example.com', 'quick');
    const ac = buildDiagRequest('a.example.com c.example.com', 'quick');
    expect(ab!.configName).not.toBe(ac!.configName);
  });

  it('the same set hashes deterministically and order-insensitively', () => {
    const once = buildDiagRequest('a.example.com b.example.com', 'quick');
    const again = buildDiagRequest('a.example.com b.example.com', 'quick');
    expect(again!.configName).toBe(once!.configName);
    // Hash input is SORTED — entry order must not fork the reuse key. The
    // first host in the display prefix still differs by entry order, so
    // compare the [hash] segment.
    const hashOf = (name: string) => name.match(/\[([0-9a-f]{6})\]/)![1];
    const reversed = buildDiagRequest('b.example.com a.example.com', 'quick');
    expect(hashOf(reversed!.configName)).toBe(hashOf(once!.configName));
  });

  it('same hosts with different PATHS are different sets (probe URLs hashed)', () => {
    const roots = buildDiagRequest('a.example.com b.example.com', 'quick');
    const paths = buildDiagRequest('a.example.com b.example.com/health', 'quick');
    expect(paths!.configName).not.toBe(roots!.configName);
  });

  it('preserves the full URL as entered (path included) in the probe endpoint', () => {
    const req = buildDiagRequest('https://example.com/pricing?x=1', 'quick');
    expect(req!.config.endpoint).toEqual({
      kind: 'network',
      host: 'https://example.com/pricing?x=1',
    });
  });

  it('dedupes repeated entries before deciding single vs set', () => {
    const req = buildDiagRequest('example.com example.com', 'quick');
    expect(req!.isSet).toBe(false);
    expect(req!.entries).toEqual(['example.com']);
    expect(req!.configName).toBe('Diag: example.com (Quick)');
  });

  it('dedupes by RESOLVED probe URL, not by raw text (#782 P1)', () => {
    // "example.com" and "https://example.com/" are one probe. Raw-string
    // de-duplication let both through, so the set carried the same URL twice:
    // two --target flags for one target, a `+N` that overstated the
    // membership, and a per-URL grouping that reported one URL with double
    // the attempts.
    const req = buildDiagRequest('example.com\nhttps://example.com/', 'quick');
    expect(req!.isSet).toBe(false);
    expect(req!.entries).toEqual(['example.com']);
    expect(req!.config.endpoint).toEqual({ kind: 'network', host: 'https://example.com/' });

    const set = buildDiagRequest('example.com https://example.com/ b.example.com', 'quick');
    expect(set!.config.endpoint).toEqual({
      kind: 'network',
      host: 'https://example.com/',
      hosts: ['https://example.com/', 'https://b.example.com/'],
    });
    expect(set!.configName).toMatch(/^Diag set: example\.com \+1 \[[0-9a-f]{6}\] \(Quick\)$/);
  });

  it('distinct paths on one host stay distinct members', () => {
    // The de-duplication is by full URL, so /a and /b are two targets.
    const req = buildDiagRequest('example.com/a\nexample.com/b', 'quick');
    expect(req!.config.endpoint).toEqual({
      kind: 'network',
      host: 'https://example.com/a',
      hosts: ['https://example.com/a', 'https://example.com/b'],
    });
  });

  it('returns null on empty input', () => {
    expect(buildDiagRequest('', 'quick')).toBeNull();
    expect(buildDiagRequest('  , \n ', 'quick')).toBeNull();
  });

  // ── Burst sampling (#782 P2) ──────────────────────────────────────────

  it('defaults to a single sample per mode (runs: 1, no name suffix)', () => {
    const req = buildDiagRequest('example.com', 'quick');
    expect(req!.config.workload.runs).toBe(1);
    expect(req!.configName).toBe('Diag: example.com (Quick)');
  });

  it('burst: samples become workload.runs AND part of the reuse key', () => {
    const req = buildDiagRequest('example.com', 'quick', 5);
    expect(req!.config.workload.runs).toBe(5);
    // A x5 config must NOT find_or_create-collide with the runs:1 config —
    // the server would return the existing row and silently drop the burst.
    expect(req!.configName).toBe('Diag: example.com (Quick x5)');
  });

  it('burst on a set keeps the set naming (x-suffix inside the preset label)', () => {
    const req = buildDiagRequest('a.example.com b.example.com', 'standard', 3);
    expect(req!.configName).toMatch(
      /^Diag set: a\.example\.com \+1 \[[0-9a-f]{6}\] \(Standard x3\)$/,
    );
    expect(req!.config.workload.runs).toBe(3);
  });

  // ── Watchdog headroom (review follow-up on #820) ────────────────────────
  // max_duration_secs = clamp(900, 2 × PRESET_EST_SECS × samples × URLs, 7200)
  // — ALWAYS set. The flat 1800s (bursts only) killed a Full x5 over 8+ URLs
  // mid-flight and gave a single-sample multi-URL Full set nothing at all.

  it('single quick probe keeps the 900s default floor', () => {
    expect(buildDiagRequest('example.com', 'quick')!.config.max_duration_secs).toBe(900);
  });

  it('full, 1 sample, 1 URL stays at the floor (2×45s < 900s)', () => {
    expect(buildDiagRequest('example.com', 'full')!.config.max_duration_secs).toBe(900);
  });

  it('full x5 over 8 URLs scales up (2 × 45 × 5 × 8 = 3600s — breached the old 1800s)', () => {
    const urls = Array.from({ length: 8 }, (_, i) => `h${i}.example.com`).join(' ');
    expect(buildDiagRequest(urls, 'full', 5)!.config.max_duration_secs).toBe(3600);
  });

  it('runaway sets clamp at the 7200s ceiling (split the set instead)', () => {
    const urls = Array.from({ length: 40 }, (_, i) => `h${i}.example.com`).join(' ');
    expect(buildDiagRequest(urls, 'full', 5)!.config.max_duration_secs).toBe(7200);
  });
});

// ── #820: ?host= round-trip (the %2C%2520 double-encode) ──────────────────

describe('host query param helpers', () => {
  it('extractHosts splits on whitespace/commas/newlines and dedupes', () => {
    expect(extractHosts('example.com, www.cloudflare.com\nexample.com')).toEqual([
      'example.com',
      'www.cloudflare.com',
    ]);
    expect(extractHosts('https://example.com/pricing')).toEqual(['example.com']);
    expect(extractHosts('  ')).toEqual([]);
  });

  it('hostsToQueryParam emits hostnames only, comma-joined, NO spaces', () => {
    // The raw input "a, b" used to be stored verbatim — the space is what
    // percent-encoded (and later double-encoded to %2520) in the URL bar.
    expect(hostsToQueryParam('example.com, www.cloudflare.com')).toBe(
      'example.com,www.cloudflare.com',
    );
    expect(hostsToQueryParam('https://example.com/pricing')).toBe('example.com');
    expect(hostsToQueryParam('')).toBe('');
  });

  it('round-trips through URLSearchParams without double encoding', () => {
    const params = new URLSearchParams();
    params.set('host', hostsToQueryParam('example.com, www.cloudflare.com'));
    expect(params.toString()).toBe('host=example.com%2Cwww.cloudflare.com');
    const read = decodeHostQueryParam(new URLSearchParams(params.toString()).get('host'));
    expect(read).toBe('example.com, www.cloudflare.com');
  });

  it('decodeHostQueryParam repairs the prod double-encoded value (#820)', () => {
    // ?host=example.com%2C%2520www.cloudflare.com → .get() decodes once to
    // "example.com,%20www.cloudflare.com" — one more layer remains.
    expect(decodeHostQueryParam('example.com,%20www.cloudflare.com')).toBe(
      'example.com, www.cloudflare.com',
    );
    expect(decodeHostQueryParam('example.com')).toBe('example.com');
    expect(decodeHostQueryParam(null)).toBe('');
    // A literal stray '%' must not throw.
    expect(decodeHostQueryParam('example.com,%')).toBe('example.com, %');
  });
});
