// #765: the URL-probe watchlist accumulated dead nwk-a-* hosts because EVERY
// network-kind config counted as a watch entry — auto-provisioned matrix-cell
// targets, canary configs, and SDK endpoints included — and nothing cleaned
// them up when their deployment was torn down. Membership is now scoped to the
// configs the probe page itself creates ("Diag: <host> (Preset)"; pre-rename
// "Probe: …"). This pins the predicate.
//
// #820: "Diag set:" configs are watch entries too — a multi-URL set run used
// to be invisible on the probe page because the set config never qualified,
// so its detail was never fetched and its runs attributed to nobody. These
// tests also pin the host attribution (every set member gets the run) and the
// per-URL verdict override.

import { describe, expect, it } from 'vitest';
import {
  hostsForDiagConfig,
  hostsFromDiagConfigName,
  isDiagSetConfigName,
  isWatchlistConfigName,
  probeRunVerdict,
} from '../lib/watchlist';

describe('isWatchlistConfigName', () => {
  it('accepts the probe page naming (current and pre-rename)', () => {
    expect(isWatchlistConfigName('Diag: example.com (Quick)')).toBe(true);
    expect(isWatchlistConfigName('Diag: nwk-a-abc.eastus.cloudapp.azure.com (Full)')).toBe(true);
    expect(isWatchlistConfigName('Probe: example.com (Standard)')).toBe(true);
  });

  it('accepts multi-URL set configs (#820 — set runs must reach the watchlist)', () => {
    expect(isWatchlistConfigName('Diag set: example.com +1 (Quick)')).toBe(true);
    expect(isWatchlistConfigName('Diag set: example.com +3 (Full x5)')).toBe(true);
    expect(isDiagSetConfigName('Diag set: example.com +1 (Quick)')).toBe(true);
    expect(isDiagSetConfigName('Diag: example.com (Quick)')).toBe(false);
  });

  it('rejects configs other surfaces create against network targets', () => {
    // Matrix-cell / benchmark configs (the dead nwk-a-* pollution).
    expect(isWatchlistConfigName('apibench nwk-a-abc cell 3')).toBe(false);
    // Nightly canary probe configs (scripts/soak-canary.sh).
    expect(isWatchlistConfigName('soak-canary-probe-20260817T031500Z')).toBe(false);
    // SDK endpoints (they have their own page).
    expect(isWatchlistConfigName('Checkout API')).toBe(false);
    // Prefix must be exact — no separator or missing space does not count.
    expect(isWatchlistConfigName('Diagnostics run')).toBe(false);
    expect(isWatchlistConfigName('Diag:example.com')).toBe(false);
    expect(isWatchlistConfigName('Diag setup: example.com (Quick)')).toBe(false);
  });
});

describe('hostsFromDiagConfigName', () => {
  it('parses single-URL names (current, pre-rename, and burst-suffixed)', () => {
    expect(hostsFromDiagConfigName('Diag: example.com (Quick)')).toEqual(['example.com']);
    expect(hostsFromDiagConfigName('Probe: example.com (Standard)')).toEqual(['example.com']);
    expect(hostsFromDiagConfigName('Diag: example.com (Quick x5)')).toEqual(['example.com']);
  });

  it('parses set names to their first member (the name carries no more)', () => {
    expect(hostsFromDiagConfigName('Diag set: example.com +1 (Quick)')).toEqual(['example.com']);
    expect(hostsFromDiagConfigName('Diag set: example.com +3 (Full x5)')).toEqual(['example.com']);
  });

  it('returns [] for non-probe names', () => {
    expect(hostsFromDiagConfigName('Checkout API')).toEqual([]);
    expect(hostsFromDiagConfigName('apibench nwk-a-abc cell 3')).toEqual([]);
  });
});

describe('hostsForDiagConfig', () => {
  it('returns every member of a set config from endpoint.hosts (#820)', () => {
    expect(
      hostsForDiagConfig({
        name: 'Diag set: example.com +1 (Quick)',
        endpoint: {
          kind: 'network',
          host: 'https://example.com/',
          hosts: ['https://example.com/', 'https://www.cloudflare.com/'],
        },
      }),
    ).toEqual(['example.com', 'www.cloudflare.com']);
  });

  it('returns the single host for a single-URL config', () => {
    expect(
      hostsForDiagConfig({
        name: 'Diag: example.com (Quick)',
        endpoint: { kind: 'network', host: 'https://example.com/pricing' },
      }),
    ).toEqual(['example.com']);
  });

  it('falls back to name parsing without an endpoint (list items)', () => {
    expect(hostsForDiagConfig({ name: 'Diag set: example.com +1 (Quick)' })).toEqual([
      'example.com',
    ]);
  });
});

describe('probeRunVerdict', () => {
  const NOW = Date.parse('2026-08-20T12:00:00Z');
  const STALE = 24 * 60 * 60 * 1000;
  const run = (over: Partial<Parameters<typeof probeRunVerdict>[0]> = {}) => ({
    status: 'completed',
    success_count: 4,
    failure_count: 0,
    created_at: '2026-08-20T11:00:00Z',
    ...over,
  });

  it('mirrors the pre-#820 run-level rules', () => {
    expect(probeRunVerdict(run(), NOW, STALE)).toBe('healthy');
    expect(probeRunVerdict(run({ failure_count: 1 }), NOW, STALE)).toBe('partial');
    expect(probeRunVerdict(run({ success_count: 0, failure_count: 4 }), NOW, STALE)).toBe('failed');
    expect(probeRunVerdict(run({ status: 'failed' }), NOW, STALE)).toBe('failed');
    expect(probeRunVerdict(run({ status: 'running' }), NOW, STALE)).toBe('pending');
    expect(probeRunVerdict(run({ created_at: '2026-08-18T11:00:00Z' }), NOW, STALE)).toBe('stale');
    expect(probeRunVerdict(run({ success_count: 0 }), NOW, STALE)).toBe('pending');
  });

  it('per-URL counts override the run-level aggregate (#820 — one flaky set member)', () => {
    // The set run is completed-with-failures (partial at run level), but THIS
    // member's attempts all succeeded → its row is healthy…
    const setRun = run({ success_count: 6, failure_count: 2 });
    expect(probeRunVerdict(setRun, NOW, STALE, { ok: 4, fail: 0 })).toBe('healthy');
    // …while the member that owned the failures reads failed/partial.
    expect(probeRunVerdict(setRun, NOW, STALE, { ok: 0, fail: 2 })).toBe('failed');
    expect(probeRunVerdict(setRun, NOW, STALE, { ok: 2, fail: 2 })).toBe('partial');
  });

  it('staleness still wins over healthy with per-URL counts', () => {
    const old = run({ created_at: '2026-08-10T11:00:00Z' });
    expect(probeRunVerdict(old, NOW, STALE, { ok: 4, fail: 0 })).toBe('stale');
  });
});
