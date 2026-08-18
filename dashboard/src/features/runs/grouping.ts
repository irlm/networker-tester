import type { LiveAttempt } from '../../api/types';

export function groupByProtocol(attempts: LiveAttempt[]): Record<string, LiveAttempt[]> {
  const groups: Record<string, LiveAttempt[]> = {};
  for (const attempt of attempts) {
    const key = attempt.protocol || 'unknown';
    (groups[key] ??= []).push(attempt);
  }
  return groups;
}
