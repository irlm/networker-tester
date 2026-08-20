import { useEffect } from 'react';

/**
 * Lock page scrolling while an overlay is open.
 *
 * Refcounted on purpose. The naive version — save `document.body.style.overflow`
 * on mount, restore it on unmount — is a save/restore of GLOBAL state, so two
 * overlaps break it: open a modal over the mobile drawer, close either one, and
 * whichever unmounts first restores the value it captured (usually `''`),
 * unlocking the page behind the overlay that is still open. Counting instead
 * means the lock lifts exactly when the last holder releases it.
 *
 * The pre-lock value is captured once, when the count goes 0 → 1, and restored
 * once, when it returns to 0 — so a page that legitimately set its own
 * `overflow` keeps it.
 */
let lockCount = 0;
let previousOverflow: string | null = null;

function acquire(): void {
  if (lockCount === 0) {
    previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
  }
  lockCount += 1;
}

function release(): void {
  lockCount = Math.max(0, lockCount - 1);
  if (lockCount === 0 && previousOverflow !== null) {
    document.body.style.overflow = previousOverflow;
    previousOverflow = null;
  }
}

/** Hold the lock while `active` is true; release it on unmount. */
export function useBodyScrollLock(active: boolean): void {
  useEffect(() => {
    if (!active) return;
    acquire();
    return release;
  }, [active]);
}

/** Test-only: reset the module-level counter between cases. */
export function __resetBodyScrollLock(): void {
  lockCount = 0;
  previousOverflow = null;
  document.body.style.overflow = '';
}
