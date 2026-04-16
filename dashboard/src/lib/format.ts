/**
 * Format an ISO timestamp as a relative time string ("2m ago", "3h ago", "2d ago").
 * Falls back to the raw string on invalid input.
 */
export function timeAgo(iso: string): string {
  try {
    const ms = Date.now() - new Date(iso).getTime();
    if (ms < 0) return 'just now';
    const secs = Math.floor(ms / 1000);
    if (secs < 60) return `${secs}s ago`;
    const mins = Math.floor(secs / 60);
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    return `${days}d ago`;
  } catch {
    return iso;
  }
}

/**
 * Format a duration between two timestamps as a human-readable string.
 * Handles both string (ISO) and Date inputs.
 * Returns "\u2014" if start is null/undefined.
 */
export function formatDuration(
  start: string | Date | null | undefined,
  end: string | Date | null | undefined,
): string {
  if (!start) return '\u2014';
  const s = start instanceof Date ? start.getTime() : new Date(start).getTime();
  const e = end
    ? (end instanceof Date ? end.getTime() : new Date(end).getTime())
    : Date.now();
  const ms = e - s;
  if (ms < 1000) return `${ms}ms`;
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${secs % 60}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}
