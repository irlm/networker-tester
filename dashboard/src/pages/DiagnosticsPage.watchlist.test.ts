// #765: the URL-probe watchlist accumulated dead nwk-a-* hosts because EVERY
// network-kind config counted as a watch entry — auto-provisioned matrix-cell
// targets, canary configs, and SDK endpoints included — and nothing cleaned
// them up when their deployment was torn down. Membership is now scoped to the
// configs the probe page itself creates ("Diag: <host> (Preset)"; pre-rename
// "Probe: …"). This pins the predicate.

import { describe, expect, it } from 'vitest';
import { isWatchlistConfigName } from '../lib/watchlist';

describe('isWatchlistConfigName', () => {
  it('accepts the probe page naming (current and pre-rename)', () => {
    expect(isWatchlistConfigName('Diag: example.com (Quick)')).toBe(true);
    expect(isWatchlistConfigName('Diag: nwk-a-abc.eastus.cloudapp.azure.com (Full)')).toBe(true);
    expect(isWatchlistConfigName('Probe: example.com (Standard)')).toBe(true);
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
  });
});
