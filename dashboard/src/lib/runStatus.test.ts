import { describe, expect, it } from 'vitest';
import { runDisplayStatus } from './runStatus';

describe('runDisplayStatus (audit F9 — one verdict per run everywhere)', () => {
  it('completed with zero failures stays completed', () => {
    expect(runDisplayStatus({ status: 'completed', success_count: 9, failure_count: 0 })).toBe('completed');
  });

  it('completed-with-failures becomes partial (the cloudflare 7/9 case)', () => {
    expect(runDisplayStatus({ status: 'completed', success_count: 7, failure_count: 2 })).toBe('partial');
  });

  it('completed where everything failed reads failed', () => {
    expect(runDisplayStatus({ status: 'completed', success_count: 0, failure_count: 9 })).toBe('failed');
  });

  it('non-completed statuses pass through verbatim', () => {
    for (const s of ['queued', 'provisioning', 'running', 'failed', 'cancelled']) {
      expect(runDisplayStatus({ status: s, success_count: 1, failure_count: 1 })).toBe(s);
    }
  });
});
