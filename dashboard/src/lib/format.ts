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
