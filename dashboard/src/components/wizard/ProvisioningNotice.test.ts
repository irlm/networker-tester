import { describe, it, expect } from 'vitest';
import { provisioningSummary } from './provisioning-notice';

describe('provisioningSummary', () => {
  it('states VM count, location, and the charge warning', () => {
    const s = provisioningSummary({ vmCount: 1, cloud: 'Azure', region: 'eastus', onlineRunners: 2 });
    expect(s.headline).toContain('1 VM');
    expect(s.headline).toContain('Azure · eastus');
    expect(s.headline).toMatch(/charges apply/i);
  });

  it('pluralizes VMs', () => {
    expect(provisioningSummary({ vmCount: 3, cloud: 'AWS', region: 'us-east-1', onlineRunners: 1 }).headline)
      .toContain('3 VMs');
  });

  it('omits the region separator when region is empty', () => {
    const s = provisioningSummary({ vmCount: 1, cloud: 'multiple', region: '', onlineRunners: 1 });
    expect(s.headline).toContain('on multiple.');
    expect(s.headline).not.toContain('·');
  });

  it('flags runner readiness green when runners are online', () => {
    const s = provisioningSummary({ vmCount: 1, cloud: 'Azure', region: 'eastus', onlineRunners: 2 });
    expect(s.runnerOk).toBe(true);
    expect(s.runner).toContain('2 runners online');
  });

  it('warns and singularizes when exactly one runner is online', () => {
    const s = provisioningSummary({ vmCount: 1, cloud: 'Azure', region: 'eastus', onlineRunners: 1 });
    expect(s.runnerOk).toBe(true);
    expect(s.runner).toContain('1 runner online');
  });

  it('warns when no runner is online (target provisions, run queues)', () => {
    const s = provisioningSummary({ vmCount: 1, cloud: 'Azure', region: 'eastus', onlineRunners: 0 });
    expect(s.runnerOk).toBe(false);
    expect(s.runner).toMatch(/no runner online/i);
    expect(s.runner).toMatch(/queues/i);
  });
});
