/**
 * Watchlist membership (#765): only the configs the URL-probe page itself
 * creates — named "Diag: <host> (Preset)" (pre-rename "Probe: …"), or
 * "Diag set: <host> +N (Preset)" for multi-URL set runs (#782/#820) — are
 * watch entries. Every network-kind config used to qualify, which pulled
 * auto-provisioned benchmark/matrix-cell targets (nwk-a-*.cloudapp.azure.com),
 * canary configs, and SDK endpoints into the watchlist: dead hosts accumulated
 * after their deployment was torn down (25 failed / 21 stale of 67 in the
 * 2026-08-18 E2E pass), and each config fired its own detail fetch
 * (~76 requests on page load).
 */

import { extractHost } from './diag-request';

export function isWatchlistConfigName(name: string): boolean {
  return /^(?:Probe|Diag)(?: set)?: /.test(name);
}

/** True for the multi-URL set configs the probe page creates (#782). */
export function isDiagSetConfigName(name: string): boolean {
  return /^(?:Probe|Diag) set: /.test(name);
}

/**
 * Hosts a probe-page config name refers to, parsed from the name alone.
 * "Diag: <host> (Preset)" yields the host; "Diag set: <host> +N (Preset)"
 * yields only the FIRST member (the name carries no more) — callers with the
 * config detail should prefer {@link hostsForDiagConfig}, which reads
 * endpoint.hosts and returns every member.
 */
export function hostsFromDiagConfigName(name: string): string[] {
  const m = name.match(/^(?:Probe|Diag)(?: set)?:\s+(.+?)\s+(?:\+\d+\s+)?\(/);
  return m ? [m[1]] : [];
}

/** Minimal shape shared by TestConfig / TestConfigListItem for host lookup. */
export interface DiagConfigLike {
  name: string;
  endpoint?: { kind: string; host?: string; hosts?: string[] };
}

/**
 * Every watched host a probe config covers. Set configs (#820) contribute ALL
 * their member URLs — this is what attributes a "Diag set:" run to each
 * member's watched-URL row. Falls back to name parsing for list items (no
 * endpoint) and non-network shapes.
 */
export function hostsForDiagConfig(config: DiagConfigLike): string[] {
  const ep = config.endpoint;
  if (ep && ep.kind === 'network') {
    const urls = ep.hosts && ep.hosts.length > 0 ? ep.hosts : ep.host ? [ep.host] : [];
    const hosts = urls.map(extractHost).filter(Boolean);
    if (hosts.length > 0) return [...new Set(hosts)];
  }
  return hostsFromDiagConfigName(config.name);
}

export type ProbeVerdict = 'healthy' | 'partial' | 'failed' | 'stale' | 'pending';

/**
 * Health verdict for a watched-URL row (shared by the summary strip and the
 * card border/dot). Verdict rule shared with the Runs pages (runDisplayStatus,
 * audit F9): completed-with-some-failures reads "partial", not "failed" — the
 * same run must never be green on /runs and red here. Staleness ("no check in
 * 24h") is evaluated before healthy so an old green check surfaces as stale.
 *
 * `counts` (per-URL attempt tallies from a set run's attempts, #820) override
 * the run-level success/failure counts: a set run where only ONE member URL
 * failed must not paint every member red.
 */
export function probeRunVerdict(
  run: { status: string; success_count: number; failure_count: number; created_at: string },
  now: number,
  staleThresholdMs: number,
  counts?: { ok: number; fail: number },
): ProbeVerdict {
  const ok = counts ? counts.ok : run.success_count;
  const fail = counts ? counts.fail : run.failure_count;
  const timeSinceLastRun = now - new Date(run.created_at).getTime();
  if (run.status === 'failed' || run.status === 'cancelled' || (fail > 0 && ok === 0)) {
    return 'failed';
  }
  if (run.status === 'completed' && fail > 0) return 'partial';
  if (run.status === 'queued' || run.status === 'provisioning' || run.status === 'running') {
    return 'pending';
  }
  if (timeSinceLastRun > staleThresholdMs) return 'stale';
  if (run.status === 'completed' && ok > 0) return 'healthy';
  // completed-with-no-attempts or unknown — neither healthy nor failed.
  return 'pending';
}
