/**
 * Watchlist membership (#765): only the configs the URL-probe page itself
 * creates are watch entries. The STRUCTURAL test_kind='url_probe' field is
 * authoritative when the row carries one — it survives renames and can't be
 * spoofed by a benchmark config that happens to be named "Diag: …". The name
 * prefix — "Diag: <host> (Preset)" (pre-rename "Probe: …"), or
 * "Diag set: <host> +N [hash] (Preset)" for multi-URL set runs (#782/#820) —
 * is only the legacy fallback for pre-test_kind rows. Every network-kind
 * config used to qualify, which pulled auto-provisioned benchmark/matrix-cell
 * targets (nwk-a-*.cloudapp.azure.com), canary configs, and SDK endpoints
 * into the watchlist: dead hosts accumulated after their deployment was torn
 * down (25 failed / 21 stale of 67 in the 2026-08-18 E2E pass), and each
 * config fired its own detail fetch (~76 requests on page load).
 */

import { extractHost } from './diag-request';

export function isWatchlistConfigName(name: string): boolean {
  return /^(?:Probe|Diag)(?: set)?: /.test(name);
}

/**
 * Watchlist membership: the structural kind wins when the row carries one; the
 * name prefix is only the legacy fallback (pre-test_kind rows). Deciding by
 * name alone meant renaming a probe config silently erased its watchlist
 * history, and any config named "Diag set: …" got injected.
 */
export function isWatchlistConfig(config: { name: string; test_kind?: string | null }): boolean {
  if (config.test_kind) return config.test_kind === 'url_probe';
  return isWatchlistConfigName(config.name);
}

/** True for the multi-URL set configs the probe page creates (#782). */
export function isDiagSetConfigName(name: string): boolean {
  return /^(?:Probe|Diag) set: /.test(name);
}

/**
 * Hosts a probe-page config name refers to, parsed from the name alone.
 * "Diag: <host> (Preset)" yields the host; "Diag set: <host> +N [hash]
 * (Preset)" yields only the FIRST member (the name carries no more) — the
 * `[hex]` membership hash (optional: pre-hash set names lack it) is skipped.
 * Callers with the config detail should prefer {@link hostsForDiagConfig},
 * which reads endpoint.hosts and returns every member.
 */
export function hostsFromDiagConfigName(name: string): string[] {
  const m = name.match(/^(?:Probe|Diag)(?: set)?:\s+(.+?)\s+(?:\+\d+\s+(?:\[[0-9a-f]+\]\s+)?)?\(/);
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
 * `counts` (per-URL attempt tallies from a set run's attempts, #820) are
 * authoritative per-member evidence and OVERRIDE run-level status, including
 * failed/cancelled: a set run killed by the watchdog reports status 'failed',
 * but a member whose own attempts all succeeded must stay green — run-level
 * verdicts painted every member red and defeated the per-member verdict.
 * {ok:0, fail:0} means "no attributed attempts for this member" — no evidence,
 * so neither green nor red ('pending').
 */
export function probeRunVerdict(
  run: { status: string; success_count: number; failure_count: number; created_at: string },
  now: number,
  staleThresholdMs: number,
  counts?: { ok: number; fail: number },
): ProbeVerdict {
  if (run.status === 'queued' || run.status === 'provisioning' || run.status === 'running') {
    return 'pending';
  }
  const timeSinceLastRun = now - new Date(run.created_at).getTime();
  if (counts) {
    if (counts.ok === 0 && counts.fail === 0) return 'pending';
    if (counts.fail > 0 && counts.ok === 0) return 'failed';
    if (counts.fail > 0) return 'partial';
    if (timeSinceLastRun > staleThresholdMs) return 'stale';
    return 'healthy';
  }
  if (
    run.status === 'failed'
    || run.status === 'cancelled'
    || (run.failure_count > 0 && run.success_count === 0)
  ) {
    return 'failed';
  }
  if (run.status === 'completed' && run.failure_count > 0) return 'partial';
  if (timeSinceLastRun > staleThresholdMs) return 'stale';
  if (run.status === 'completed' && run.success_count > 0) return 'healthy';
  // completed-with-no-attempts or unknown — neither healthy nor failed.
  return 'pending';
}
