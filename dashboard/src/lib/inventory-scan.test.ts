// The exact ambiguity that made "scan all providers" look dead: after a scan
// that returned nothing, the panel repeated the pre-scan prompt. These pin that
// a scan is always distinguishable from never having scanned.

import { describe, it, expect } from 'vitest';
import {
  formatScanTime,
  inventoryEmptyState,
  inventoryScanSummary,
  type InventoryScanState,
} from './inventory-scan';

// Local wall-clock 16:14 whatever the machine's timezone is.
const AT = new Date(2026, 7, 20, 16, 14, 3);

describe('formatScanTime', () => {
  it('is 24h, zero-padded, locale-independent', () => {
    expect(formatScanTime(AT)).toBe('16:14');
    expect(formatScanTime(new Date(2026, 7, 20, 9, 5))).toBe('09:05');
    expect(formatScanTime(new Date(2026, 7, 20, 0, 0))).toBe('00:00');
  });
});

describe('inventoryEmptyState', () => {
  it('prompts to scan only BEFORE the first scan', () => {
    expect(inventoryEmptyState({ status: 'never' })).toBe(
      'Click "scan all providers" to discover VMs across Azure, AWS, and GCP.'
    );
  });

  it('after an empty scan, says what was scanned and when', () => {
    const state: InventoryScanState = {
      status: 'done',
      at: AT,
      scanned: ['azure', 'aws', 'gcp'],
      notConfigured: [],
    };
    expect(inventoryEmptyState(state)).toBe('no VMs found — scanned azure, aws, gcp at 16:14');
    // …and never the pre-scan prompt again.
    expect(inventoryEmptyState(state)).not.toContain('Click "scan all providers"');
  });

  it('names the providers it could not scan for want of an account', () => {
    expect(
      inventoryEmptyState({ status: 'done', at: AT, scanned: ['azure'], notConfigured: ['aws', 'gcp'] })
    ).toBe(
      'no VMs found — scanned azure at 16:14 · not scanned: aws, gcp (no cloud account configured)'
    );
  });

  it('a project with no cloud accounts at all is told what to do', () => {
    const text = inventoryEmptyState({
      status: 'done',
      at: AT,
      scanned: [],
      notConfigured: ['azure', 'aws', 'gcp'],
    });
    expect(text).toContain('nothing scanned at 16:14');
    expect(text).toContain('not scanned: azure, aws, gcp (no cloud account configured)');
    expect(text).toContain('add a cloud account');
  });

  it('a failed request is not mistaken for an empty result', () => {
    const text = inventoryEmptyState({ status: 'failed', at: AT });
    expect(text).toBe('scan failed at 16:14 — the request did not complete; try again.');
    expect(text).not.toContain('no VMs found');
  });
});

describe('inventoryScanSummary', () => {
  it('is empty before the first scan (nothing to caption)', () => {
    expect(inventoryScanSummary({ status: 'never' })).toBe('');
  });

  it('captions a successful scan under the results table', () => {
    expect(
      inventoryScanSummary({ status: 'done', at: AT, scanned: ['azure', 'gcp'], notConfigured: ['aws'] })
    ).toBe('scanned azure, gcp at 16:14 · not scanned: aws (no cloud account configured)');
  });
});
