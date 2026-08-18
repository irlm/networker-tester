/**
 * Watchlist membership (#765): only the configs the URL-probe page itself
 * creates — named "Diag: <host> (Preset)" (pre-rename "Probe: …") — are watch
 * entries. Every network-kind config used to qualify, which pulled
 * auto-provisioned benchmark/matrix-cell targets (nwk-a-*.cloudapp.azure.com),
 * canary configs, and SDK endpoints into the watchlist: dead hosts accumulated
 * after their deployment was torn down (25 failed / 21 stale of 67 in the
 * 2026-08-18 E2E pass), and each config fired its own detail fetch
 * (~76 requests on page load).
 */
export function isWatchlistConfigName(name: string): boolean {
  return /^(?:Probe|Diag): /.test(name);
}
