// #812: URL Probe 409 "config already exists". The probe page reused configs
// via a CLIENT-SIDE name search over the project config list, but that list is
// capped at the 200 newest — old `Diag:` names fell out of the window, the
// find missed, the create hit UNIQUE(project_id, name), and the run died with
// a 409. The create request now carries `find_or_create: true` so the server
// returns the existing row (200, same shape as a fresh create) instead of
// conflicting. These tests pin that flag — and the config shape — for BOTH the
// single-URL path and the multi-URL set path (#788/#782 `Diag set:` naming).

import { describe, expect, it } from 'vitest';
import { buildDiagRequest } from '../lib/diag-request';

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
});
