import { describe, expect, it } from 'vitest';
import {
  DEFAULT_METHODOLOGY,
  METHODOLOGY_PRESETS,
  methodologyForPreset,
} from './testbed-constants';

describe('methodologyForPreset (audit F14 — Review must match Methodology)', () => {
  it('seeds Standard with the numbers the Standard card advertises (10/50/5%)', () => {
    const std = METHODOLOGY_PRESETS.find(p => p.id === 'standard')!;
    const m = methodologyForPreset('standard');
    expect(m.warmup_runs).toBe(std.warmup);
    expect(m.measured_runs).toBe(std.measured);
    expect(m.target_error_pct).toBe(std.targetError);
    // The regression: wizards defaulted to DEFAULT_METHODOLOGY (5/30/2%)
    // while highlighting Standard (10/50/5%).
    expect(m.warmup_runs).toBe(10);
    expect(m.measured_runs).toBe(50);
    expect(m.target_error_pct).toBe(5);
  });

  it('maps every preset id to its own advertised numbers', () => {
    for (const p of METHODOLOGY_PRESETS) {
      const m = methodologyForPreset(p.id);
      expect(m.warmup_runs).toBe(p.warmup);
      expect(m.measured_runs).toBe(p.measured);
      expect(m.target_error_pct).toBe(p.targetError ?? 0);
    }
  });

  it('preserves the non-preset methodology fields (gates, outlier policy)', () => {
    const m = methodologyForPreset('standard');
    expect(m.cooldown_ms).toBe(DEFAULT_METHODOLOGY.cooldown_ms);
    expect(m.quality_gates).toEqual(DEFAULT_METHODOLOGY.quality_gates);
    expect(m.publication_gates).toEqual(DEFAULT_METHODOLOGY.publication_gates);
    expect(m.outlier_policy).toEqual(DEFAULT_METHODOLOGY.outlier_policy);
  });

  it('falls back to defaults for unknown preset ids', () => {
    expect(methodologyForPreset('nope')).toEqual(DEFAULT_METHODOLOGY);
  });
});
