import { describe, expect, it } from 'vitest';
import type { TesterRow } from '../api/testers';
import { isOnlineTester } from './tester-readiness';

const tester = (power_state: TesterRow['power_state'], agent_status: string | null) => ({
  power_state,
  agent_status,
}) as TesterRow;

describe('isOnlineTester', () => {
  it('requires both a running VM and a connected agent', () => {
    expect(isOnlineTester(tester('running', 'online'))).toBe(true);
    expect(isOnlineTester(tester('running', 'offline'))).toBe(false);
    expect(isOnlineTester(tester('stopped', 'online'))).toBe(false);
    expect(isOnlineTester(tester('starting', null))).toBe(false);
  });
});
