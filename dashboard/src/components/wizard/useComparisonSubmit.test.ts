import { describe, expect, it } from 'vitest';
import { launchOutcomeToast } from './useComparisonSubmit';

// #793 P1-3: the submit hook toasted `Launched ${cells.length} runs` from the
// REQUESTED count — a fully-failed matrix launch showed a success toast. The
// toast now derives from the server's actual {launched, total, failed, errors}.
describe('launchOutcomeToast', () => {
  it('error toast listing the per-cell errors when nothing launched', () => {
    const t = launchOutcomeToast({
      launched: 0,
      total: 2,
      failed: 2,
      errors: [
        "rust @ AWS: cloud account 'AWS' is in error state: Invalid access key ID",
        'go @ AWS: same',
      ],
    });
    expect(t.type).toBe('error');
    expect(t.message).toContain('0 of 2 runs launched');
    expect(t.message).toContain("cloud account 'AWS' is in error state: Invalid access key ID");
    expect(t.message).toContain('go @ AWS: same');
  });

  it('partial launch is not a success — counts and errors are surfaced', () => {
    const t = launchOutcomeToast({
      launched: 3,
      total: 4,
      failed: 1,
      errors: ['win · haproxy: HAProxy has no native Windows build'],
    });
    expect(t.type).toBe('info');
    expect(t.message).toContain('Launched 3 of 4 runs');
    expect(t.message).toContain('1 failed');
    expect(t.message).toContain('HAProxy has no native Windows build');
  });

  it('success uses the RETURNED launched count, with singular/plural', () => {
    expect(launchOutcomeToast({ launched: 4, total: 4, failed: 0 })).toEqual({
      type: 'success',
      message: 'Launched 4 runs',
    });
    expect(launchOutcomeToast({ launched: 1, total: 1, failed: 0, errors: null })).toEqual({
      type: 'success',
      message: 'Launched 1 run',
    });
  });

  it('tolerates a null errors field on a failed launch', () => {
    const t = launchOutcomeToast({ launched: 0, total: 1, failed: 1, errors: null });
    expect(t.type).toBe('error');
    expect(t.message).toBe('Launch failed: 0 of 1 runs launched');
  });
});
