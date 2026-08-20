import type { LiveAttempt } from '../../api/types';
import { stripAnsi } from '../../lib/ansi';

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

/**
 * One attempt's failure reason for display (#824). Prefers the live stream's
 * structured `error.message`, falls back to the REST rows' flat
 * `error_message` (old rows carry raw ANSI SGR codes — stripped here).
 * Null when the attempt succeeded or recorded no reason.
 */
export function attemptFailureReason(a: LiveAttempt): string | null {
  if (a.success) return null;
  const raw = a.error?.message ?? a.error_message;
  if (!raw) return null;
  const clean = stripAnsi(raw).trim();
  return clean.length > 0 ? clean : null;
}

/**
 * The dominant failure reason of a protocol group, for the collapsed block
 * header ("5 FAIL — QUIC handshake timeout", #824). Returns the most common
 * reason when it covers MORE THAN HALF of the group's failures — a grab-bag
 * of distinct reasons crowns nobody rather than misleading with one of them.
 */
export function dominantFailureReason(attempts: LiveAttempt[]): string | null {
  const failures = attempts.filter((a) => !a.success);
  if (failures.length === 0) return null;
  const counts = new Map<string, number>();
  for (const a of failures) {
    const reason = attemptFailureReason(a);
    if (reason) counts.set(reason, (counts.get(reason) ?? 0) + 1);
  }
  let best: string | null = null;
  let bestCount = 0;
  for (const [reason, count] of counts) {
    if (count > bestCount) { best = reason; bestCount = count; }
  }
  return best != null && bestCount * 2 > failures.length ? best : null;
}
