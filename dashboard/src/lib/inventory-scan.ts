// What the cloud-inventory panel says about the LAST scan.
//
// The bug this exists for: after a scan that found nothing, the panel re-rendered
// `Click "scan all providers" to discover VMs…` — byte-identical to never having
// clicked. The owner reasonably concluded the button was dead (the backend was a
// stub, but the UI made it impossible to tell). So the empty state is driven by
// scan STATE, never by `inventory.length === 0`: once a scan has happened the
// panel says what was scanned, when, and what was skipped.
//
// Kept as its own module (no React) so the wording is unit-testable.

export type InventoryScanState =
  | { status: 'never' }
  /** The request itself failed (network, 5xx) — nothing was scanned. */
  | { status: 'failed'; at: Date }
  | {
      status: 'done';
      at: Date;
      /** Providers the server actually queried. */
      scanned: string[];
      /** Providers the project has no cloud account for — an absence, not an error. */
      notConfigured: string[];
    };

/** 24h local wall-clock, formatted from the Date's own parts so the string is
 *  stable across locales (and across test machines). */
export function formatScanTime(at: Date): string {
  const hh = String(at.getHours()).padStart(2, '0');
  const mm = String(at.getMinutes()).padStart(2, '0');
  return `${hh}:${mm}`;
}

/**
 * One-line provenance for a completed scan: what was scanned, when, and what
 * was skipped for want of an account. Empty string before the first scan.
 * Shown under the results table and inside the empty state.
 */
export function inventoryScanSummary(state: InventoryScanState): string {
  if (state.status === 'never') return '';
  if (state.status === 'failed') return `scan failed at ${formatScanTime(state.at)}`;

  const parts = [
    state.scanned.length > 0
      ? `scanned ${state.scanned.join(', ')} at ${formatScanTime(state.at)}`
      : `nothing scanned at ${formatScanTime(state.at)}`,
  ];
  if (state.notConfigured.length > 0) {
    parts.push(`not scanned: ${state.notConfigured.join(', ')} (no cloud account configured)`);
  }
  return parts.join(' · ');
}

/**
 * What the panel shows when there are no VM rows. Never ambiguous: the
 * "click to scan" prompt appears only before the first scan.
 */
export function inventoryEmptyState(state: InventoryScanState): string {
  if (state.status === 'never') {
    return 'Click "scan all providers" to discover VMs across Azure, AWS, and GCP.';
  }
  if (state.status === 'failed') {
    return `${inventoryScanSummary(state)} — the request did not complete; try again.`;
  }
  if (state.scanned.length === 0) {
    return `${inventoryScanSummary(state)} — add a cloud account below to scan for VMs.`;
  }
  return `no VMs found — ${inventoryScanSummary(state)}`;
}
