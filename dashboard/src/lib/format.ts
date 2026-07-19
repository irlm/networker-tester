/**
 * Format an ISO timestamp as a relative time string ("2m ago", "3h ago", "2d ago").
 * Falls back to the raw string on invalid input.
 */
export function timeAgo(iso: string): string {
  try {
    const parsed = new Date(iso).getTime();
    // new Date('garbage') yields NaN without throwing — honor the documented
    // raw-string fallback instead of rendering "NaNd ago".
    if (Number.isNaN(parsed)) return iso;
    const ms = Date.now() - parsed;
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
 * Compact millisecond formatter for perf instrumentation tables
 * ("-" for missing, "<1ms", otherwise one decimal place).
 *
 * Distinct from `lib/analysis.ts::formatMs`, which renders probe measurements
 * with µs precision — keep that one for measurement data.
 */
export function formatMsCompact(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '-';
  if (ms < 1) return '<1ms';
  return `${ms.toFixed(1)}ms`;
}

/**
 * Short display label for an endpoint host. FQDNs shorten to their first
 * label; IP addresses are returned whole — `host.split('.')[0]` used to
 * title endpoint rows "136" / "34" (audit F17).
 */
export function hostLabel(host: string): string {
  const trimmed = host.trim();
  // IPv4 (all-numeric labels) or IPv6 (contains ':') — show the full address.
  if (trimmed.includes(':') || /^\d{1,3}(\.\d{1,3}){3}$/.test(trimmed)) return trimmed;
  const first = trimmed.split('.')[0];
  // Defensive: a leading numeric label (e.g. truncated IP) is meaningless alone.
  if (/^\d+$/.test(first)) return trimmed;
  return first || trimmed;
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
