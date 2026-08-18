import type { LiveAttempt } from '../../api/types';

export function groupByProtocol(attempts: LiveAttempt[]): Record<string, LiveAttempt[]> {
  const groups: Record<string, LiveAttempt[]> = {};
  for (const attempt of attempts) {
    const key = attempt.protocol || 'unknown';
    (groups[key] ??= []).push(attempt);
  }
  return groups;
}

/**
 * Group attempts by the URL they probed (multi-URL set runs, #782). Attempts
 * without a target_url (single-URL runs, pre-#782 testers) land under '' —
 * callers render the flat single-URL layout when there are fewer than two
 * NON-EMPTY url keys.
 */
export function groupByTargetUrl(attempts: LiveAttempt[]): Record<string, LiveAttempt[]> {
  const groups: Record<string, LiveAttempt[]> = {};
  for (const attempt of attempts) {
    const key = attempt.target_url ?? '';
    (groups[key] ??= []).push(attempt);
  }
  return groups;
}
