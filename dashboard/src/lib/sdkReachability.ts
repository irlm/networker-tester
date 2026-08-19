import type { SdkEndpoint } from '../api/types';

/**
 * Reachability verdict from an SDK endpoint's latest sdkprobe run (#765: dead
 * endpoints rendered identically to live ones). Mirrors the URL-probe
 * watchlist rule (runDisplayStatus family): completed-with-some-successes is
 * reachable (partial when some attempts failed); failed / cancelled /
 * completed-all-fail is unreachable; active statuses are probing; no run at
 * all (or a completed run that recorded no attempts) yields no verdict.
 */
export type SdkReachability = 'reachable' | 'partial' | 'unreachable' | 'probing' | 'none';

export function sdkReachability(
  ep: Pick<SdkEndpoint, 'last_run_status' | 'last_run_success_count' | 'last_run_failure_count'>,
): SdkReachability {
  const status = ep.last_run_status;
  if (!status) return 'none';
  if (status === 'queued' || status === 'provisioning' || status === 'running') return 'probing';
  const ok = ep.last_run_success_count ?? 0;
  const fail = ep.last_run_failure_count ?? 0;
  if (status === 'failed' || status === 'cancelled' || (fail > 0 && ok === 0)) return 'unreachable';
  if (status === 'completed' && ok > 0) return fail > 0 ? 'partial' : 'reachable';
  return 'none';
}
