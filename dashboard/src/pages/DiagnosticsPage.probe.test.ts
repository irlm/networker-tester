// #812: URL Probe 409 "config already exists". The probe page reused configs
// via a CLIENT-SIDE name search over the project config list, but that list is
// capped at the 200 newest — old `Diag:` names fell out of the window, the
// find missed, the create hit UNIQUE(project_id, name), and the run died with
// a 409. The create request now carries `find_or_create: true` so the server
// returns the existing row (200, same shape as a fresh create) instead of
// conflicting. These tests pin that flag — and the config shape — for BOTH the
// single-URL path and the multi-URL set path (#788/#782 `Diag set:` naming).

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
    expect(req!.configName).toBe('Diag set: a.example.com +2 (Quick)');
    expect(req!.config.endpoint).toEqual({
      kind: 'network',
      host: 'https://a.example.com/',
      hosts: ['https://a.example.com/', 'https://b.example.com/', 'https://c.example.com/'],
    });
  });

  it('re-running the same host+preset builds the SAME config name (reuse key)', () => {
    const first = buildDiagRequest('https://example.com/pricing', 'standard');
    const again = buildDiagRequest('https://example.com/pricing', 'standard');
    expect(first!.configName).toBe('Diag: example.com (Standard)');
    expect(again!.configName).toBe(first!.configName);
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

  it('returns null on empty input', () => {
    expect(buildDiagRequest('', 'quick')).toBeNull();
    expect(buildDiagRequest('  , \n ', 'quick')).toBeNull();
  });

  // ── Burst sampling (#782 P2) ──────────────────────────────────────────

  it('defaults to a single sample per mode (runs: 1, no name suffix)', () => {
    const req = buildDiagRequest('example.com', 'quick');
    expect(req!.config.workload.runs).toBe(1);
    expect(req!.configName).toBe('Diag: example.com (Quick)');
    expect(req!.config.max_duration_secs).toBeUndefined();
  });

  it('burst: samples become workload.runs AND part of the reuse key', () => {
    const req = buildDiagRequest('example.com', 'quick', 5);
    expect(req!.config.workload.runs).toBe(5);
    // A x5 config must NOT find_or_create-collide with the runs:1 config —
    // the server would return the existing row and silently drop the burst.
    expect(req!.configName).toBe('Diag: example.com (Quick x5)');
    // Bursts get watchdog headroom (a Full x5 set brushes the 900s default).
    expect(req!.config.max_duration_secs).toBe(1800);
  });

  it('burst on a set keeps the set naming', () => {
    const req = buildDiagRequest('a.example.com b.example.com', 'standard', 3);
    expect(req!.configName).toBe('Diag set: a.example.com +1 (Standard x3)');
    expect(req!.config.workload.runs).toBe(3);
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
