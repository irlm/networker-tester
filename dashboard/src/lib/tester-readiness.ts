import type { TesterRow } from '../api/testers';

/**
 * A runner can accept work only when its VM is running and its agent is
 * connected. Keep every launch surface and readiness summary on this exact
 * predicate so handoff cannot change the meaning of "online".
 */
export function isOnlineTester(tester: TesterRow): boolean {
  return tester.power_state === 'running' && tester.agent_status === 'online';
}
